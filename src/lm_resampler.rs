use crate::fir_convolve::FirConvolve;
use crate::dsd2pcm::HTAPS_1MHZ_3TO1_EQ;
use crate::dsd2pcm::HTAPS_288K_3TO1_EQ;
use crate::dsd2pcm::HTAPS_2MHZ_7TO1_EQ; // NEW second-stage (7:1)
use crate::dsd2pcm::HTAPS_4MHZ_7TO1_EQ; // NEW: ~4.032 MHz -> /7
use crate::dsd2pcm::HTAPS_576K_3TO1_EQ;
use crate::dsd2pcm::HTAPS_8MHZ_7TO1_EQ;
use crate::dsd2pcm::HTAPS_DDRX10_7TO1_EQ;
use crate::dsd2pcm::HTAPS_DSDX5_7TO1_EQ;
// NEW: 576 kHz -> /3 (final 192 kHz)
use crate::dsd2pcm::HTAPS_DDRX5_14TO1_EQ; // ADD first-stage half taps (5× up, 14:1 down)
use crate::dsd2pcm::HTAPS_DDRX5_7TO_1_EQ; // NEW: 10*DSD -> /7
use crate::dsd2pcm::HTAPS_DSDX10_21TO1_EQ;    // NEW: Stage1 (10 -> /21) half taps
use crate::dsd2pcm::HTAPS_1_34MHZ_7TO1_EQ;    // NEW: Stage2 ( -> /7 ) half taps

// --- Add direct single-stage polyphase path for L=5, M=294 using HTAPS_DDRX5_294TO1_EQ ---
// Enable with env: DSD2DXD_POLY294=1  (falls back to existing 3‑stage cascade if unset)

use crate::dsd2pcm::HTAPS_DDRX5_294TO1_EQ; // ADD (full-rate (right) half taps for 5x / 294 path)

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
    delay2: u64,     // original full-filter group delay (for latency calc)
    decim2: u32,     // 7 (kept for latency formula)
    // Stage 3 (polyphase decimator by 3)
    poly3: DecimFIRSym,
    delay3: u64,
    decim3: u32,     // 3
    // NEW: optional single-stage polyphase path for L=5, M=294
    poly294: Option<PolyPhaseL5M294>, // when Some => single-stage L=5/M=294 direct path
    // --- ADD: Stage1 rational polyphase (only used when L <= decim1 and env enables) ---
    stage1_poly: Option<Stage1PolyL>,
    // NEW two-phase L=10/M=147 path (DSD64 -> 192k)
    two_phase_l10_m147: Option<TwoPhaseL10M147>,
}

// NEW: PolyPhase294 struct for direct single-stage path
#[derive(Debug)]
struct PolyPhaseL5M294 {
    // L-phase subfilters: sub[r] holds taps where original high-rate index m % L == r
    sub: [Vec<f64>; 5],
    max_len: usize,
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
            max_len,
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

// --- ADD: Stage1 rational polyphase (only used when L <= decim1 and env enables) ---
#[derive(Debug)]
struct Stage1PolyL {
    // L-phase polyphase: phases[r][k] = h[r + k*L]
    phases: Vec<Vec<f64>>,
    l: u32,
    m: u32,              // decim1
    ring: Vec<f64>,
    mask: usize,
    w: usize,
    input_count: u64,
    // Rational time accumulator: acc += l; while acc >= m produce output; acc -= m.
    acc: u32,
    // Phase (n*M mod L): updated only when we emit an output
    phase_mod: u32,
    // Base input delay (ceil( (N-1)/2 / L )) to approximate group delay before first output
    input_delay: u64,
    primed: bool,
}

impl Stage1PolyL {
    fn new(right_half: &[f64], l: u32, m: u32) -> Self {
        // Full symmetric taps
        let mut full: Vec<f64> = right_half.iter().rev().cloned().collect();
        full.extend_from_slice(right_half);
        let n = full.len();
        let l_usize = l as usize;

        // Build L phases
        let mut phases = vec![Vec::<f64>::new(); l_usize];
        for (i, &c) in full.iter().enumerate() {
            phases[i % l_usize].push(c);
        }

        // Group delay in high-rate domain
        let gd_high = (n as u64 - 1) / 2;
        // Convert to input sample delay (ceil)
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
            input_count: 0,
            acc: 0,
            phase_mod: 0,
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
                return None;
            }
        }

