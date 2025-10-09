#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stage1Mode {
    SlotSim,
    Accum,
}
use crate::filters::HTAPS_1MHZ_3TO1_EQ;
use crate::filters::HTAPS_288K_3TO1_EQ;
use crate::filters::HTAPS_2MHZ_7TO1_EQ;
use crate::filters::HTAPS_4MHZ_7TO1_EQ;
use crate::filters::HTAPS_576K_3TO1_EQ;
use crate::filters::HTAPS_8MHZ_7TO1_EQ;
use crate::filters::HTAPS_DDRX10_21TO1_EQ;
use crate::filters::HTAPS_DDRX10_7TO1_EQ;
use crate::filters::HTAPS_DSDX5_7TO1_EQ;
// NEW: 576 kHz -> /3 (final 192 kHz)
use crate::filters::HTAPS_2_68MHZ_7TO1_EQ;
use crate::filters::HTAPS_DDRX5_14TO1_EQ; // ADD first-stage half taps (5× up, 14:1 down)
use crate::filters::HTAPS_DDRX5_7TO_1_EQ;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
// no extra traits needed

// Global LUT cache for Stage1, shared across instances/channels.
// Keyed by (L, len(right_half), hash(contents of right_half)) to avoid rebuilding.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct Stage1LutKey {
    l: u32,
    n: u32,
    hash: u64,
}

static STAGE1_LUT_CACHE: OnceLock<Mutex<HashMap<Stage1LutKey, Arc<Vec<Vec<Vec<f64>>>>>>> = OnceLock::new();

#[inline]
fn hash_f64_slice_fnv1a(v: &[f64]) -> u64 {
    // 64-bit FNV-1a over raw bit patterns, stable for identical tap tables
    let mut h: u64 = 0xcbf29ce484222325; // FNV offset basis
    const P: u64 = 0x00000100000001B3; // FNV prime
    for &f in v {
        let b = f.to_bits();
        h ^= b;
        h = h.wrapping_mul(P);
    }
    h
}

fn build_stage1_lut_from_right_half(right_half: &[f64], l: u32) -> Vec<Vec<Vec<f64>>> {
    // Reconstruct full symmetric taps and polyphase decomposition
    let mut full: Vec<f64> = right_half.iter().rev().cloned().collect();
    full.extend_from_slice(right_half);
    let mut phases: Vec<Vec<f64>> = vec![Vec::new(); l as usize];
    for (i, &c) in full.iter().enumerate() {
        phases[i % l as usize].push(c);
    }
    // Build 8-bit LUT per phase and per group of 8 taps (newest-first order)
    let mut out: Vec<Vec<Vec<f64>>> = Vec::with_capacity(phases.len());
    for taps in &phases {
        let n = taps.len();
        let groups = (n + 7) / 8;
        let mut phase_lut: Vec<Vec<f64>> = Vec::with_capacity(groups);
        for g in 0..groups {
            let base = g * 8;
            let mut coeffs: [f64; 8] = [0.0; 8];
            for j in 0..8 {
                let idx = base + j;
                if idx < n {
                    coeffs[j] = taps[idx];
                }
            }
            let mut group_lut = vec![0.0f64; 256];
            for b in 0u16..256u16 {
                let mut sum = 0.0f64;
                for j in 0..8 {
                    let sign = if (b >> j) & 1 == 1 { 1.0 } else { -1.0 };
                    sum += coeffs[j] * sign;
                }
                group_lut[b as usize] = sum;
            }
            phase_lut.push(group_lut);
        }
        out.push(phase_lut);
    }
    out
}

fn get_or_build_stage1_lut(right_half: &[f64], l: u32) -> Arc<Vec<Vec<Vec<f64>>>> {
    let key = Stage1LutKey {
        l,
        n: right_half.len() as u32,
        hash: hash_f64_slice_fnv1a(right_half),
    };
    let cache = STAGE1_LUT_CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    // Fast path: try hit
    if let Some(found) = cache.lock().unwrap().get(&key) {
        return found.clone();
    }
    // Miss: build and insert
    let lut = build_stage1_lut_from_right_half(right_half, l);
    let arc = Arc::new(lut);
    cache.lock().unwrap().insert(key, arc.clone());
    arc
}

// Toggle slow-domain Stage1 polyphase (debug / diagnostic).
// Set to true to enable Stage1PolySlow path; keep false for production.
// Slow-domain Stage1 polyphase is always active for L=5 now; legacy toggle removed.

