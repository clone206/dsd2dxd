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
    // Stage 1 half taps (right half including center if odd)
    stage1_half: &'static [f64],
    up_factor: u32, // L
    decim1: u32,    // 14 (M=294) or 7 (M=147)
    // Stage 2 (polyphase decimator by 7) optional (None for two-phase path)
    poly2: Option<DecimFIRSym>,
    // Stage 3 (polyphase decimator by 3) optional (None for two-phase path)
    poly3: Option<DecimFIRSym>,
    // If Some(d), indicates we enabled the (logical) Stage1 polyphase optimization and
    // stores the Stage1 effective input-sample delay (ceil(group_delay_high / L)).
    two_phase_lm147_384: Option<TwoPhaseLM147_384>,
    // Unified Stage1 polyphase (replaces former Slow + Generic variants)
    stage1_poly: Option<Stage1Poly>,
    // Verbose flag retained for later diagnostics
}
impl LMResampler {
    pub fn new(l: u32, m: i32, verbose: bool, out_rate: u32) -> Self {
        match m {
            294 => {
                // Original cascade Stage1 definitions
                let s = Self {
                    stage1_half: &HTAPS_DDRX5_14TO1_EQ,
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
                        stage1_half: &HTAPS_DDRX10_21TO1_EQ,
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
                    stage1_half,
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
}

// Unified two‑phase L∈{10,20} / (21*7) path for DSD64 or DSD128 -> 384 kHz
#[derive(Debug)]
struct TwoPhaseLM147_384 {
    stage1: Stage1PolyM21, // ×L /21 polyphase (L=10 or 20)
    stage2: DecimFIRSym,   // /7 (2.688 MHz -> 384 kHz)
}

impl TwoPhaseLM147_384 {
    fn new(l: u32) -> Self {
        debug_assert!(
            l == 10 || l == 20,
            "TwoPhaseLM147_384 only supports L=10 or L=20"
        );
        Self {
            stage1: Stage1PolyM21::new(l, &HTAPS_DDRX10_21TO1_EQ),
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

// Unified Stage1 polyphase for L ∈ {10,20} with m=21
#[derive(Debug)]
struct Stage1PolyM21 {
    phases: Vec<Vec<f64>>,
    l: u32,         // 10 or 20
    m: u32,         // 21
    ring: Vec<f64>, // input sample history
    mask: usize,
    w: usize,
    acc: u32,
    phase_mod: u32,
    input_count: u64,
    input_delay: u64,
    primed: bool,
}

impl Stage1PolyM21 {
    fn new(l: u32, right_half: &[f64]) -> Self {
        let m = 21u32;
        // Rebuild full symmetric high‑rate taps
        let mut full: Vec<f64> = right_half.iter().rev().cloned().collect();
        full.extend_from_slice(right_half);
        let n = full.len();
        // Build phases
        let mut phases = vec![Vec::new(); l as usize];
        for (i, &c) in full.iter().enumerate() {
            phases[i % l as usize].push(c);
        }
        // Group delay high-rate -> input samples (ceil)
        let max_len = phases.iter().map(|p| p.len()).max().unwrap_or(0);
        let gd_high = (n as u64 - 1) / 2;
        let input_delay = (gd_high + (l as u64 - 1)) / l as u64;
        let cap = max_len.next_power_of_two().max(128);
        Self {
            phases,
            l,
            m,
            ring: vec![0.0; cap],
            mask: cap - 1,
            w: 0,
            acc: 0,
            phase_mod: 0,
            input_count: 0,
            input_delay,
            primed: false,
        }
    }

    #[inline]
    fn push_bit(&mut self, bit: u8) -> Option<f64> {
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

        // Convolution taps[k] * x[n-k]
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
    full: Vec<f64>, // full symmetric taps
    len: usize,
    half: usize,
    has_center: bool,
    center: usize, // (len-1)/2
    decim: usize,  // decimation factor D
    ring: Vec<f64>,
    mask: usize,
    w: usize,            // next write index
    count: usize,        // total samples seen
    left_half: Vec<f64>, // first half taps (outer->inner)
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
        Self {
            full,
            len,
            half,
            has_center,
            center,
            decim,
            ring: vec![0.0; cap],
            mask: cap - 1,
            w: 0,
            count: 0,
            left_half,
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

        let acc = unsafe { self.convolve_scalar() };
        Some(acc)
    }

    // ----- Convolution implementations -----
    #[inline(always)]
    unsafe fn convolve_scalar(&self) -> f64 {
        let newest = (self.w + self.ring.len() - 1) & self.mask;
        let mut acc = 0.0;
        for i in 0..self.half {
            let tap = self.left_half[i];
            let left_idx = (newest + self.ring.len() - (self.len - 1 - i)) & self.mask;
            let right_idx = (newest + self.ring.len() - i) & self.mask;
            acc += tap * (self.ring[left_idx] + self.ring[right_idx]);
        }
        if self.has_center {
            let center_idx = (newest + self.ring.len() - (self.len - 1 - self.half)) & self.mask;
            acc += self.full[self.half] * self.ring[center_idx];
        }
        acc
    }
}

// (Legacy thread-local cache for slow Stage1 polyphase removed after unification.)

// Unified Stage1 polyphase (merging former Slow + Generic variants)
#[derive(Debug)]
struct Stage1Poly {
    phases: Vec<Vec<f64>>, // phases[r][k] = h[r + k*L]
    l: u32,
    m: u32,
    ring: Vec<f64>,
    mask: usize,
    w: usize,
    high_index: u64,
    delay_high: u64,
    phase1: u32,
    primed: bool,
    input_count: u64,
}

impl Stage1Poly {
    // Accept half taps (right half including center if odd) and reconstruct full symmetric tap set.
    fn new(right_half: &[f64], l: u32, m: u32) -> Self {
        // Rebuild full symmetric taps: mirror of right_half (excluding implicit center duplication handled by simple reverse + extend)
        let mut full: Vec<f64> = right_half.iter().rev().cloned().collect();
        full.extend_from_slice(right_half);
        let mut phases = vec![Vec::new(); l as usize];
        for (i, &c) in full.iter().enumerate() {
            phases[i % l as usize].push(c);
        }
        let n = full.len() as u64;
        let delay_high = (n - 1) / 2;
        let max_len = phases.iter().map(|p| p.len()).max().unwrap_or(0);
        let cap = max_len.next_power_of_two().max(64);
        Self {
            phases,
            l,
            m,
            ring: vec![0.0; cap],
            mask: cap - 1,
            w: 0,
            high_index: 0,
            delay_high,
            phase1: 0,
            primed: false,
            input_count: 0,
        }
    }
    #[inline]
    fn push(&mut self, bit: u8) -> Option<f64> {
        let mut first: Option<f64> = None;
        self.push_all(bit, |y| {
            if first.is_none() {
                first = Some(y)
            }
        });
        first
    }
    #[inline]
    fn push_all<F: FnMut(f64)>(&mut self, bit: u8, mut emit: F) {
        let s = if bit != 0 { 1.0 } else { -1.0 };
        self.ring[self.w] = s;
        self.w = (self.w + 1) & self.mask;
        self.input_count += 1;
        for _ in 0..self.l {
            let idx_high = self.high_index;
            self.high_index += 1;
            if !self.primed {
                if idx_high >= self.delay_high {
                    self.primed = true;
                    self.phase1 = 0;
                }
                continue;
            }
            self.phase1 += 1;
            if self.phase1 != self.m {
                continue;
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
    }
}
