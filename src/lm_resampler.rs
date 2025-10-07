use crate::filters::HTAPS_DDRX10_21TO1_EQ;
use crate::fir_convolve::FirConvolve;
use crate::filters::HTAPS_1MHZ_3TO1_EQ;
use crate::filters::HTAPS_288K_3TO1_EQ;
use crate::filters::HTAPS_2MHZ_7TO1_EQ; // NEW second-stage (7:1)
use crate::filters::HTAPS_4MHZ_7TO1_EQ; // NEW: ~4.032 MHz -> /7
use crate::filters::HTAPS_576K_3TO1_EQ;
use crate::filters::HTAPS_8MHZ_7TO1_EQ;
use crate::filters::HTAPS_DDRX10_7TO1_EQ;
use crate::filters::HTAPS_DSDX5_7TO1_EQ;
// NEW: 576 kHz -> /3 (final 192 kHz)
use crate::filters::HTAPS_DDRX5_14TO1_EQ; // ADD first-stage half taps (5× up, 14:1 down)
use crate::filters::HTAPS_DDRX5_7TO_1_EQ; // NEW: 10*DSD -> /7
use crate::filters::HTAPS_DSDX10_21TO1_EQ;    // NEW: Stage1 (10 -> /21) half taps
use crate::filters::HTAPS_1_34MHZ_7TO1_EQ;    // NEW: Stage2 ( -> /7 ) half taps
use crate::filters::HTAPS_2_68MHZ_7TO1_EQ; // ADD: 2.688 MHz -> /7 second-stage (×20 /21 path)

// --- Add direct single-stage polyphase path for L=5, M=294 using HTAPS_DDRX5_294TO1_EQ ---
// Enable with env: DSD2DXD_POLY294=1  (falls back to existing 3‑stage cascade if unset)

use crate::filters::HTAPS_DDRX5_294TO1_EQ; // ADD (full-rate (right) half taps for 5x / 294 path)

// Toggle slow-domain Stage1 polyphase (debug / diagnostic).
// Set to true to enable Stage1PolySlow path; keep false for production.
const USE_STAGE1_SLOW: bool = true;

// Add (near existing USE_STAGE1_SLOW)
const STAGE1_SLOW_DBG: bool = false; // set true to print Stage1 polyphase diagnostics

// ====================================================================================
// Generalized equiripple L/M resampler covering:
//   - L=5,  M=294: (×5 -> /14) -> /7 -> /3  -> 96 kHz
//   - L=5,  M=147: (×5 -> /7)  -> /7 -> /3  -> 192 kHz
//   - L=10, M=147: (×10 -> /7) -> /7 -> /3  -> 384 kHz
// Stage1 dump now supported for any M: output after first stage (×L -> /decim1).
// ====================================================================================
pub struct EquiLMResampler {
    // Stage 1
    fir1: FirConvolve,
    up_factor: u32, // L
    decim1: u32,    // 14 (M=294) or 7 (M=147)
    delay1: u64,
    ups_index: u64,
    phase1: u32,
    primed1: bool,
    // Stage 2 (polyphase decimator by 7)
    poly2: DecimFIRSym,
    decim2: u32,     // 7 (kept for latency formula)
    // Stage 3 (polyphase decimator by 3)
    poly3: DecimFIRSym,
    decim3: u32,     // 3
    // NEW: optional single-stage polyphase path for L=5, M=294
    poly294: Option<PolyPhaseL5M294>, // when Some => single-stage L=5/M=294 direct path
    // If Some(d), indicates we enabled the (logical) Stage1 polyphase optimization and
    // stores the Stage1 effective input-sample delay (ceil(group_delay_high / L)).
    stage1_poly_input_delay: Option<u64>,
    // NEW two-phase L=10/M=147 path (DSD64 -> 192k)
    two_phase_l10_m147: Option<TwoPhaseL10M147>,
    two_phase_l20_m147_384: Option<TwoPhaseL20M147_384>, // ADD
    two_phase_l10_m147_384: Option<TwoPhaseL10M147_384>, // NEW
    // Minimal Stage‑1 slow‑domain polyphase (diagnostic) – optional via constant toggle.
    stage1_poly_slow: Option<Stage1PolySlow>,
}

