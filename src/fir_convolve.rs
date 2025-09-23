// Optimized symmetric FIR convolver with optional runtime‑selected SIMD (AVX2 / NEON).
// Assumes an EVEN total number of taps (no center tap).
// Input slice second_half_taps = "right" half (N/2 taps). Full length N = 2 * second_half_taps.len().

#[derive(Clone, Copy)]
enum ImplKind {
    Scalar,
    Avx2,
    Neon,
}

pub struct FirConvolve {
    left_half: Vec<f64>, // taps[0 .. N/2 -1] (left half, outermost -> innermost)
    state: Vec<f64>,
    write_idx: usize,
    n: usize,
    half: usize,
    kind: ImplKind,
    // Full taps (linear phase, even length) for event (impulse) engine
    full_taps: Vec<f64>,
    // Impulse accumulation ring (event-driven convolution for i8 path)
    imp_acc: Vec<f64>,
    imp_pos: usize,
}

// Fast lookup converting i8 sample to f64.
// Generic (identity) so we can pass any i8; primary use: {-1,0,1} for DSD bit / zero-stuffed data.
static I8_TO_F64: [f64; 256] = {
    let mut tbl = [0.0f64; 256];
    let mut i = 0;
    while i < 256 {
        tbl[i] = (i as i8) as f64;
        i += 1;
    }
    tbl
};

impl FirConvolve {
    pub fn new(second_half_taps: &[f64]) -> Self {
        let half = second_half_taps.len();
        let n = half * 2;
        assert!(half > 0, "FIR must have at least 2 taps total (even length).");

        let mut left_half = second_half_taps.to_vec();
        left_half.reverse(); // now outermost -> inward

        // Reconstruct full impulse response h[0..n-1]
        let mut left = second_half_taps.to_vec();
        left.reverse();
        let full_taps: Vec<f64> = left.iter().chain(second_half_taps.iter()).cloned().collect();
        debug_assert_eq!(full_taps.len(), n);

        let kind = Self::detect_kind();

        FirConvolve {
            left_half,
            state: vec![0.0; n],
            write_idx: 0,
            n,
            half,
            kind,
            full_taps,
            imp_acc: vec![0.0; n],
            imp_pos: 0,
        }
    }

    #[inline]
    fn detect_kind() -> ImplKind {
        #[cfg(all(target_arch = "x86_64"))]
        {
            if std::is_x86_feature_detected!("avx2") {
                return ImplKind::Avx2;
            }
        }
        #[cfg(all(target_arch = "aarch64"))]
        {
            return ImplKind::Neon;
        }
        ImplKind::Scalar
    }

    #[inline]
    pub fn process_sample(&mut self, x: f64) -> f64 {
        self.state[self.write_idx] = x;
        let acc = self.convolve_current();
        self.advance();
        acc
    }

    // Optimized i8 variant: event-driven convolution.
    // For non-zero sample: O(N) add of scaled impulse into future output slots.
    // For zero sample: O(1). Always correct (implements standard FIR).
    #[inline]
    pub fn process_sample_i8(&mut self, x: i8) -> f64 {
        let xv = I8_TO_F64[x as u8 as usize];
        if xv != 0.0 {
            // Inject scaled impulse response starting at current position
            let scale = xv;
            let n = self.n;
            let mut pos = self.imp_pos;
            for &c in &self.full_taps {
                // y[n + k] contribution
                self.imp_acc[pos] += scale * c;
                pos += 1;
                if pos == n { pos = 0; }
            }
        }
        // Output current sample (completed contributions)
        let y = self.imp_acc[self.imp_pos];
        self.imp_acc[self.imp_pos] = 0.0;
        // Advance ring position
        self.imp_pos += 1;
        if self.imp_pos == self.n { self.imp_pos = 0; }
        y
    }

    // Internal: run convolution using current write_idx sample (just written),
    // without advancing the circular pointer.
    #[inline(always)]
    fn convolve_current(&self) -> f64 {
        match self.kind {
            ImplKind::Scalar => self.convolve_scalar(),

            #[cfg(target_arch = "x86_64")]
            ImplKind::Avx2 => unsafe { self.convolve_avx2() },
            #[cfg(not(target_arch = "x86_64"))]
            ImplKind::Avx2 => self.convolve_scalar(),

            #[cfg(target_arch = "aarch64")]
            ImplKind::Neon => unsafe { self.convolve_neon() },
            #[cfg(not(target_arch = "aarch64"))]
            ImplKind::Neon => self.convolve_scalar(),
        }
    }