// ====================================================================================
// Generalized equiripple L/M resampler covering:
//   - L=5,  M=294: (×5 -> /14) -> /7 -> /3  -> 96 kHz
//   - L=5,  M=147: (×5 -> /7)  -> /7 -> /3  -> 192 kHz
//   - L=10, M=147: (×10 -> /7) -> /7 -> /3  -> 384 kHz
// Stage1 dump now supported for any M: output after first stage (×L -> /decim1).
// ====================================================================================
pub struct LMResampler {
    // Unified Stage1 polyphase (replaces former Slow + Generic variants)
    stage1_poly: Option<Stage1Poly>,
    // Stage 2 (polyphase decimator by 7) optional (None for two-phase path)
    poly2: Option<DecimFIRSym>,
    // Stage 3 (polyphase decimator by 3) optional (None for two-phase path)
    poly3: Option<DecimFIRSym>,
    // If Some(d), indicates we enabled the (logical) Stage1 polyphase optimization and
    // stores the Stage1 effective input-sample delay (ceil(group_delay_high / L)).
    two_phase_lm147_384: Option<TwoPhaseLM147_384>,
}
impl LMResampler {
    pub fn new(l: u32, m: i32, verbose: bool, out_rate: u32) -> Self {
        match m {
            294 => {
                // Original cascade Stage1 definitions
                let s1 = Stage1Poly::new(&HTAPS_DDRX5_14TO1_EQ, l, 14);
                let s = Self {
                    // Stage 2 (polyphase decimator by 7)
                    poly2: Some(DecimFIRSym::new_from_half(&HTAPS_2MHZ_7TO1_EQ, 7)),
                    // Stage 3 (polyphase decimator by 3)
                    poly3: Some(DecimFIRSym::new_from_half(&HTAPS_288K_3TO1_EQ, 3)),
                    two_phase_lm147_384: None,
                    stage1_poly: Some(s1),
                };
                if verbose {
                    eprintln!(
                            "[DBG] Equiripple L/M path: L={} M=294 — (×L -> /14 -> /7 -> /3) [Stage2/3 polyphase].",
                            l
                        );
                }
                s
            }
            147 => {
                // DSD128 -> 384k two‑phase path (×10 -> /21 -> /7) DEFAULT for L=10
                if (l == 10 || l == 20) && out_rate == 384_000 {
                    if verbose {
                        eprintln!("[DBG] Two-phase L={}/M=147 path enabled: (×L -> /21 (poly) -> /7) => 384k", l);
                    }
                    // Minimal placeholders (not used directly)
                    return Self {
                        poly2: None,
                        poly3: None,
                        two_phase_lm147_384: Some(TwoPhaseLM147_384::new(l)),
                        stage1_poly: None,
                    };
                }

                // (Existing selection logic unchanged, just swap Stage2/3 construction)
                let (stage1_half, right2, right3, label) = if l == 5 && out_rate == 384_000 {
                    // DSD256 -> 384k path (L=5, M=147) reuses SAME Stage1 half taps as other 384k paths (DDRx10_7TO1) per request.
                    // Second and third stage identical to other 384k (8MHz -> /7, 1MHz -> /3).
                    (
                        &HTAPS_DDRX10_7TO1_EQ[..],
                        &HTAPS_8MHZ_7TO1_EQ[..],
                        &HTAPS_1MHZ_3TO1_EQ[..],
                        "384k (L=5 reuse DDRx10 taps, 8MHz, 1MHz)",
                    )
                } else if (l == 10 || l == 20) && out_rate == 384_000 {
                    (
                        &HTAPS_DDRX10_7TO1_EQ[..],
                        &HTAPS_8MHZ_7TO1_EQ[..],
                        &HTAPS_1MHZ_3TO1_EQ[..],
                        "384k (DDRx10, 8MHz, 1MHz)",
                    )
                } else if l == 5 && out_rate == 96_000 {
                    (
                        &HTAPS_DSDX5_7TO1_EQ[..],
                        &HTAPS_2MHZ_7TO1_EQ[..],
                        &HTAPS_288K_3TO1_EQ[..],
                        "96k (DSD×5, 2MHz, 288k)",
                    )
                } else if l == 10 && out_rate == 192_000 {
                    (
                        &HTAPS_DDRX10_7TO1_EQ[..],
                        &HTAPS_4MHZ_7TO1_EQ[..],
                        &HTAPS_576K_3TO1_EQ[..],
                        "192k (DDRx10, 4MHz, 576k)",
                    )
                } else {
                    if l == 10 {
                        (
                            &HTAPS_DDRX10_7TO1_EQ[..],
                            &HTAPS_4MHZ_7TO1_EQ[..],
                            &HTAPS_576K_3TO1_EQ[..],
                            "192k (DDRx10, 4MHz, 576k)",
                        )
                    } else {
                        (
                            &HTAPS_DDRX5_7TO_1_EQ[..],
                            &HTAPS_4MHZ_7TO1_EQ[..],
                            &HTAPS_576K_3TO1_EQ[..],
                            "192k (DDR×5, 4MHz, 576k)",
                        )
                    }
                };

                if verbose {
                    eprintln!(
                            "[DBG] Equiripple L/M path: L={} M=147 — (×L -> /7 -> /7 -> /3) using {} [Stage2/3 polyphase].",
                            l, label
                        );
                }
                let s1 = Stage1Poly::new(stage1_half, l, 7);
                return Self {
                    // Stage 2 (polyphase decimator by 7)
                    poly2: Some(DecimFIRSym::new_from_half(right2, 7)),
                    // Stage 3 (polyphase decimator by 3)
                    poly3: Some(DecimFIRSym::new_from_half(right3, 3)),
                    two_phase_lm147_384: None,
                    stage1_poly: Some(s1),
                };
            }
            _ => panic!("Unsupported L/M combination: L={} M={}", l, m),
        }
    }