// NEW: PolyPhase294 struct for direct single-stage path
#[derive(Debug)]
struct PolyPhaseL5M294 {
    // L-phase subfilters: sub[r] holds taps where original high-rate index m % L == r
    sub: [Vec<f64>; 5],
    // Circular delay of input (original rate) samples (±1.0)
    delay: Vec<f64>,
    mask: usize,
    write: usize,          // points to next write position
    acc: i32,              // phase accumulator (adds L each input, subtract M when >= M)
    phase_mod: i32,        // (n_out * M) mod L
    input_count: usize,    // number of input samples ingested
    delay_input: usize,    // input samples required before first valid output (group delay in input units)
    l: i32,
    m: i32,
}

impl PolyPhaseL5M294 {
    fn new(half_taps_right: &[f64]) -> Self {
        let l = 5;
        let m = 294;
        // Reconstruct full symmetric taps at HIGH (post-upsampling) rate
        let mut full: Vec<f64> = half_taps_right.iter().rev().cloned().collect();
        full.extend_from_slice(half_taps_right);
        let full_len = full.len();

        // Split into L polyphase branches (index mod L)
        let mut sub: [Vec<f64>; 5] = [
            Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new()
        ];
        for (i, &c) in full.iter().enumerate() {
            sub[i % l as usize].push(c);
        }
        let max_len = sub.iter().map(|v| v.len()).max().unwrap_or(0);

        // Convert group delay ( (N-1)/2 high-rate samples ) into input (original) sample units:
        // Each input sample corresponds to L high-rate slots; only one is non-zero.
        // So integer floor conversion is acceptable; add 1 for safety to ensure full support.
        let group_delay_high = (full_len as u64 - 1) / 2;
        let delay_input = (group_delay_high / l as u64) as usize + 1;

        // Ring buffer size: next power-of-two >= max_len
        let cap = max_len.next_power_of_two().max(64);
        let delay = vec![0.0f64; cap];

        Self {
            sub,
            delay,
            mask: cap - 1,
            write: 0,
            acc: 0,
            phase_mod: 0,
            input_count: 0,
            delay_input,
            l,
            m,
        }
    }

    #[inline]
    fn push_bit(&mut self, bit: u8) -> Option<f64> {
        // Store input sample (±1.0)
        let s = if bit != 0 { 1.0 } else { -1.0 };
        self.delay[self.write & self.mask] = s;
        self.write = (self.write + 1) & self.mask;
        self.input_count += 1;

        // Accumulate phase (rational time): add L per input, produce one output when >= M
        self.acc += self.l;
        if self.acc < self.m {
            return None;
        }
        self.acc -= self.m;

        // Not enough latency yet: consume but do not output
        if self.input_count <= self.delay_input {
            // Update phase_mod (n_out*M mod L) even if we suppress output
            self.phase_mod = (self.phase_mod + (self.m % self.l)) % self.l;
            return None;
        }

        // Phase for this output: r = (n_out * M) mod L
        let phase = self.phase_mod as usize;
        let taps = &self.sub[phase];

        // Convolution over input samples (most recent at write-1)
        let mut acc = 0.0;
        let mut idx = (self.write.wrapping_sub(1)) & self.mask;
        for &c in taps {
            acc += c * self.delay[idx];
            if taps.len() == 0 { break; }
            idx = idx.wrapping_sub(1) & self.mask;
        }

        // Advance (n_out*M mod L): add M % L
        self.phase_mod = (self.phase_mod + (self.m % self.l)) % self.l;

        Some(acc)
    }

    #[inline]
    fn output_latency_frames(&self) -> f64 {
        // Approximate output latency in final-rate frames:
        // Output rate = input_rate * L / M
        // Latency (input samples) => input_delay * (output_rate / input_rate) = delay_input * L / M
        (self.delay_input as f64) * (self.l as f64) / (self.m as f64)
    }
}

// --- ADD: Two-phase L=10/M=147 (21 * 7) path for DSD64 -> 192 kHz --------------------
#[derive(Debug)]
struct TwoPhaseL10M147 {
    stage1: Stage1PolyL10_21, // polyphase L=10 / 21
    stage2: DecimFIRSym,      // /7 integer decimator
}