    // Internal: advance circular index
    #[inline(always)]
    fn advance(&mut self) {
        self.write_idx += 1;
        if self.write_idx == self.n {
            self.write_idx = 0;
        }
    }

    #[inline]
    fn idx_back(&self, base: usize, k: usize) -> usize {
        // (base + n - k) % n  (base always < n, k < n)
        let n = self.n;
        let b = base + n;
        (b - k) % n
    }

    #[inline]
    fn idx_fwd(&self, base: usize, k: usize) -> usize {
        // (base + 1 + k) % n
        (base + 1 + k) % self.n
    }

    #[inline]
    fn convolve_scalar(&self) -> f64 {
        let w = self.write_idx;
        let mut acc = 0.0;
        for k in 0..self.half {
            let a = self.state[self.idx_back(w, k)];
            let b = self.state[self.idx_fwd(w, k)];
            acc += self.left_half[k] * (a + b);
        }
        acc
    }

    // ================= AVX2 (x86_64) =================
    #[cfg(all(target_arch = "x86_64"))]
    #[inline]
    unsafe fn convolve_avx2(&self) -> f64 {
        use std::arch::x86_64::*;
        if self.half < 4 {
            return self.convolve_scalar();
        }
        let w = self.write_idx;
        let mut acc_v = _mm256_setzero_pd();
        let chunks = self.half / 4;
        let tail_start = chunks * 4;

        for chunk in 0..chunks {
            let k0 = chunk * 4;
            // Load coeffs (outermost -> inward)
            let coeffs = _mm256_loadu_pd(self.left_half.as_ptr().add(k0));

            // Gather 4 pair sums manually (still scalar loads but fewer mul/add handling)
            let mut pair_sum: [f64; 4] = [0.0; 4];
            for i in 0..4 {
                let k = k0 + i;
                let a = self.state[self.idx_back(w, k)];
                let b = self.state[self.idx_fwd(w, k)];
                pair_sum[i] = a + b;
            }
            let samples = _mm256_loadu_pd(pair_sum.as_ptr());
            let prod = _mm256_mul_pd(coeffs, samples);
            acc_v = _mm256_add_pd(acc_v, prod);
        }

        // Horizontal add acc_v
        let mut tmp = [0f64; 4];
        _mm256_storeu_pd(tmp.as_mut_ptr(), acc_v);
        let mut acc = tmp.iter().sum();

        // Tail
        for k in tail_start..self.half {
            let a = self.state[self.idx_back(w, k)];
            let b = self.state[self.idx_fwd(w, k)];
            acc += self.left_half[k] * (a + b);
        }
        acc
    }

    // ================= NEON (aarch64) =================
    #[cfg(all(target_arch = "aarch64"))]
    #[inline]
    unsafe fn convolve_neon(&self) -> f64 {
        use core::arch::aarch64::*;
        if self.half < 2 {
            return self.convolve_scalar();
        }

        let w = self.write_idx;
        let mut acc_f64 = 0.0;
        let mut acc_v = vdupq_n_f64(0.0);
        let chunks = self.half / 2;
        let tail_start = chunks * 2;

        for chunk in 0..chunks {
            let k0 = chunk * 2;
            let k1 = k0 + 1;

            // Pair sums
            let s0 = {
                let a = self.state[self.idx_back(w, k0)];
                let b = self.state[self.idx_fwd(w, k0)];
                a + b
            };
            let s1 = {
                let a = self.state[self.idx_back(w, k1)];
                let b = self.state[self.idx_fwd(w, k1)];
                a + b
            };

            // Coeffs
            let c0 = self.left_half[k0];
            let c1 = self.left_half[k1];

            // Load into vectors
            let sums = vld1q_f64([s0, s1].as_ptr());
            let coeffs = vld1q_f64([c0, c1].as_ptr());
            acc_v = vmlaq_f64(acc_v, coeffs, sums);
        }

        // Horizontal add NEON vector
        let pair = vadd_f64(vget_low_f64(acc_v), vget_high_f64(acc_v));
        acc_f64 += vget_lane_f64(pair, 0);

        // Tail
        for k in tail_start..self.half {
            let a = self.state[self.idx_back(w, k)];
            let b = self.state[self.idx_fwd(w, k)];
            acc_f64 += self.left_half[k] * (a + b);
        }
        acc_f64
    }
}