    // Process 8 DSD bits packed in one byte; bit order controlled by lsb_first.
    // Returns number of PCM outputs produced (0..=8, though typically << 8).
    pub fn push_byte_lm<F: FnMut(f64)>(&mut self, byte: u8, lsb_first: bool, mut emit: F) -> usize {
        // Two-phase path: delegate to specialized struct
        if let Some(tp) = self.two_phase_lm147_384.as_mut() {
            return tp.push_byte(byte, lsb_first, emit);
        }
        // Single-phase cascade: Stage1 -> /7 -> /3
        let (Some(ref mut s1), Some(ref mut p2), Some(ref mut p3)) = (
            self.stage1_poly.as_mut(),
            self.poly2.as_mut(),
            self.poly3.as_mut(),
        ) else {
            return 0;
        };
        let mut produced = 0usize;
        if lsb_first {
            for b in 0..8 {
                let bit = (byte >> b) & 1;
                s1.push_all(bit, |y1| {
                    if let Some(y2) = p2.push(y1) {
                        if let Some(y3) = p3.push(y2) {
                            emit(y3);
                            produced += 1;
                        }
                    }
                });
            }
        } else {
            for b in (0..8).rev() {
                let bit = (byte >> b) & 1;
                s1.push_all(bit, |y1| {
                    if let Some(y2) = p2.push(y1) {
                        if let Some(y3) = p3.push(y2) {
                            emit(y3);
                            produced += 1;
                        }
                    }
                });
            }
        }
        produced
    }
}

// Unified two‑phase L∈{10,20} / (21*7) path for DSD64 or DSD128 -> 384 kHz
#[derive(Debug)]
struct TwoPhaseLM147_384 {
    stage1: Stage1Poly,  // ×L /21 polyphase (L=10 or 20)
    stage2: DecimFIRSym, // /7 (2.688 MHz -> 384 kHz)
}

impl TwoPhaseLM147_384 {
    fn new(l: u32) -> Self {
        Self {
            stage1: Stage1Poly::new(&HTAPS_DDRX10_21TO1_EQ, l, 21),
            stage2: DecimFIRSym::new_from_half(&HTAPS_2_68MHZ_7TO1_EQ, 7),
        }
    }

    #[inline]
    fn push_byte<F: FnMut(f64)>(&mut self, byte: u8, lsb_first: bool, mut emit: F) -> usize {
        let mut produced = 0usize;
        if lsb_first {
            for b in 0..8 {
                let bit = (byte >> b) & 1;
                self.stage1.push_all(bit, |y1| {
                    if let Some(y2) = self.stage2.push(y1) {
                        emit(y2);
                        produced += 1;
                    }
                });
            }
        } else {
            for b in (0..8).rev() {
                let bit = (byte >> b) & 1;
                self.stage1.push_all(bit, |y1| {
                    if let Some(y2) = self.stage2.push(y1) {
                        emit(y2);
                        produced += 1;
                    }
                });
            }
        }
        produced
    }
}