impl TwoPhaseL10M147 {
    fn new() -> Self {
        Self {
            stage1: Stage1PolyL10_21::new(&HTAPS_DSDX10_21TO1_EQ),
            stage2: DecimFIRSym::new_from_half(&HTAPS_1_34MHZ_7TO1_EQ, 7),
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
        // Stage1 latency (input samples) scaled by L/(21*7)
        let d1 = (self.stage1.input_delay() as f64) * (10.0 / (21.0 * 7.0));
        let d2 = (self.stage2.center_delay() as f64) / 7.0;
        d1 + d2
    }
}

// ADD: Two‑phase L=20 / (21*7) path for DSD64 -> 384 kHz
#[derive(Debug)]
struct TwoPhaseL20M147_384 {
    stage1: Stage1PolyL20_21, // ×20 /21 polyphase
    stage2: DecimFIRSym,      // /7 (2.688 MHz -> 384 kHz)
}

impl TwoPhaseL20M147_384 {
    fn new() -> Self {
        Self {
            stage1: Stage1PolyL20_21::new(&HTAPS_DDRX10_21TO1_EQ),
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
    #[inline]
    fn output_latency_frames(&self) -> f64 {
        // Stage1 latency (input samples) scaled by L / (21*7) plus stage2 latency (scaled by /7)
        let d1 = (self.stage1.input_delay() as f64) * (20.0 / (21.0 * 7.0));
        let d2 = (self.stage2.center_delay() as f64) / 7.0;
        d1 + d2
    }
}

// ADD new two‑phase DSD128 -> 384k path (×10 -> /21 -> /7)
#[derive(Debug)]
struct TwoPhaseL10M147_384 {
    stage1: Stage1PolyL10_21, // ×10 /21 polyphase (already implemented)
    stage2: DecimFIRSym,      // /7 (2.688 MHz -> 384 kHz)
}

impl TwoPhaseL10M147_384 {
    fn new() -> Self {
        Self {
            stage1: Stage1PolyL10_21::new(&HTAPS_DDRX10_21TO1_EQ),
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
    #[inline]
    fn output_latency_frames(&self) -> f64 {
        // Stage1 latency (input samples) scaled by L/(21*7) + stage2 latency (/7 domain)
        let d1 = (self.stage1.input_delay() as f64) * (10.0 / (21.0 * 7.0));
        let d2 = (self.stage2.center_delay() as f64) / 7.0;
        d1 + d2
    }
}

// ADD: Stage1 polyphase L=20 /21
#[derive(Debug)]
struct Stage1PolyL20_21 {
    phases: [Vec<f64>; 20],
    l: u32,         // 20
    m: u32,         // 21
    ring: Vec<f64>, // input (original DSD64 rate) history
    mask: usize,
    w: usize,
    acc: u32,
    phase_mod: u32,
    input_count: u64,
    input_delay: u64,
    primed: bool,
}

impl Stage1PolyL20_21 {
    fn new(right_half: &[f64]) -> Self {
        let l = 20;
        let m = 21;
        // Rebuild full symmetric taps
        let mut full: Vec<f64> = right_half.iter().rev().cloned().collect();
        full.extend_from_slice(right_half);
        let n = full.len();
        // Build phases h_r[k] = full[r + kL]
        let mut phases: [Vec<f64>; 20] = Default::default();
        for (i, &c) in full.iter().enumerate() {
            phases[i % 20].push(c);
        }
        // Group delay in input samples (ceil)
        let gd_high = (n as u64 - 1) / 2;
        let input_delay = (gd_high + (l as u64 - 1)) / l as u64;
        let max_len = phases.iter().map(|p| p.len()).max().unwrap_or(0);
        let cap = max_len.next_power_of_two().max(128);

        Self {
            phases,
            l: l as u32,
            m: m as u32,
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
        let mut sum = 0.0;
        let mut idx = (self.w + self.ring.len() - 1) & self.mask;
        for &c in taps {
            sum += c * self.ring[idx];
            idx = (idx + self.ring.len() - 1) & self.mask;
        }

        self.phase_mod = (self.phase_mod + (self.m % self.l)) % self.l;

        Some(sum)
    }

    #[inline]
    fn input_delay(&self) -> u64 {
        self.input_delay
    }
}

// --- ====================================================================================
// ====================================================================================
impl EquiLMResampler {
    pub fn new(
        l: u32,
        m: i32,
        verbose: bool,
        first_channel: bool,
        out_rate: u32,
    ) -> Self {
        let use_poly294 = std::env::var("DSD2DXD_POLY294")
            .map(|v| {
                let v = v.to_ascii_lowercase();
                v == "1" || v == "true" || v == "yes" || v == "on"
            })
            .unwrap_or(false);

        match m {
            294 => {
                if use_poly294 && l == 5 {
                    let poly = PolyPhaseL5M294::new(&HTAPS_DDRX5_294TO1_EQ);
                    if verbose {
                        eprintln!("[DBG] Polyphase L=5/M=294 single-stage path enabled (HTAPS_DDRX5_294TO1_EQ).");
                    }
                    return Self {
                        fir1: FirConvolve::new(&HTAPS_DDRX5_14TO1_EQ),
                        up_factor: l,
                        decim1: 14,
                        delay1: 0,
                        ups_index: 0,
                        phase1: 0,
                        primed1: true,
                        // Stage 2 (polyphase decimator by 7)
                        poly2: DecimFIRSym::new_from_half(&HTAPS_2MHZ_7TO1_EQ, 7),
                        decim2: 7,
                        // Stage 3 (polyphase decimator by 3)
                        poly3: DecimFIRSym::new_from_half(&HTAPS_288K_3TO1_EQ, 3),
                        decim3: 3,
                        poly294: Some(poly),
                        // If Some(d), indicates we enabled the (logical) Stage1 polyphase optimization and
                        // stores the Stage1 effective input-sample delay (ceil(group_delay_high / L)).
                        stage1_poly_input_delay: None,
                        two_phase_l10_m147: None,
                        two_phase_l20_m147_384: None, // ADD
                        two_phase_l10_m147_384: None, // NEW
                        stage1_poly_slow: None,
                    };
                }
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
                    ups_index: 0,
                    phase1: 0,
                    primed1: false,
                    // Stage 2 (polyphase decimator by 7)
                    poly2,
                    decim2: 7,
                    // Stage 3 (polyphase decimator by 3)
                    poly3,
                    decim3: 3,
                    poly294: None,
                    stage1_poly_input_delay,
                    two_phase_l10_m147: None,
                    two_phase_l20_m147_384: None, // ADD
                    two_phase_l10_m147_384: None, // NEW
                    stage1_poly_slow: None,
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
                    let tp = TwoPhaseL10M147_384::new();
                    // Minimal placeholders (not used directly)
                    let fir1 = FirConvolve::new(&HTAPS_DDRX10_21TO1_EQ);
                    let dummy2 = DecimFIRSym::new_from_half(&HTAPS_2_68MHZ_7TO1_EQ, 7);
                    let dummy3 = DecimFIRSym::new_from_half(&HTAPS_2_68MHZ_7TO1_EQ, 7);
                    return Self {
                        fir1,
                        up_factor: l,
                        decim1: 21,
                        delay1: 0,
                        ups_index: 0,
                        phase1: 0,
                        primed1: true,
                        poly2: dummy2,
                        decim2: 7,
                        poly3: dummy3,
                        decim3: 3,
                        poly294: None,
                        stage1_poly_input_delay: None,
                        two_phase_l10_m147: None,
                        two_phase_l20_m147_384: None,
                        two_phase_l10_m147_384: Some(tp),
                        stage1_poly_slow: None,
                    };
                }

                // Existing L=20 two‑phase 384k branch remains below
                if l == 20 && out_rate == 384_000 {
                    if verbose {
                        eprintln!("[DBG] Two-phase L=20/M=147 path enabled: (×20 -> /21 (poly) -> /7) => 384k");
                    }
                    let tp = TwoPhaseL20M147_384::new();
                    // Placeholder FIR & decims (unused in this mode beyond latency scaffolding)
                    let fir1 = FirConvolve::new(&HTAPS_DDRX10_21TO1_EQ);
                    let dummy2 = DecimFIRSym::new_from_half(&HTAPS_2_68MHZ_7TO1_EQ, 7);
                    let dummy3 = DecimFIRSym::new_from_half(&HTAPS_2_68MHZ_7TO1_EQ, 7);
                    return Self {
                        fir1,
                        up_factor: l,
                        decim1: 21,
                        delay1: 0,
                        ups_index: 0,
                        phase1: 0,
                        primed1: true,
                        poly2: dummy2,
                        decim2: 7,
                        poly3: dummy3,
                        decim3: 3,
                        poly294: None,
                        stage1_poly_input_delay: None,
                        two_phase_l10_m147: None,
                        two_phase_l20_m147_384: Some(tp),
                        two_phase_l10_m147_384: None,
                        stage1_poly_slow: None,
                    };
                }

                // (Existing selection logic unchanged, just swap Stage2/3 construction)
                let (fir1, full1, right2, full2, right3, full3, label) =
                    if (l == 10 || l == 20) && out_rate == 384_000 {
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
                let stage1_half: &[f64] = if (l == 10 || l == 20) && out_rate == 384_000 {
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
                    ups_index: 0,
                    phase1: 0,
                    primed1: false,
                    // Stage 2 (polyphase decimator by 7)
                    poly2,
                    decim2: 7,
                    // Stage 3 (polyphase decimator by 3)
                    poly3,
                    decim3: 3,
                    poly294: None,
                    stage1_poly_input_delay,
                    two_phase_l10_m147: None,
                    two_phase_l20_m147_384: None, // ADD
                    two_phase_l10_m147_384: None, // NEW
                    stage1_poly_slow: None,
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

    // Unified push: handles both full cascade and stage1-dump. Breaks early once an output is produced.
    #[inline]
    pub fn push_bit_lm(&mut self, bit: u8) -> Option<f64> {
        if let Some(poly) = self.poly294.as_mut() {
            return poly.push_bit(bit);
        }
        if let Some(tp) = self.two_phase_l10_m147_384.as_mut() {
            return tp.push_bit(bit);
        }
        if let Some(tp) = self.two_phase_l20_m147_384.as_mut() {
            return tp.push_bit(bit);
        }
        if let Some(tp) = self.two_phase_l10_m147.as_mut() {
            return tp.push_bit(bit);
        }

        // FULL Stage1 polyphase for L=5 and L=20 (now supports multi-output per input internally).
        // Restrict experimental slow Stage1 polyphase to L=5 only (L=20 needs its own tap design).
        if USE_STAGE1_SLOW && self.up_factor == 5 {
            if self.stage1_poly_slow.is_none() {
                self.stage1_poly_slow = Some(Stage1PolySlow::new(
                    &self.fir1.full_taps,
                    self.up_factor,
                    self.decim1,
                ));
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

        // Legacy zero-stuff path (unchanged) for other L / modes.
        let bit_i8: i8 = if bit != 0 { 1 } else { -1 };
        let mut out: Option<f64> = None;
        for p in 0..self.up_factor {
            let xi8 = if p == 0 { bit_i8 } else { 0 };
            let y1 = self.fir1.process_sample_i8(xi8);
            let idx_up = self.ups_index;
            self.ups_index += 1;

            if !self.primed1 {
                if idx_up >= self.delay1 {
                    self.primed1 = true;
                    self.phase1 = 0;
                }
                continue;
            }

            self.phase1 += 1;
            if self.phase1 != self.decim1 {
                continue;
            }
            self.phase1 = 0;

            if let Some(y2) = self.poly2.push(y1) {
                if let Some(y3) = self.poly3.push(y2) {
                    out = Some(y3);
                }
            }
        }
        out
    }

    #[inline]
    pub fn output_latency_frames(&self) -> f64 {
        if let Some(tp) = self.two_phase_l10_m147_384.as_ref() {
            return tp.output_latency_frames();
        }
        if let Some(tp) = self.two_phase_l20_m147_384.as_ref() {
            return tp.output_latency_frames();
        }
        if let Some(tp) = self.two_phase_l10_m147.as_ref() {
            return tp.output_latency_frames();
        }
        // If logical Stage1 polyphase enabled, use its input-sample delay; else use raw high-rate delay1.
        let stage1_delay_out = if let Some(d_in) = self.stage1_poly_input_delay {
            (d_in as f64) * (self.up_factor as f64)
                / (self.decim1 as f64 * self.decim2 as f64 * self.decim3 as f64)
        } else {
            (self.delay1 as f64)
                / (self.decim1 as f64 * self.decim2 as f64 * self.decim3 as f64)
        };
        let l1 = stage1_delay_out;
        let l2 = (self.poly2.center_delay() as f64)
            / (self.decim2 as f64 * self.decim3 as f64);
        let l3 = (self.poly3.center_delay() as f64) / (self.decim3 as f64);
        l1 + l2 + l3
    }
}

// Lightweight integer polyphase decimator structure (D=7 or 3)
#[derive(Debug)]
struct DecimFIRSym {
    full: Vec<f64>,    // full symmetric taps
    len: usize,
    half: usize,
    has_center: bool,
    center: usize,     // (len-1)/2
    decim: usize,      // decimation factor D
    ring: Vec<f64>,
    mask: usize,
    w: usize,          // next write index
    count: usize,      // total samples seen
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

// --- BEGIN ADD Stage1PolyL10_21 -------------------------------------------------------

#[derive(Debug)]
struct Stage1PolyL10_21 {
    // L=10 phase polyphase: phases[r][k] = h[r + k*10]
    phases: [Vec<f64>; 10],
    l: u32,          // 10
    m: u32,          // 21
    ring: Vec<f64>,  // input (original DSD64 rate) sample history
    mask: usize,
    w: usize,
    acc: u32,        // time accumulator (adds L, subtracts M)
    phase_mod: u32,  // (n_out * M) mod L advanced by (M % L) each output
    input_count: u64,
    input_delay: u64,
    primed: bool,
}

impl Stage1PolyL10_21 {
    fn new(right_half: &[f64]) -> Self {
        let l = 10u32;
        let m = 21u32;

        // Rebuild full symmetric high‑rate taps
        let mut full: Vec<f64> = right_half.iter().rev().cloned().collect();
        full.extend_from_slice(right_half);
        let n = full.len();

        // Build L phases
        let mut phases: [Vec<f64>; 10] = Default::default();
        for (i, &c) in full.iter().enumerate() {
            phases[i % 10].push(c);
        }

        // Group delay in high-rate samples => convert to input samples (ceil division by L)
        let gd_high = (n as u64 - 1) / 2;
        let input_delay = (gd_high + (l as u64 - 1)) / l as u64;

        let max_len = phases.iter().map(|p| p.len()).max().unwrap_or(0);
        let cap = max_len.next_power_of_two().max(64);

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

        // Advance rational time
        self.acc += self.l;
        if self.acc < self.m {
            return None;
        }
        self.acc -= self.m;

        // Wait until we have enough history (group delay)
        if !self.primed {
            if self.input_count >= self.input_delay {
                self.primed = true;
            } else {
                // Still advance phase sequence for deterministic alignment
                self.phase_mod = (self.phase_mod + (self.m % self.l)) % self.l;
                return None;
            }
        }

        // Phase for this output
        let phase = self.phase_mod as usize;
        let taps = &self.phases[phase];

        // Convolution:
        // taps[k] corresponds to input sample x[n - k]
        // newest input is at w-1; iterate taps forward consuming older samples
        let mut acc_sum = 0.0;
        let mut idx = (self.w + self.ring.len() - 1) & self.mask; // newest
        for &c in taps {
            acc_sum += c * self.ring[idx];
            idx = (idx + self.ring.len() - 1) & self.mask; // older
        }

        // Advance phase_mod for next output
        self.phase_mod = (self.phase_mod + (self.m % self.l)) % self.l;

        Some(acc_sum)
    }

    #[inline]
    fn input_delay(&self) -> u64 {
        self.input_delay
    }
}

// --- END ADD Stage1PolyL10_21 ---------------------------------------------------------

use std::cell::RefCell;
use std::collections::HashMap;
use std::collections::VecDeque;

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
    high_index: u64,       // high-rate sample counter (after upsample expansion)
    delay_high: u64,       // (N-1)/2 group delay in high-rate samples
    phase1: u32,           // decimation phase counter (0..d1-1)
    primed: bool,
    input_count: u64,
    dbg_outputs: u64,
}

impl Stage1PolySlow {
    fn new(full_taps: &[f64], l: u32, d1: u32) -> Self {
        // Build phases in natural time order (k increasing)
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
            dbg_outputs: 0,
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
        for p in 0..self.l {
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

                if STAGE1_SLOW_DBG && self.dbg_outputs < 128 {
                    eprintln!(
                        "[S1DBG] in={} p={} emit=1 high_idx={} phase_used={} taps={} total_out={}",
                        self.input_count,
                        p,
                        idx_high,
                        phase,
                        taps.len(),
                        self.dbg_outputs + 1
                    );
                }
                self.dbg_outputs += 1;
            }
            // (Do NOT break; continue to advance remaining high-rate slots.)
        }

        if STAGE1_SLOW_DBG && emitted.is_none() && self.input_count <= 128 {
            eprintln!(
                "[S1DBG] in={} emit=0 high_idx_end={} primed={} phase1={}",
                self.input_count,
                self.high_index,
                self.primed,
                self.phase1
            );
        }

        emitted
    }
}
