use crate::filters::HTAPS_1MHZ_3TO1_EQ;
use crate::filters::HTAPS_288K_3TO1_EQ;
use crate::filters::HTAPS_2MHZ_7TO1_EQ; 
use crate::filters::HTAPS_4MHZ_7TO1_EQ; 
use crate::filters::HTAPS_576K_3TO1_EQ;
use crate::filters::HTAPS_8MHZ_7TO1_EQ;
use crate::filters::HTAPS_DDRX10_21TO1_EQ;
use crate::filters::HTAPS_DDRX10_7TO1_EQ;
use crate::filters::HTAPS_DSDX5_7TO1_EQ;
use crate::fir_convolve::FirConvolve;
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
    // Stage 1
    fir1: FirConvolve,
    up_factor: u32, // L
    decim1: u32,    // 14 (M=294) or 7 (M=147)
    delay1: u64,

    // Stage 2 (polyphase decimator by 7)
    poly2: DecimFIRSym,
    decim2: u32, // 7 (kept for latency formula)
    // Stage 3 (polyphase decimator by 3)
    poly3: DecimFIRSym,
    decim3: u32, // 3
    // If Some(d), indicates we enabled the (logical) Stage1 polyphase optimization and
    // stores the Stage1 effective input-sample delay (ceil(group_delay_high / L)).
    stage1_poly_input_delay: Option<u64>,
    two_phase_lm147_384: Option<TwoPhaseLM147_384>,
    // Minimal Stage‑1 slow‑domain polyphase (diagnostic) – optional via constant toggle.
    stage1_poly_slow: Option<Stage1PolySlow>,
    // Generic Stage1 polyphase (for L != 5 fallback instead of legacy zero stuffing)
    stage1_poly_generic: Option<Stage1PolyGeneric>,
    // Verbose flag retained for later diagnostics
    verbose: bool,
}
impl LMResampler {
    pub fn new(l: u32, m: i32, verbose: bool, first_channel: bool, out_rate: u32) -> Self {
        match m {
            294 => {
                // Original cascade Stage1 definitions
                let fir1 = FirConvolve::new(&HTAPS_DDRX5_14TO1_EQ);
                let full1 = (HTAPS_DDRX5_14TO1_EQ.len() * 2) as u64;
                let delay1 = (full1 - 1) / 2;
                let poly2 = DecimFIRSym::new_from_half(&HTAPS_2MHZ_7TO1_EQ, 7);
                let poly3 = DecimFIRSym::new_from_half(&HTAPS_288K_3TO1_EQ, 3);
                let use_stage1_poly = !first_channel;
                let stage1_poly_input_delay = if use_stage1_poly {
                    // group delay high-rate = delay1; convert to input samples (ceil / L)
                    let input_delay = (delay1 + (l as u64 - 1)) / l as u64;
                    if verbose {
                        eprintln!(
                            "[DBG] Stage1 polyphase (logical) enabled (L={} decim1=14) input_delay_in={}",
                            l, input_delay
                        );
                    }
                    Some(input_delay)
                } else {
                    None
                };

                let s = Self {
                    fir1,
                    up_factor: l,
                    decim1: 14,
                    delay1,

                    // Stage 2 (polyphase decimator by 7)
                    poly2,
                    decim2: 7,
                    // Stage 3 (polyphase decimator by 3)
                    poly3,
                    decim3: 3,
                    stage1_poly_input_delay,
                    two_phase_lm147_384: None,
                    stage1_poly_slow: None,
                    stage1_poly_generic: None,
                    verbose,
                };
                if verbose {
                    if first_channel {
                        eprintln!(
                            "[DBG] Equiripple L/M path: L={} M=294 — STAGE1 DUMP (×L -> /14).",
                            l
                        );
                    } else {
                        eprintln!(
                            "[DBG] Equiripple L/M path: L={} M=294 — (×L -> /14 -> /7 -> /3) [Stage2/3 polyphase].",
                            l
                        );
                    }
                }
                s
            }
            147 => {
                // DSD128 -> 384k two‑phase path (×10 -> /21 -> /7) DEFAULT for L=10
                if l == 10 && out_rate == 384_000 {
                    if verbose {
                        eprintln!("[DBG] Two-phase L=10/M=147 path enabled: (×10 -> /21 (poly) -> /7) => 384k");
                    }
                    let tp = TwoPhaseLM147_384::new(l);
                    // Minimal placeholders (not used directly)
                    let fir1 = FirConvolve::new(&HTAPS_DDRX10_21TO1_EQ);
                    let dummy2 = DecimFIRSym::new_from_half(&HTAPS_2_68MHZ_7TO1_EQ, 7);
                    let dummy3 = DecimFIRSym::new_from_half(&HTAPS_2_68MHZ_7TO1_EQ, 7);
                    return Self {
                        fir1,
                        up_factor: l,
                        decim1: 21,
                        delay1: 0,

                        poly2: dummy2,
                        decim2: 7,
                        poly3: dummy3,
                        decim3: 3,
                        stage1_poly_input_delay: None,
                        two_phase_lm147_384: Some(tp),
                        stage1_poly_slow: None,
                        stage1_poly_generic: None,
                        verbose,
                    };
                }

                // Existing L=20 two‑phase 384k branch remains below
                if l == 20 && out_rate == 384_000 {
                    if verbose {
                        eprintln!("[DBG] Two-phase L=20/M=147 path enabled: (×20 -> /21 (poly) -> /7) => 384k");
                    }
                    let tp = TwoPhaseLM147_384::new(l);
                    // Placeholder FIR & decims (unused in this mode beyond latency scaffolding)
                    let fir1 = FirConvolve::new(&HTAPS_DDRX10_21TO1_EQ);
                    let dummy2 = DecimFIRSym::new_from_half(&HTAPS_2_68MHZ_7TO1_EQ, 7);
                    let dummy3 = DecimFIRSym::new_from_half(&HTAPS_2_68MHZ_7TO1_EQ, 7);
                    return Self {
                        fir1,
                        up_factor: l,
                        decim1: 21,
                        delay1: 0,

                        poly2: dummy2,
                        decim2: 7,
                        poly3: dummy3,
                        decim3: 3,
                        stage1_poly_input_delay: None,
                        two_phase_lm147_384: Some(tp),
                        stage1_poly_slow: None,
                        stage1_poly_generic: None,
                        verbose,
                    };
                }

                // (Existing selection logic unchanged, just swap Stage2/3 construction)
                let (fir1, full1, right2, _full2, right3, _full3, label) =
                    if l == 5 && out_rate == 384_000 {
                        // DSD256 -> 384k path (L=5, M=147) reuses SAME Stage1 half taps as other 384k paths (DDRx10_7TO1) per request.
                        // Second and third stage identical to other 384k (8MHz -> /7, 1MHz -> /3).
                        (
                            FirConvolve::new(&HTAPS_DDRX10_7TO1_EQ),
                            (HTAPS_DDRX10_7TO1_EQ.len() * 2) as u64,
                            &HTAPS_8MHZ_7TO1_EQ[..],
                            (HTAPS_8MHZ_7TO1_EQ.len() * 2) as u64,
                            &HTAPS_1MHZ_3TO1_EQ[..],
                            (HTAPS_1MHZ_3TO1_EQ.len() * 2) as u64,
                            "384k (L=5 reuse DDRx10 taps, 8MHz, 1MHz)",
                        )
                    } else if (l == 10 || l == 20) && out_rate == 384_000 {
                        (
                            FirConvolve::new(&HTAPS_DDRX10_7TO1_EQ),
                            (HTAPS_DDRX10_7TO1_EQ.len() * 2) as u64,
                            &HTAPS_8MHZ_7TO1_EQ[..],
                            (HTAPS_8MHZ_7TO1_EQ.len() * 2) as u64,
                            &HTAPS_1MHZ_3TO1_EQ[..],
                            (HTAPS_1MHZ_3TO1_EQ.len() * 2) as u64,
                            "384k (DDRx10, 8MHz, 1MHz)",
                        )
                    } else if l == 5 && out_rate == 96_000 {
                        (
                            FirConvolve::new(&HTAPS_DSDX5_7TO1_EQ),
                            (HTAPS_DSDX5_7TO1_EQ.len() * 2) as u64,
                            &HTAPS_2MHZ_7TO1_EQ[..],
                            (HTAPS_2MHZ_7TO1_EQ.len() * 2) as u64,
                            &HTAPS_288K_3TO1_EQ[..],
                            (HTAPS_288K_3TO1_EQ.len() * 2) as u64,
                            "96k (DSD×5, 2MHz, 288k)",
                        )
                    } else {
                        (
                            FirConvolve::new(&HTAPS_DDRX5_7TO_1_EQ),
                            (HTAPS_DDRX5_7TO_1_EQ.len() * 2) as u64,
                            &HTAPS_4MHZ_7TO1_EQ[..],
                            (HTAPS_4MHZ_7TO1_EQ.len() * 2) as u64,
                            &HTAPS_576K_3TO1_EQ[..],
                            (HTAPS_576K_3TO1_EQ.len() * 2) as u64,
                            "192k (DDR×5, 4MHz, 576k)",
                        )
                    };
                let _stage1_half: &[f64] = if l == 5 && out_rate == 384_000 {
                    &HTAPS_DDRX10_7TO1_EQ
                } else if (l == 10 || l == 20) && out_rate == 384_000 {
                    &HTAPS_DDRX10_7TO1_EQ
                } else if l == 5 && out_rate == 96_000 {
                    &HTAPS_DSDX5_7TO1_EQ
                } else {
                    &HTAPS_DDRX5_7TO_1_EQ
                };
                let delay1 = (full1 - 1) / 2;
                let poly2 = DecimFIRSym::new_from_half(right2, 7);
                let poly3 = DecimFIRSym::new_from_half(right3, 3);
                let use_stage1_poly = !first_channel;
                let stage1_poly_input_delay = if use_stage1_poly {
                    let input_delay = (delay1 + (l as u64 - 1)) / l as u64;
                    if verbose {
                        eprintln!(
                            "[DBG] Stage1 polyphase (logical) enabled (L={} decim1=7) input_delay_in={}",
                            l, input_delay
                        );
                    }
                    Some(input_delay)
                } else {
                    None
                };

                let s = Self {
                    fir1,
                    up_factor: l,
                    decim1: 7,
                    delay1,

                    // Stage 2 (polyphase decimator by 7)
                    poly2,
                    decim2: 7,
                    // Stage 3 (polyphase decimator by 3)
                    poly3,
                    decim3: 3,
                    stage1_poly_input_delay,
                    two_phase_lm147_384: None,
                    stage1_poly_slow: None,
                    stage1_poly_generic: None,
                    verbose,
                };
                if verbose {
                    if first_channel {
                        eprintln!(
                            "[DBG] Equiripple L/M path: L={} M=147 — STAGE1 DUMP (×L -> /7).",
                            l
                        );
                    } else {
                        eprintln!(
                            "[DBG] Equiripple L/M path: L={} M=147 — (×L -> /7 -> /7 -> /3) using {} [Stage2/3 polyphase].",
                            l, label
                        );
                    }
                }
                s
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
        // FULL Stage1 polyphase for L=5  (now supports multi-output per input internally).
        // Restrict experimental slow Stage1 polyphase to L=5 only (L=20 needs its own tap design).
        // Stage1 slow polyphase always active for L=5 paths.
        if self.up_factor == 5 {
            if self.stage1_poly_slow.is_none() {
                let half = self.fir1.full_taps.len() / 2;
                let right_half = &self.fir1.full_taps[half..];
                self.stage1_poly_slow = Some(Stage1PolySlow::new(
                    right_half,
                    self.up_factor,
                    self.decim1,
                ));
                if self.verbose {
                    let total_m = (self.decim1 as u64) * (self.decim2 as u64) * (self.decim3 as u64);
                    eprintln!(
                        "[DBG] Stage1PolySlow active: L={} first_stage=/{} total_decim={} ({} -> final).",
                        self.up_factor,
                        self.decim1,
                        total_m,
                        if total_m == 294 { "×5->/14->/7->/3" } else { "×5->/7->/7->/3" }
                    );
                }
            }
            let poly = self.stage1_poly_slow.as_mut().unwrap();
            if let Some(y1) = poly.push(bit) {
                if let Some(y2) = self.poly2.push(y1) {
                    if let Some(y3) = self.poly3.push(y2) {
                        return Some(y3);
                    }
                }
            }
            return None;
        }

        // Generic Stage1 polyphase (replaces legacy zero-stuff path).
        if self.stage1_poly_generic.is_none() {
            self.stage1_poly_generic = Some(Stage1PolyGeneric::new(
                &self.fir1.full_taps,
                self.up_factor as u32,
                self.decim1 as u32,
            ));
        }
        let poly = self.stage1_poly_generic.as_mut().unwrap();
        let mut final_out: Option<f64> = None;
        // If L >= decim1 we may produce multiple Stage1 outputs per input sample.
        if self.up_factor as u32 >= self.decim1 {
            poly.push_all(bit, |y1| {
                if let Some(y2) = self.poly2.push(y1) {
                    if let Some(y3) = self.poly3.push(y2) {
                        final_out = Some(y3); // keep last (chronological) output
                    }
                }
            });
        } else {
            if let Some(y1) = poly.push(bit) {
                if let Some(y2) = self.poly2.push(y1) {
                    if let Some(y3) = self.poly3.push(y2) {
                        final_out = Some(y3);
                    }
                }
            }
        }
        final_out
    }

    #[inline]
    pub fn output_latency_frames(&self) -> f64 {
        if let Some(tp) = self.two_phase_lm147_384.as_ref() {
            return tp.output_latency_frames();
        }
        // If logical Stage1 polyphase enabled, use its input-sample delay; else use raw high-rate delay1.
        let stage1_delay_out = if let Some(d_in) = self.stage1_poly_input_delay {
            (d_in as f64) * (self.up_factor as f64)
                / (self.decim1 as f64 * self.decim2 as f64 * self.decim3 as f64)
        } else {
            (self.delay1 as f64) / (self.decim1 as f64 * self.decim2 as f64 * self.decim3 as f64)
        };
        let l1 = stage1_delay_out;
        let l2 = (self.poly2.center_delay() as f64) / (self.decim2 as f64 * self.decim3 as f64);
        let l3 = (self.poly3.center_delay() as f64) / (self.decim3 as f64);
        l1 + l2 + l3
    }
}


// Unified two‑phase L∈{10,20} / (21*7) path for DSD64 or DSD128 -> 384 kHz
#[derive(Debug)]
struct TwoPhaseLM147_384 {
    stage1: Stage1PolyL21, // ×L /21 polyphase (L=10 or 20)
    stage2: DecimFIRSym,   // /7 (2.688 MHz -> 384 kHz)
    l: u32,
}

impl TwoPhaseLM147_384 {
    fn new(l: u32) -> Self {
        debug_assert!(l == 10 || l == 20, "TwoPhaseLM147_384 only supports L=10 or L=20");
        Self {
            stage1: Stage1PolyL21::new(l, &HTAPS_DDRX10_21TO1_EQ),
            stage2: DecimFIRSym::new_from_half(&HTAPS_2_68MHZ_7TO1_EQ, 7),
            l,
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
    #[inline]
    fn output_latency_frames(&self) -> f64 {
        // Stage1 latency (input samples) scaled by L / (21*7) plus stage2 latency (scaled by /7)
        let d1 = (self.stage1.input_delay() as f64) * (self.l as f64 / (21.0 * 7.0));
        let d2 = (self.stage2.center_delay() as f64) / 7.0;
        d1 + d2
    }
}

// Unified Stage1 polyphase for L ∈ {10,20} with m=21
#[derive(Debug)]
struct Stage1PolyL21 {
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

impl Stage1PolyL21 {
    fn new(l: u32, right_half: &[f64]) -> Self {
        debug_assert!(l == 10 || l == 20, "Stage1PolyL21 only supports L=10 or L=20");
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

    #[inline]
    fn input_delay(&self) -> u64 { self.input_delay }
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

    #[inline]
    fn center_delay(&self) -> usize {
        self.center
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

use std::cell::RefCell;
use std::collections::HashMap;
use std::vec;
// (VecDeque import removed; not needed)

// Thread-local cache for slow-domain Stage1 polyphase states (keyed by EquiLMResampler pointer)
thread_local! {
    static STAGE1_SLOW_POLY: RefCell<HashMap<usize, Stage1PolySlow>> = RefCell::new(HashMap::new());
}

// Minimal slow-domain Stage1 polyphase applying noble identities (×L FIR -> /decim1) without zero stuffing.
// Built lazily from the existing high-rate FIR taps (fir1.full_taps) and used only inside push_bit_lm.
#[derive(Debug)]
struct Stage1PolySlow {
    phases: Vec<Vec<f64>>, // phases[r][k] = h[r + k*L], k increasing in time (older)
    l: u32,                // up factor (5)
    d1: u32,               // first decimation (7 or 14)
    ring: Vec<f64>,
    mask: usize,
    w: usize,
    high_index: u64, // high-rate sample counter (after upsample expansion)
    delay_high: u64, // (N-1)/2 group delay in high-rate samples
    phase1: u32,     // decimation phase counter (0..d1-1)
    primed: bool,
    input_count: u64,
}

impl Stage1PolySlow {
    fn new(right_half: &[f64], l: u32, d1: u32) -> Self {
        // Reconstruct full symmetric taps from right half (like other polyphase constructors)
        let mut full: Vec<f64> = right_half.iter().rev().cloned().collect();
        full.extend_from_slice(right_half);

        // Build phases in natural time order (k increasing)
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
            d1,
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
        // Write one real input sample (±1.0)
        let s = if bit != 0 { 1.0 } else { -1.0 };
        self.ring[self.w] = s;
        self.w = (self.w + 1) & self.mask;
        self.input_count += 1;

        let mut emitted: Option<f64> = None;

        // Simulate all L high-rate slots (p = 0..L-1). Only p=0 is non-zero, but we must
        // still advance high_index through every slot to mirror legacy scheduling.
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
            if self.phase1 != self.d1 {
                continue;
            }
            self.phase1 = 0;

            // Only compute once (L < d1 ensures <=1 output per input). Still finish loop
            // so remaining high-rate slots advance high_index.
            if emitted.is_none() {
                let phase = (idx_high % self.l as u64) as usize;
                let taps = &self.phases[phase];

                let mut sum = 0.0;
                let mut sample_idx = (self.w + self.ring.len() - 1) & self.mask;
                for &c in taps {
                    sum += c * self.ring[sample_idx];
                    sample_idx = (sample_idx + self.ring.len() - 1) & self.mask;
                }
                emitted = Some(sum);
            }
            // (Do NOT break; continue to advance remaining high-rate slots.)
        }


        emitted
    }
}

// Generic Stage1 polyphase variant used for legacy replacement (supports any L, d1 where L < some multiple of d1).
// Strategy: identical scheduling model to Stage1PolySlow (simulate all L high-rate slots per real input)
// but allows different (l, d1) pairs (e.g., L=10,d1=21) and supports at most one output per input when L < d1.
// If in future L >= d1 cases are needed, extend to push multiple outputs per input (similar to Stage1PolyL10_21 design).
#[derive(Debug)]
struct Stage1PolyGeneric {
    phases: Vec<Vec<f64>>, // phases[r][k] = h[r + k*L]
    l: u32,
    d1: u32,
    ring: Vec<f64>,
    mask: usize,
    w: usize,
    high_index: u64,
    delay_high: u64,
    phase1: u32,
    primed: bool,
    input_count: u64,
}

impl Stage1PolyGeneric {
    fn new(full_taps: &[f64], l: u32, d1: u32) -> Self {
        // Build phase lists
        let mut phases = vec![Vec::new(); l as usize];
        for (i, &c) in full_taps.iter().enumerate() {
            phases[i % l as usize].push(c);
        }
        let n = full_taps.len() as u64;
        let delay_high = (n - 1) / 2;
        let max_len = phases.iter().map(|p| p.len()).max().unwrap_or(0);
        let cap = max_len.next_power_of_two().max(64);
        Self {
            phases,
            l,
            d1,
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

    // Simple single-output helper (used when L < d1 guaranteeing <=1 output per input)
    #[inline]
    fn push(&mut self, bit: u8) -> Option<f64> {
        let mut first: Option<f64> = None;
        self.push_all(bit, |y| {
            if first.is_none() {
                first = Some(y);
            }
        });
        first
    }

    // Multi-output path: invoke closure for every Stage1 output (in-order) produced by this input sample.
    #[inline]
    fn push_all<F: FnMut(f64)>(&mut self, bit: u8, mut emit: F) {
        // Drain any impossible leftover state (none kept between calls besides ring & counters)
        let s = if bit != 0 { 1.0 } else { -1.0 };
        self.ring[self.w] = s;
        self.w = (self.w + 1) & self.mask;
        self.input_count += 1;

        for _p in 0..self.l {
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
            if self.phase1 != self.d1 {
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