#[derive(Debug)]
struct Stage1Poly {
    // Shared
    l: u32,
    m: u32,
    // Float ring removed; use bit-packed signs only
    // Bit-packed ring (one bit per input sample): 1 for +1, 0 for -1
    ring_bits: Vec<u64>,
    bits_mask: usize, // same capacity as ring (power of 2) - 1
    wbits: usize,     // next write bit index
    mode: Stage1Mode,
    // Precomputed byte-LUTs: lut[phase][group][pattern 0..255] => partial sum for 8 taps
    // Phase taps are evaluated newest-first; group 0 covers the newest 8 taps, group 1 the next 8, etc.
    lut: Arc<Vec<Vec<Vec<f64>>>>,
    // Legacy SlotSim fields removed
    delay_high: u64,
    high_index: u64,
    phase1: u32,
    // Accum fields
    acc: u32,
    phase_mod: u32,
    input_count: u64,
    input_delay: u64,
    // Common
    primed: bool,
}

impl Stage1Poly {
    fn new(right_half: &[f64], l: u32, m: u32) -> Self {
        // Full symmetric taps length when mirroring right_half as [rev(right_half), right_half]
        let n = (right_half.len() * 2) as u64;
        let delay_high = (n - 1) / 2; // (N-1)/2
        // Max taps per phase is ceil(n / l)
        let max_len = ((n as usize) + l as usize - 1) / l as usize;
        let cap = max_len.next_power_of_two().max(128);
        let input_delay = (delay_high + (l as u64 - 1)) / l as u64; // used only in Accum
        let lut = get_or_build_stage1_lut(right_half, l);
        let me = Self {
            l,
            m,
            ring_bits: vec![0u64; (cap + 63) / 64],
            bits_mask: cap - 1,
            wbits: 0,
            mode: if m == 21 { Stage1Mode::Accum } else { Stage1Mode::SlotSim },
            delay_high,
            high_index: 0,
            phase1: 0,
            lut,
            acc: 0,
            phase_mod: 0,
            input_count: 0,
            input_delay,
            primed: false,
        };
        me
    }

    #[inline]
    fn set_next_bit(&mut self, is_pos: bool) {
        let word = self.wbits >> 6;
        let bit = self.wbits & 63;
        if is_pos {
            self.ring_bits[word] |= 1u64 << bit;
        } else {
            self.ring_bits[word] &= !(1u64 << bit);
        }
        self.wbits = (self.wbits + 1) & self.bits_mask;
    }

    #[inline]
    fn get_bit_at(&self, idx: usize) -> bool {
        let word = idx >> 6;
        let bit = idx & 63;
        ((self.ring_bits[word] >> bit) & 1) != 0
    }

    // Build a byte from 8 bits going backwards in time from start_idx (inclusive).
    // bit 0 = newest (start_idx), bit j = sample at start_idx - j
    #[inline]
    fn byte_from_bits_rev(&self, mut start_idx: usize) -> u8 {
        let mut b: u8 = 0;
        let capb = self.bits_mask + 1;
        for j in 0..8 {
            if self.get_bit_at(start_idx) {
                b |= 1u8 << j;
            }
            start_idx = (start_idx + capb - 1) & self.bits_mask;
        }
        b
    }


    #[inline]
    fn push_all<F: FnMut(f64)>(&mut self, bit: u8, mut emit: F) {
        match self.mode {
            Stage1Mode::Accum => {
                self.push_accum_all(bit, emit);
            }
            Stage1Mode::SlotSim => {
                // Original slot simulation at high-rate: write once, advance L slots
                self.set_next_bit(bit != 0);
                for _ in 0..self.l {
                    self.sim_high_slot(&mut emit);
                }
            }
        }
    }

    #[inline]
    fn sim_high_slot<F: FnMut(f64)>(&mut self, emit: &mut F) {
        let idx_high = self.high_index;
        self.high_index += 1;
        if !self.primed {
            if idx_high >= self.delay_high {
                self.primed = true;
                self.phase1 = 0;
            }
            return;
        }
        self.phase1 += 1;
        if self.phase1 != self.m {
            return;
        }
        self.phase1 = 0;
        let phase = (idx_high % self.l as u64) as usize;
        // LUT approach over bit-packed ring
        let phase_lut = &self.lut[phase];
        let mut sum = 0.0;
        let mut bidx = (self.wbits + self.bits_mask) & self.bits_mask; // newest bit index (wbits-1)
        let capb = self.bits_mask + 1;
        for group_lut in phase_lut.iter() {
            let byte = self.byte_from_bits_rev(bidx);
            sum += group_lut[byte as usize];
            bidx = (bidx + capb - 8) & self.bits_mask;
        }
        emit(sum);
    }

