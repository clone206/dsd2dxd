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
    up_factor: u32, // L
    decim1: u32,    // 14 (M=294) or 7 (M=147)
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
                let s = Self {
                    up_factor: l,
                    decim1: 14,
                    // Stage 2 (polyphase decimator by 7)
                    poly2: Some(DecimFIRSym::new_from_half(&HTAPS_2MHZ_7TO1_EQ, 7)),
                    // Stage 3 (polyphase decimator by 3)
                    poly3: Some(DecimFIRSym::new_from_half(&HTAPS_288K_3TO1_EQ, 3)),
                    two_phase_lm147_384: None,
                    stage1_poly: Some(Stage1Poly::new(&HTAPS_DDRX5_14TO1_EQ, l, 14)),
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
                        up_factor: l,
                        decim1: 21,
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
                } else {
                    (
                        &HTAPS_DDRX5_7TO_1_EQ[..],
                        &HTAPS_4MHZ_7TO1_EQ[..],
                        &HTAPS_576K_3TO1_EQ[..],
                        "192k (DDR×5, 4MHz, 576k)",
                    )
                };

                if verbose {
                    eprintln!(
                            "[DBG] Equiripple L/M path: L={} M=147 — (×L -> /7 -> /7 -> /3) using {} [Stage2/3 polyphase].",
                            l, label
                        );
                }
                return Self {
                    up_factor: l,
                    decim1: 7,
                    // Stage 2 (polyphase decimator by 7)
                    poly2: Some(DecimFIRSym::new_from_half(right2, 7)),
                    // Stage 3 (polyphase decimator by 3)
                    poly3: Some(DecimFIRSym::new_from_half(right3, 3)),
                    two_phase_lm147_384: None,
                    stage1_poly: Some(Stage1Poly::new(stage1_half, l, 7)),
                };
            }
            _ => panic!("Unsupported L/M combination: L={} M={}", l, m),
        }
    }

    // Unified push. Breaks early once an output is produced.
    #[inline]
    pub fn push_bit_lm(&mut self, bit: u8) -> Option<f64> {
        if let Some(tp) = self.two_phase_lm147_384.as_mut() {
            return tp.push_bit(bit);
        }
        // Stage1Poly now always eagerly constructed in constructor for non two-phase paths.
        let poly = self.stage1_poly.as_mut().unwrap();
        let mut final_out: Option<f64> = None;
        // If L >= decim1 we may produce multiple Stage1 outputs per input sample.
        if let (Some(ref mut p2), Some(ref mut p3)) = (self.poly2.as_mut(), self.poly3.as_mut()) {
            if self.up_factor as u32 >= self.decim1 {
                poly.push_all(bit, |y1| {
                    if let Some(y2) = p2.push(y1) {
                        if let Some(y3) = p3.push(y2) {
                            final_out = Some(y3); // keep last output
                        }
                    }
                });
            } else if let Some(y1) = poly.push(bit) {
                if let Some(y2) = p2.push(y1) {
                    if let Some(y3) = p3.push(y2) {
                        final_out = Some(y3);
                    }
                }
            }
        } else {
            // Two-phase path uses separate structure; should not reach here with stage1_poly set
            return None;
        }
        final_out
    }

    // Process 8 DSD bits packed in one byte; bit order controlled by lsb_first.
    // Returns number of PCM outputs produced (0..=8, though typically << 8).
    #[allow(dead_code)]
    pub fn push_byte_lm<F: FnMut(f64)>(&mut self, byte: u8, lsb_first: bool, mut emit: F) -> usize {
        let mut produced = 0usize;
        if lsb_first {
            for b in 0..8 {
                self.maybe_emit((byte >> b) & 1, &mut produced, &mut emit);
            }
        } else {
            for b in (0..8).rev() {
                self.maybe_emit((byte >> b) & 1, &mut produced, &mut emit);
            }
        }
        produced
    }

    #[inline]
    fn maybe_emit<F: FnMut(f64)>(&mut self, bit: u8, produced: &mut usize, emit: &mut F) {
        if let Some(y) = self.push_bit_lm(bit) {
            *produced += 1;
            emit(y);
        }
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
    fn push_bit(&mut self, bit: u8) -> Option<f64> {
        if let Some(y1) = self.stage1.push_bit(bit) {
            if let Some(y2) = self.stage2.push(y1) {
                return Some(y2);
            }
        }
        None
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stage1Mode {
    SlotSim,
    Accum,
}

#[derive(Debug)]
struct Stage1Poly {
    // Shared
    phases: Vec<Vec<f64>>,
    l: u32,
    m: u32,
    ring: Vec<f64>,
    mask: usize,
    w: usize,
    mode: Stage1Mode,
    // SlotSim fields
    high_index: u64,
    delay_high: u64,
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
        // Rebuild full symmetric taps from half-side
        let mut full: Vec<f64> = right_half.iter().rev().cloned().collect();
        full.extend_from_slice(right_half);
        let mut phases = vec![Vec::new(); l as usize];
        for (i, &c) in full.iter().enumerate() {
            phases[i % l as usize].push(c);
        }
        let n = full.len() as u64;
        let delay_high = (n - 1) / 2; // (N-1)/2
        let max_len = phases.iter().map(|p| p.len()).max().unwrap_or(0);
        let cap = max_len.next_power_of_two().max(128);
        // Choose mode: keep accumulator only for m==21 (where we previously validated)
        let mode = if m == 21 {
            Stage1Mode::Accum
        } else {
            Stage1Mode::SlotSim
        };
        let input_delay = (delay_high + (l as u64 - 1)) / l as u64; // used only in Accum
        Self {
            phases,
            l,
            m,
            ring: vec![0.0; cap],
            mask: cap - 1,
            w: 0,
            mode,
            high_index: 0,
            delay_high,
            phase1: 0,
            acc: 0,
            phase_mod: 0,
            input_count: 0,
            input_delay,
            primed: false,
        }
    }

    #[inline]
    fn push_bit(&mut self, bit: u8) -> Option<f64> {
        self.push(bit)
    }

    #[inline]
    fn push_all<F: FnMut(f64)>(&mut self, bit: u8, mut emit: F) {
        match self.mode {
            Stage1Mode::Accum => {
                if let Some(y) = self.push(bit) {
                    emit(y);
                }
            }
            Stage1Mode::SlotSim => {
                // Original slot simulation: write input once, simulate L high-rate slots
                let s = if bit != 0 { 1.0 } else { -1.0 };
                self.ring[self.w] = s;
                self.w = (self.w + 1) & self.mask;
                for _ in 0..self.l {
                    self.sim_high_slot(&mut emit);
                }
            }
        }
    }

    #[inline]
    fn push(&mut self, bit: u8) -> Option<f64> {
        match self.mode {
            Stage1Mode::Accum => self.push_accum(bit),
            Stage1Mode::SlotSim => {
                let mut first = None;
                self.push_all(bit, |y| {
                    if first.is_none() {
                        first = Some(y);
                    }
                });
                first
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
        let taps = &self.phases[phase];
        let mut sum = 0.0;
        let mut sample_idx = (self.w + self.ring.len() - 1) & self.mask;
        for &c in taps {
            sum += c * self.ring[sample_idx];
            sample_idx = (sample_idx + self.ring.len() - 1) & self.mask;
        }
        emit(sum);
    }

    #[inline]
    fn push_accum(&mut self, bit: u8) -> Option<f64> {
        let s = if bit != 0 { 1.0 } else { -1.0 };
        self.ring[self.w] = s;
        self.w = (self.w + 1) & self.mask;
        self.input_count += 1;
        self.acc += self.l;
        if self.acc < self.m {
            return None;
        }
        self.acc -= self.m;
        if !self.primed {
            if self.input_count >= self.input_delay {
                self.primed = true;
            } else {
                self.phase_mod = (self.phase_mod + (self.m % self.l)) % self.l;
                return None;
            }
        }
        let phase = self.phase_mod as usize;
        let taps = &self.phases[phase];
        let mut acc_sum = 0.0;
        let mut idx = (self.w + self.ring.len() - 1) & self.mask;
        for &c in taps {
            acc_sum += c * self.ring[idx];
            idx = (idx + self.ring.len() - 1) & self.mask;
        }
        self.phase_mod = (self.phase_mod + (self.m % self.l)) % self.l;
        Some(acc_sum)
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