        // Phase r sequence (n_out * M) mod L tracked via phase_mod advance
        let phase = self.phase_mod as usize;
        let taps = &self.phases[phase];

        // IMPORTANT FIX:
        // For correct L-phase rational polyphase, taps with lower high‑rate indices
        // correspond to OLDER input samples. We were previously multiplying them
        // against NEWER samples, inverting time and destroying the response
        // (white noise / aliasing). Iterate taps in REVERSE so the largest
        // index (latest in impulse) multiplies newest sample first.
        let mut acc_sum = 0.0;
        let mut idx = (self.w + self.ring.len() - 1) & self.mask; // newest
        for &c in taps.iter().rev() {
            acc_sum += c * self.ring[idx];
            idx = (idx + self.ring.len() - 1) & self.mask; // older sample
        }

        // Advance phase_mod
        let step = (self.m % self.l) as u32;
        self.phase_mod = (self.phase_mod + step) % self.l;

        Some(acc_sum)
    }

    #[inline]
    fn input_delay(&self) -> u64 {
        self.input_delay
    }
}

fn stage1_poly_enabled() -> bool {
    std::env::var("DSD2DXD_STAGE1_POLY")
        .map(|v| {
            let v = v.to_ascii_lowercase();
            v == "1" || v == "true" || v == "yes" || v == "on"
        })
        .unwrap_or(false)
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

// 3. In EquiLMResampler::new (m == 147 branch) update the early two-phase debug message
// (search for the existing two-phase debug eprintln! and replace its text):

// OLD (inside if l == 10 && target_rate_hz == 192_000 && !dump_stage1):
// "[DBG] Two-phase L=10/M=147 path enabled: (×10 -> /21 -> /7) reference Stage1 FIR + optimized Stage2"
// NEW:
 // "[DBG] Two-phase L=10/M=147 path enabled: (×10 -> /21 (poly L-phase) -> /7)"

// (Only the string changes; no logic change needed.)

// --- ====================================================================================
// ====================================================================================
impl EquiLMResampler {
    pub fn new(
        l: u32,
        m: i32,
        verbose: bool,
        dump_stage1: bool,
        print_config: bool,
        target_rate_hz: i32,
    ) -> Self {
        let use_poly294 = std::env::var("DSD2DXD_POLY294")
            .map(|v| {
                let v = v.to_ascii_lowercase();
                v == "1" || v == "true" || v == "yes" || v == "on"
            })
            .unwrap_or(false);

        match m {
            294 => {
                if use_poly294 && l == 5 && !dump_stage1 {
                    let poly = PolyPhaseL5M294::new(&HTAPS_DDRX5_294TO1_EQ);
                    if verbose && print_config {
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
                        delay2: 0,
                        decim2: 7,
                        // Stage 3 (polyphase decimator by 3)
                        poly3: DecimFIRSym::new_from_half(&HTAPS_288K_3TO1_EQ, 3),
                        delay3: 0,
                        decim3: 3,
                        poly294: Some(poly),
                        stage1_poly: None,
                        two_phase_l10_m147: None,
                    };
                }
                // Original cascade Stage1 definitions
                let fir1 = FirConvolve::new(&HTAPS_DDRX5_14TO1_EQ);
                let full1 = (HTAPS_DDRX5_14TO1_EQ.len() * 2) as u64;
                let full2 = (HTAPS_2MHZ_7TO1_EQ.len() * 2) as u64;
                let full3 = (HTAPS_288K_3TO1_EQ.len() * 2) as u64;
                let delay1 = (full1 - 1) / 2;
                let delay2 = (full2 - 1) / 2;
                let delay3 = (full3 - 1) / 2;
                let poly2 = DecimFIRSym::new_from_half(&HTAPS_2MHZ_7TO1_EQ, 7);
                let poly3 = DecimFIRSym::new_from_half(&HTAPS_288K_3TO1_EQ, 3);
                let use_stage1_poly = l < 14 && !dump_stage1;
                let stage1_poly = if use_stage1_poly {
                    if verbose && print_config {
                        eprintln!(
                            "[DBG] Stage1 polyphase (L-phase) enabled by default (L={} decim1=14)",
                            l
                        );
                    }
                    Some(Stage1PolyL::new(&HTAPS_DDRX5_14TO1_EQ, l, 14))
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
                    delay2,
                    decim2: 7,
                    // Stage 3 (polyphase decimator by 3)
                    poly3,
                    delay3,
                    decim3: 3,
                    poly294: None,
                    stage1_poly,
                    two_phase_l10_m147: None,
                };
                if verbose && print_config {
                    if dump_stage1 {
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
                // NEW: two-phase L=10 / (21*7) path for DSD64 -> 192k
                if l == 10 && target_rate_hz == 192_000 && !dump_stage1 {
                    if verbose && print_config {
                        eprintln!(
                            "[DBG] Two-phase L=10/M=147 path enabled: (×10 -> /21 (poly L-phase) -> /7)"
                        );
                    }
                    let two_phase = TwoPhaseL10M147::new();
                    // Provide placeholders for fields required by struct but unused in this mode
                    let fir1 = FirConvolve::new(&HTAPS_DSDX10_21TO1_EQ); // not used directly (internal handled in two_phase)
                    let poly2 = DecimFIRSym::new_from_half(&HTAPS_1_34MHZ_7TO1_EQ, 7);
                    let poly3 = DecimFIRSym::new_from_half(&HTAPS_1_34MHZ_7TO1_EQ, 7);
                    return Self {
                        fir1,
                        up_factor: l,
                        decim1: 21,
                        delay1: 0,
                        ups_index: 0,
                        phase1: 0,
                        primed1: true,
                        poly2,
                        delay2: 0,
                        decim2: 7,
                        poly3,
                        delay3: 0,
                        decim3: 3,
                        poly294: None,
                        stage1_poly: None,
                        two_phase_l10_m147: Some(two_phase),
                    };
                }
                // (Existing selection logic unchanged, just swap Stage2/3 construction)
                let (fir1, full1, right2, full2, right3, full3, label) =
                    if (l == 10 || l == 20) && target_rate_hz == 384_000 {
                        (
                             FirConvolve::new(&HTAPS_DDRX10_7TO1_EQ),
                             (HTAPS_DDRX10_7TO1_EQ.len() * 2) as u64,
                             &HTAPS_8MHZ_7TO1_EQ[..],
                             (HTAPS_8MHZ_7TO1_EQ.len() * 2) as u64,
                             &HTAPS_1MHZ_3TO1_EQ[..],
                             (HTAPS_1MHZ_3TO1_EQ.len() * 2) as u64,
                             "384k (DDRx10, 8MHz, 1MHz)",
                         )
                    } else if l == 5 && target_rate_hz == 96_000 {
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
                let stage1_half: &[f64] = if (l == 10 || l == 20) && target_rate_hz == 384_000 {
                    &HTAPS_DDRX10_7TO1_EQ
                } else if l == 5 && target_rate_hz == 96_000 {
                    &HTAPS_DSDX5_7TO1_EQ
                } else {
                    &HTAPS_DDRX5_7TO_1_EQ
                };
                let delay1 = (full1 - 1) / 2;
                let delay2 = (full2 - 1) / 2;
                let delay3 = (full3 - 1) / 2;
                let poly2 = DecimFIRSym::new_from_half(right2, 7);
                let poly3 = DecimFIRSym::new_from_half(right3, 3);
                let use_stage1_poly = l < 7 && !dump_stage1; // only L=5 benefits
                let stage1_poly = if use_stage1_poly {
                    if verbose && print_config {
                        eprintln!(
                            "[DBG] Stage1 polyphase (L-phase) enabled by default (L={} decim1=7)",
                            l
                        );
                    }
                    Some(Stage1PolyL::new(stage1_half, l, 7))
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
                    delay2,
                    decim2: 7,
                    // Stage 3 (polyphase decimator by 3)
                    poly3,
                    delay3,
                    decim3: 3,
                    poly294: None,
                    stage1_poly,
                    two_phase_l10_m147: None,
                };
                if verbose && print_config {
                    if dump_stage1 {
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
    pub fn push_bit_lm(&mut self, bit: u8, dump_stage1: bool) -> Option<f64> {
        if let Some(poly) = self.poly294.as_mut() {
            return poly.push_bit(bit);
        }
        if let Some(tp) = self.two_phase_l10_m147.as_mut() {
            return tp.push_bit(bit);
        }
        let bit_i8: i8 = if bit != 0 { 1 } else { -1 };
        let mut out: Option<f64> = None;
        for p in 0..self.up_factor {
            let xi8 = if p == 0 { bit_i8 } else { 0 };
            let y1 = self.fir1.process_sample_i8(xi8);
            let idx_up = self.ups_index;
            self.ups_index += 1;

            // Prime stage 1
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

            // Stage1 dump mode: capture stage1 output but DO NOT early return
            // (we still must process the remaining zero-stuffed samples for correctness).
            if dump_stage1 {
                out = Some(y1);
                continue;
            }

            // Stage 2 polyphase decimation
            if let Some(y2) = self.poly2.push(y1) {
                // Stage 3 polyphase decimation
                if let Some(y3) = self.poly3.push(y2) {
                    out = Some(y3);
                }
            }
        }
        out
    }

    #[inline]
    pub fn output_latency_frames(&self, dump_stage1: bool) -> f64 {
        if let Some(poly) = self.poly294.as_ref() {
            return poly.output_latency_frames();
        }
        if let Some(tp) = self.two_phase_l10_m147.as_ref() {
            return tp.output_latency_frames();
        }
        if let Some(stage1p) = self.stage1_poly.as_ref() {
            if dump_stage1 {
                // Stage1 outputs are at rate: input * L / decim1
                return (stage1p.input_delay() as f64) * (self.up_factor as f64) / (self.decim1 as f64);
            }
            // Cascade: stage1 delay (in input samples) converted to final output frames
            let d1_out = (stage1p.input_delay() as f64)
                * (self.up_factor as f64)
                / (self.decim1 as f64 * self.decim2 as f64 * self.decim3 as f64);
            let d2 = (self.poly2.center_delay() as f64) / (self.decim2 as f64 * self.decim3 as f64);
            let d3 = (self.poly3.center_delay() as f64) / (self.decim3 as f64);
            return d1_out + d2 + d3;
        }
        if dump_stage1 {
            return (self.delay1 as f64) / (self.decim1 as f64);
        }
        // delay1 at Stage1 input rate contributes divided by (decim1 * decim2 * decim3)
        let l1 = (self.delay1 as f64)
            / (self.decim1 as f64 * self.decim2 as f64 * self.decim3 as f64);
        // Stage2 center delay (in Stage2 input samples) divided by (decim2 * decim3)
        let l2 = (self.poly2.center_delay() as f64)
            / (self.decim2 as f64 * self.decim3 as f64);
        // Stage3 center delay divided by decim3
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
    left_half: Vec<f64>, // first half taps (outer->inner) for fast SIMD load
    kind: SimdKind,
}

#[derive(Copy, Clone, Debug)]
enum SimdKind {
    Scalar,
    Avx2,
    Neon,
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
        let kind = Self::detect_kind();
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
            kind,
        }
    }

    #[inline]
    fn detect_kind() -> SimdKind {
        #[cfg(all(target_arch="x86_64"))]
        {
            if std::is_x86_feature_detected!("avx2") {
                return SimdKind::Avx2;
            }
        }
        #[cfg(all(target_arch="aarch64"))]
        {
            return SimdKind::Neon;
        }
        SimdKind::Scalar
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

        let acc = match self.kind {
            SimdKind::Scalar => unsafe { self.convolve_scalar() },
            SimdKind::Avx2 => {
                #[cfg(all(target_arch="x86_64"))] unsafe { self.convolve_avx2() }
                #[cfg(not(all(target_arch="x86_64")))] unsafe { self.convolve_scalar() }
            }
            SimdKind::Neon => {
                #[cfg(all(target_arch="aarch64"))] unsafe { self.convolve_neon() }
                #[cfg(not(all(target_arch="aarch64")))] unsafe { self.convolve_scalar() }
            }
        };
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

    #[cfg(all(target_arch="x86_64"))]
    #[target_feature(enable="avx2")]
    unsafe fn convolve_avx2(&self) -> f64 {
        use std::arch::x86_64::*;
        if self.half < 4 {
            return self.convolve_scalar();
        }
        let newest = (self.w + self.ring.len() - 1) & self.mask;
        let mut acc_v = _mm256_setzero_pd();
        let mut i = 0usize;
        let limit = self.half & !3;
        while i < limit {
            let mut pair_sum = [0.0f64; 4];
            // Gather pairwise sums scalar (cheap vs gather instr)
            for lane in 0..4 {
                let k = i + lane;
                let left_idx = (newest + self.ring.len() - (self.len - 1 - k)) & self.mask;
                let right_idx = (newest + self.ring.len() - k) & self.mask;
                pair_sum[lane] = self.ring[left_idx] + self.ring[right_idx];
            }
            let taps = _mm256_loadu_pd(self.left_half.as_ptr().add(i));
            let sums = _mm256_loadu_pd(pair_sum.as_ptr());
            acc_v = _mm256_fmadd_pd(taps, sums, acc_v);
            i += 4;
        }
        let mut tmp = [0.0f64; 4];
        _mm256_storeu_pd(tmp.as_mut_ptr(), acc_v);
        let mut acc = tmp.iter().sum::<f64>();
        // Remainder
        while i < self.half {
            let left_idx = (newest + self.ring.len() - (self.len - 1 - i)) & self.mask;
            let right_idx = (newest + self.ring.len() - i) & self.mask;
            acc += self.left_half[i] * (self.ring[left_idx] + self.ring[right_idx]);
            i += 1;
        }
        if self.has_center {
            let center_idx = (newest + self.ring.len() - (self.len - 1 - self.half)) & self.mask;
            acc += self.full[self.half] * self.ring[center_idx];
        }
        acc
    }

    #[cfg(all(target_arch="aarch64"))]
    #[target_feature(enable="neon")]
    unsafe fn convolve_neon(&self) -> f64 {
        use core::arch::aarch64::*;
        if self.half < 2 {
            return self.convolve_scalar();
        }
        let newest = (self.w + self.ring.len() - 1) & self.mask;
        let mut acc_v = vdupq_n_f64(0.0);
        let mut i = 0usize;
        let limit = self.half & !1;
        while i < limit {
            let mut pair = [0.0f64; 2];
            for lane in 0..2 {
                let k = i + lane;
                let left_idx = (newest + self.ring.len() - (self.len - 1 - k)) & self.mask;
                let right_idx = (newest + self.ring.len() - k) & self.mask;
                pair[lane] = self.ring[left_idx] + self.ring[right_idx];
            }
            let taps = vld1q_f64(self.left_half.as_ptr().add(i));
            let sums = vld1q_f64(pair.as_ptr());
            acc_v = vfmaq_f64(acc_v, taps, sums);
            i += 2;
        }
        let mut acc = vaddvq_f64(acc_v);
        while i < self.half {
            let left_idx = (newest + self.ring.len() - (self.len - 1 - i)) & self.mask;
            let right_idx = (newest + self.ring.len() - i) & self.mask;
            acc += self.left_half[i] * (self.ring[left_idx] + self.ring[right_idx]);
            i += 1;
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