// ============================================================================
// BytePrecalcDecimator
// Minimal, safe, idiomatic Rust adaptation of the C precalc() approach,
// used ONLY for dense bipolar DSD bitstreams with straight integer decimation
// (no zero stuffing). You precompute 256-entry tables for 8-bit windows of the
// HALF (right) filter taps. At each decimated output boundary we sum mirrored
// table contributions, reproducing the full linear‑phase FIR result with
// far fewer operations (O(numTables) vs O(taps)).
//
// Assumptions:
// - Filter specified by right-half taps (second_half_taps), even full length = 2 * len.
// - Bits interpreted bipolar: 0 -> -1.0, 1 -> +1.0 (standard DSD).
// - decim is an integer multiple of 8 (16, 32, 64, etc).
// - No zero insertion; feed raw DSD bytes in arrival order.
// - Produces one output per 'decim' input bits (decim/8 bytes).
//
// Not integrated into existing FirConvolve paths to keep patch minimal;
// use when you know the stream is a straight integer decimation case.
// ============================================================================

pub struct BytePrecalcDecimator {
    // Precomputed tables: tables[i][byte] gives partial sum for segment i
    tables: Vec<Box<[f64; 256]>>,
    num_tables: usize,
    decim: u32,
    bytes_per_out: u32,
    fifo: Vec<u8>,
    fifo_pos: usize,
    gain: f64,          // DC normalization (like ptr->gain in C)
    delay: u64,         // Group delay (half full length minus 1)/2
    delay_count: u64,   // Countdown before starting to emit
    // Cached for mirror addressing
    table_span: usize,  // num_tables * 2 - 1
}

impl BytePrecalcDecimator {
    /// Build from right-half taps (second_half_taps) and integer decimation factor.
    pub fn new(second_half_taps: &[f64], decim: u32) -> Option<Self> {
        if decim % 8 != 0 { return None; } // requires byte alignment
        let half = second_half_taps.len();
        if half == 0 { return None; }
        // Number of 8-bit windows covering half the filter
        let num_tables = (half + 7) / 8;
        // Precompute 256-entry table for each window (like C precalc)
        let mut tables: Vec<Box<[f64; 256]>> = Vec::with_capacity(num_tables);
        for t in 0..num_tables {
            let base = t * 8;
            let remain = half.saturating_sub(base);
            let k = remain.min(8); // up to 8 taps in this window
            // Table index is reversed order (ctx->numTables-1 - t) in C; we can mimic
            // by pushing and later indexing appropriately. Simpler: store in reverse now.
            let mut arr = Box::new([0.0f64; 256]);
            for dsd_seq in 0..256u16 {
                let mut acc = 0.0;
                for bit in 0..k {
                    let bit_is_one = (dsd_seq >> bit) & 1;
                    // Map 0 -> -1, 1 -> +1
                    let sample = if bit_is_one != 0 { 1.0 } else { -1.0 };
                    acc += sample * second_half_taps[base + bit];
                }
                arr[dsd_seq as usize] = acc;
            }
            tables.push(arr);
        }
        // Reverse to align with C's tableIdx = numTables - 1 - t
        tables.reverse();

        // DC gain normalization: full symmetric FIR gain ≈ 2 * sum(half taps)
        let sum_half: f64 = second_half_taps.iter().copied().sum();
        let full_gain = 2.0 * sum_half;
        let gain = if full_gain != 0.0 { 1.0 / full_gain } else { 1.0 };

        let full_len = (half * 2) as u64;
        let delay = (full_len - 1) / 2;
        Some(Self {
            tables,
            num_tables,
            decim,
            bytes_per_out: decim / 8,
            fifo: vec![0u8; (num_tables * 2 + 8).next_power_of_two()], // simple ring
            fifo_pos: 0,
            gain,
            delay,
            delay_count: delay,
            table_span: num_tables * 2 - 1,
        })
    }