    // Accumulator mode: emit all outputs scheduled for this input bit
    fn push_accum_all<F: FnMut(f64)>(&mut self, bit: u8, mut emit: F) {
        // Update bitset only (float ring removed)
        self.set_next_bit(bit != 0);
        self.input_count += 1;
        self.acc += self.l;
        // Possibly produce multiple outputs if acc exceeds m multiple times
        while self.acc >= self.m {
            self.acc -= self.m;
            if !self.primed {
                if self.input_count >= self.input_delay {
                    self.primed = true;
                } else {
                    // Advance phase even when not emitting to keep alignment
                    self.phase_mod = (self.phase_mod + (self.m % self.l)) % self.l;
                    continue;
                }
            }
            let phase = self.phase_mod as usize;
            let mut sum = 0.0;
            // Use precomputed LUTs over 8-sample groups using bit-packed ring
            let phase_lut = &self.lut[phase];
            let mut bidx = (self.wbits + self.bits_mask) & self.bits_mask; // newest bit index
            let capb = self.bits_mask + 1;
            for group_lut in phase_lut.iter() {
                let byte = self.byte_from_bits_rev(bidx);
                sum += group_lut[byte as usize];
                bidx = (bidx + capb - 8) & self.bits_mask;
            }
            emit(sum);
            self.phase_mod = (self.phase_mod + (self.m % self.l)) % self.l;
        }
    }
}

// --- ====================================================================================
// ====================================================================================
// Lightweight integer polyphase decimator structure (D=7 or 3)
#[derive(Debug)]
struct DecimFIRSym {
    _full: Vec<f64>, // full symmetric taps
    _len: usize,
    _half: usize,
    _has_center: bool,
    center: usize,         // (len-1)/2
    decim: usize,          // decimation factor D
    phases: Vec<Vec<f64>>, // polyphase components h[p + k*D]
    ring: Vec<f64>,
    mask: usize,
    w: usize,             // next write index
    count: usize,         // total samples seen
    _left_half: Vec<f64>, // first half taps (outer->inner)
}

impl DecimFIRSym {
    fn new_from_half(right_half: &[f64], decim: usize) -> Self {
        // Reconstruct full taps (mirror right_half)
        let mut full: Vec<f64> = right_half.iter().rev().cloned().collect();
        full.extend_from_slice(right_half);
        let len = full.len();
        let center = (len - 1) / 2;
        let has_center = len % 2 == 1;
        let half = if has_center { (len - 1) / 2 } else { len / 2 };
        // Ring capacity: next power of two >= len + decim (margin)
        let cap = (len + decim).next_power_of_two();
        let left_half = full[..half].to_vec();
        // Build polyphase decomposition: phases[p][k] = h[p + k*D]
        let mut phases: Vec<Vec<f64>> = vec![Vec::new(); decim];
        for (i, &c) in full.iter().enumerate() {
            phases[i % decim].push(c);
        }
        Self {
            _full: full,
            _len: len,
            _half: half,
            _has_center: has_center,
            center,
            decim,
            phases,
            ring: vec![0.0; cap],
            mask: cap - 1,
            w: 0,
            count: 0,
            _left_half: left_half,
        }
    }

    #[inline]
    fn push(&mut self, x: f64) -> Option<f64> {
        // Write newest sample
        self.ring[self.w] = x;
        self.w = (self.w + 1) & self.mask;
        let t = self.count;
        self.count += 1;

        // Need at least center samples of history
        if t < self.center {
            return None;
        }
        // Output only when (t - center) aligned to decimation grid
        if (t - self.center) % self.decim != 0 {
            return None;
        }

        let acc = unsafe { self.convolve_polyphase() };
        Some(acc)
    }

    // ----- Convolution implementations -----
    #[inline(always)]
    unsafe fn convolve_polyphase(&self) -> f64 {
        // Polyphase evaluation: y = Σ_p Σ_k h[p + kD] * x[t - (p + kD)]
        // We walk each phase p starting from newest - p and stride by D.
        let newest = (self.w + self.ring.len() - 1) & self.mask; // index of x[t]
        let mut total = 0.0;
        for (p, phase_taps) in self.phases.iter().enumerate() {
            let mut idx = (newest + self.ring.len() - p) & self.mask; // x[t - p]
            for &c in phase_taps {
                total += c * self.ring[idx];
                idx = (idx + self.ring.len() - self.decim) & self.mask; // move back D samples
            }
        }
        total
    }
}
