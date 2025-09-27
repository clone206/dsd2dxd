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
    // Stage 2
    fir2: FirConvolve,
    decim2: u32, // 7
    delay2: u64,
    mid_index2: u64,
    phase2: u32,
    primed2: bool,
    // Stage 3
    fir3: FirConvolve,
    decim3: u32, // 3
    delay3: u64,
    mid_index3: u64,
    phase3: u32,
    primed3: bool,
    // NEW: optional single-stage polyphase path for L=5, M=294
    poly294: Option<PolyPhaseL5M294>, // when Some => single-stage L=5/M=294 direct path
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
                        // Stub unused cascade fields with minimal objects (small filters)
                        fir1: FirConvolve::new(&HTAPS_DDRX5_14TO1_EQ),
                        up_factor: l,
                        decim1: 14,
                        delay1: 0,
                        ups_index: 0,
                        phase1: 0,
                        primed1: true,
                        fir2: FirConvolve::new(&HTAPS_2MHZ_7TO1_EQ),
                        decim2: 7,
                        delay2: 0,
                        mid_index2: 0,
                        phase2: 0,
                        primed2: true,
                        fir3: FirConvolve::new(&HTAPS_288K_3TO1_EQ),
                        decim3: 3,
                        delay3: 0,
                        mid_index3: 0,
                        phase3: 0,
                        primed3: true,
                        poly294: Some(poly),
                    };
                }
                // existing cascade path:
                let fir1 = FirConvolve::new(&HTAPS_DDRX5_14TO1_EQ);
                let fir2 = FirConvolve::new(&HTAPS_2MHZ_7TO1_EQ);
                let fir3 = FirConvolve::new(&HTAPS_288K_3TO1_EQ);
                let full1 = (HTAPS_DDRX5_14TO1_EQ.len() * 2) as u64;
                let full2 = (HTAPS_2MHZ_7TO1_EQ.len() * 2) as u64;
                let full3 = (HTAPS_288K_3TO1_EQ.len() * 2) as u64;
                let delay1 = (full1 - 1) / 2;
                let delay2 = (full2 - 1) / 2;
                let delay3 = (full3 - 1) / 2;
                let s = Self {
                    fir1,
                    up_factor: l,
                    decim1: 14,
                    delay1,
                    ups_index: 0,
                    phase1: 0,
                    primed1: false,
                    fir2,
                    decim2: 7,
                    delay2,
                    mid_index2: 0,
                    phase2: 0,
                    primed2: false,
                    fir3,
                    decim3: 3,
                    delay3,
                    mid_index3: 0,
                    phase3: 0,
                    primed3: false,
                    poly294: None,
                };
                if verbose && print_config {
                    if dump_stage1 {
                        eprintln!(
                            "[DBG] Equiripple L/M path: L={} M=294 — STAGE1 DUMP (×L -> /14).",
                            l
                        );
                    } else {
                        eprintln!(
                            "[DBG] Equiripple L/M path: L={} M=294 — (×L -> /14 -> /7 -> /3).",
                            l
                        );
                    }
                }
                s
            }
            147 => {
                // Select taps for M=147
                let (fir1, fir2, fir3, full1, full2, full3) =
                    if (l == 10 || l == 20) && target_rate_hz == 384_000 {
                        // 384k: reuse DDR×10 first-stage taps for both L=10 (DSD128) and L=20 (DSD64)
                        let f1 = FirConvolve::new(&HTAPS_DDRX10_7TO1_EQ);
                        let f2 = FirConvolve::new(&HTAPS_8MHZ_7TO1_EQ);
                        let f3 = FirConvolve::new(&HTAPS_1MHZ_3TO1_EQ);
                        let fl1 = (HTAPS_DDRX10_7TO1_EQ.len() * 2) as u64;
                        let fl2 = (HTAPS_8MHZ_7TO1_EQ.len() * 2) as u64;
                        let fl3 = (HTAPS_1MHZ_3TO1_EQ.len() * 2) as u64;
                        (f1, f2, f3, fl1, fl2, fl3)
                    } else if l == 5 && target_rate_hz == 96_000 {
                        // 96k: DSD×5 -> /7 (DSDX5 taps), then /7 (2MHz), then /3 (288k)
                        let f1 = FirConvolve::new(&HTAPS_DSDX5_7TO1_EQ);
                        let f2 = FirConvolve::new(&HTAPS_2MHZ_7TO1_EQ);
                        let f3 = FirConvolve::new(&HTAPS_288K_3TO1_EQ);
                        let fl1 = (HTAPS_DSDX5_7TO1_EQ.len() * 2) as u64;
                        let fl2 = (HTAPS_2MHZ_7TO1_EQ.len() * 2) as u64;
                        let fl3 = (HTAPS_288K_3TO1_EQ.len() * 2) as u64;
                        (f1, f2, f3, fl1, fl2, fl3)
                    } else if l == 10 && target_rate_hz == 192_000 {
                        // 192k: DDR×5 -> 4MHz -> 576k taps (existing)
                        let f1 = FirConvolve::new(&HTAPS_DDRX5_7TO_1_EQ);
                        let f2 = FirConvolve::new(&HTAPS_4MHZ_7TO1_EQ);
                        let f3 = FirConvolve::new(&HTAPS_576K_3TO1_EQ);
                        let fl1 = (HTAPS_DDRX5_7TO_1_EQ.len() * 2) as u64;
                        let fl2 = (HTAPS_4MHZ_7TO1_EQ.len() * 2) as u64;
                        let fl3 = (HTAPS_576K_3TO1_EQ.len() * 2) as u64;
                        (f1, f2, f3, fl1, fl2, fl3)
                    } else {
                        // Default to 192k cascade for other L/M=147 requests
                        let f1 = FirConvolve::new(&HTAPS_DDRX5_7TO_1_EQ);
                        let f2 = FirConvolve::new(&HTAPS_4MHZ_7TO1_EQ);
                        let f3 = FirConvolve::new(&HTAPS_576K_3TO1_EQ);
                        let fl1 = (HTAPS_DDRX5_7TO_1_EQ.len() * 2) as u64;
                        let fl2 = (HTAPS_4MHZ_7TO1_EQ.len() * 2) as u64;
                        let fl3 = (HTAPS_576K_3TO1_EQ.len() * 2) as u64;
                        (f1, f2, f3, fl1, fl2, fl3)
                    };
                let delay1 = (full1 - 1) / 2;
                let delay2 = (full2 - 1) / 2;
                let delay3 = (full3 - 1) / 2;
                let s = Self {
                    fir1,
                    up_factor: l,
                    decim1: 7,
                    delay1,
                    ups_index: 0,
                    phase1: 0,
                    primed1: false,
                    fir2,
                    decim2: 7,
                    delay2,
                    mid_index2: 0,
                    phase2: 0,
                    primed2: false,
                    fir3,
                    decim3: 3,
                    delay3,
                    mid_index3: 0,
                    phase3: 0,
                    primed3: false,
                    poly294: None,
                };
                if verbose && print_config {
                    if dump_stage1 {
                        eprintln!(
                            "[DBG] Equiripple L/M path: L={} M=147 — STAGE1 DUMP (×L -> /7).",
                            l
                        );
                    } else {
                        let taps_label = if (l == 10 || l == 20) && target_rate_hz == 384_000 {
                            "384k (DDRx10 taps, 8MHz, 1MHz)"
                        } else if l == 5 && target_rate_hz == 96_000 {
                            "96k (DSD×5, 2MHz, 288k)"
                        } else {
                            "192k (DDR×5, 4MHz, 576k)"
                        };
                        eprintln!(
                            "[DBG] Equiripple L/M path: L={} M=147 — (×L -> /7 -> /7 -> /3) using {} taps.",
                            l, taps_label
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

            // Stage 2
            let y2 = self.fir2.process_sample(y1);
            let idx2 = self.mid_index2;
            self.mid_index2 += 1;
            if !self.primed2 {
                if idx2 >= self.delay2 {
                    self.primed2 = true;
                    self.phase2 = 0;
                }
                continue;
            }
            self.phase2 += 1;
            if self.phase2 != self.decim2 {
                continue;
            }
            self.phase2 = 0;

            // Stage 3
            let y3 = self.fir3.process_sample(y2);
            let idx3 = self.mid_index3;
            self.mid_index3 += 1;
            if !self.primed3 {
                if idx3 >= self.delay3 {
                    self.primed3 = true;
                    self.phase3 = 0;
                }
                continue;
            }
            self.phase3 += 1;
            if self.phase3 == self.decim3 {
                self.phase3 = 0;
                out = Some(y3);
            }
        }
        out
    }

    #[inline]
    pub fn output_latency_frames(&self, dump_stage1: bool) -> f64 {
        if let Some(poly) = self.poly294.as_ref() {
            return poly.output_latency_frames();
        }
        if dump_stage1 {
            return (self.delay1 as f64) / (self.decim1 as f64);
        }
        let l1 =
            (self.delay1 as f64) / (self.decim1 as f64 * self.decim2 as f64 * self.decim3 as f64);
        let l2 = (self.delay2 as f64) / (self.decim2 as f64 * self.decim3 as f64);
        let l3 = (self.delay3 as f64) / (self.decim3 as f64);
        l1 + l2 + l3
    }
}