    /// Reset internal state (silence pattern like C not strictly required here).
    pub fn reset(&mut self) {
        for b in &mut self.fifo { *b = 0x69; }
        self.fifo_pos = 0;
        self.delay_count = self.delay;
    }

    /// Feed a block of DSD bytes; produce decimated PCM outputs.
    /// Returns number of PCM samples written to `out`.
    pub fn process_bytes(&mut self, bytes: &[u8], out: &mut [f64]) -> usize {
        if self.num_tables == 0 || self.bytes_per_out == 0 { return 0; }
        let mask = self.fifo.len() - 1; // fifo len is power-of-two
        let mut byte_count_in_frame = 0u32;
        let mut produced = 0usize;

        for &b in bytes {
            // Push newest byte
            self.fifo[self.fifo_pos & mask] = b;
            self.fifo_pos = (self.fifo_pos + 1) & mask;
            byte_count_in_frame += 1;

            if byte_count_in_frame == self.bytes_per_out {
                byte_count_in_frame = 0;

                // Accumulate mirrored table contributions
                let mut acc = 0.0;
                // Current position is one past last written
                let cur = self.fifo_pos;
                for i in 0..self.num_tables {
                    // Recent window i
                    let idx1 = cur.wrapping_sub(1 + i) & mask;
                    // Mirrored window
                    let idx2 = cur.wrapping_sub(1 + (self.table_span - i)) & mask;
                    let byte1 = self.fifo[idx1];
                    let byte2 = self.fifo[idx2];
                    // Table i corresponds to tables[i]; mirrored byte must be bit-reversed
                    acc += self.tables[i][byte1 as usize] + self.tables[i][bit_reverse_u8(byte2) as usize];
                }

                // Apply normalization
                acc *= self.gain;

                // Handle startup latency (group delay)
                if self.delay_count > 0 {
                    self.delay_count -= 1;
                } else if produced < out.len() {
                    out[produced] = acc;
                    produced += 1;
                }
                if produced == out.len() { break; }
            }
        }
        produced
    }

    pub fn gain(&self) -> f64 { self.gain }
    pub fn latency(&self) -> u64 { self.delay }
}

// Optional: simple test (enable with `cargo test -- --nocapture`)
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn symmetry_matches_reference() {
        // Create a simple low-pass like shape (even length)
        let right_half = vec![0.4, 0.3, 0.2, 0.1]; // N=8
        let mut fir_simd = FirConvolve::new(&right_half);

        // Reference straightforward full convolution for comparison
        // Reconstruct full taps
        let mut left = right_half.clone(); left.reverse();
        let full: Vec<f64> = left.iter().chain(right_half.iter()).cloned().collect();
        assert_eq!(full.len(), right_half.len()*2);

        let mut state = vec![0.0f64; full.len()];
        let mut w = 0usize;

        for i in 0..100 {
            let x = if i == 0 { 1.0 } else { 0.0 };
            // reference
            state[w] = x;
            let mut ref_acc = 0.0;
            for (t, &h) in full.iter().enumerate() {
                // t=0 is leftmost tap -> corresponds to sample x[n-(N-1 - t)]
                let idx = (w + full.len() - (full.len() - 1 - t)) % full.len();
                ref_acc += h * state[idx];
            }
            w = (w + 1) % full.len();

            let y_simd = fir_simd.process_sample(x);
            assert!((y_simd - ref_acc).abs() < 1e-9, "Mismatch at i={}: {} vs {}", i, y_simd, ref_acc);
        }
    }
}

// ADD: missing bit reversal helper (used by BytePrecalcDecimator)
#[inline]
fn bit_reverse_u8(mut b: u8) -> u8 {
    // Reverse bits in a byte (branchless, 3 shuffle steps)
    b = (b & 0xF0) >> 4 | (b & 0x0F) << 4;
    b = (b & 0xCC) >> 2 | (b & 0x33) << 2;
    b = (b & 0xAA) >> 1 | (b & 0x55) << 1;
    b
}