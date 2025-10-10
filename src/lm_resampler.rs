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
use std::env;
use std::thread;
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

// One-time config diagnostics guards
static ST1_DIAG_ONCE: OnceLock<()> = OnceLock::new();

// ------------------------------------------------------------------------------------
// Tuning knobs (via env) for chunk sizing in Stage1 and DecimFIRSym
// Sensible baked-in defaults (used when env vars are unset):
//   Stage1: TARGET≈128 groups, aligned to M (ALIGN=M)
//   Decimator: TARGET≈64 taps, aligned to D (ALIGN=decim)
// Stage1 env vars (override defaults):
//   DSD2DXD_STAGE1_CHUNK_MULT   (usize, default 0)   -> if >0, chunk = M * MULT
//   DSD2DXD_STAGE1_CHUNK_TARGET (usize, default 128) -> if >0, round up to multiple of M near TARGET
//   DSD2DXD_STAGE1_CHUNK_ALIGN  (usize, default 0)   -> if 0, ALIGN=M; else round chunk down to multiple of ALIGN
// Decimator env vars (override defaults):
//   DSD2DXD_DECIM_CHUNK_MULT    (usize, default 0)   -> if >0, chunk = decim * MULT
//   DSD2DXD_DECIM_CHUNK_TARGET  (usize, default 64)  -> if >0, round up to multiple of decim near TARGET
//   DSD2DXD_DECIM_CHUNK_ALIGN   (usize, default 0)   -> if 0, ALIGN=decim; else round chunk down to multiple of ALIGN

static ST1_MULT: OnceLock<usize> = OnceLock::new();
static ST1_ALIGN: OnceLock<usize> = OnceLock::new();
static ST1_TARGET: OnceLock<usize> = OnceLock::new();
static DEC_MULT: OnceLock<usize> = OnceLock::new();
static DEC_ALIGN: OnceLock<usize> = OnceLock::new();
static DEC_TARGET: OnceLock<usize> = OnceLock::new();

// Stage1 threading controls (optional, default disabled)
// DSD2DXD_STAGE1_PAR_THREADS: fixed number of threads per output (default 0 = disabled unless AUTO)
// DSD2DXD_STAGE1_PAR_MIN_GROUPS: minimum LUT groups to enable threading (default 128)
// DSD2DXD_STAGE1_PAR_AUTO: if set to 1/true, auto threads = min(groups, hw_threads, PAR_MAX) when PAR_THREADS==0
// DSD2DXD_STAGE1_PAR_MAX: cap the auto thread count (default: unlimited)
static ST1_PAR_THREADS: OnceLock<usize> = OnceLock::new();
static ST1_PAR_MIN_GROUPS: OnceLock<usize> = OnceLock::new();
static ST1_PAR_AUTO: OnceLock<bool> = OnceLock::new();
static ST1_PAR_MAX: OnceLock<usize> = OnceLock::new();

// Stage1 LUT usage toggle: allow disabling LUTs and using direct tap-by-tap evaluation.
// Env var: DSD2DXD_STAGE1_USE_LUT (true if unset)
static ST1_USE_LUT: OnceLock<Option<bool>> = OnceLock::new();

// Decimator polyphase toggle: allow disabling polyphase and using direct convolution.
// Env vars:
//   DSD2DXD_DECIM_USE_POLY   => global default (true if unset)
//   DSD2DXD_DECIM7_USE_POLY  => override for decim=7
//   DSD2DXD_DECIM3_USE_POLY  => override for decim=3
static DEC_USE_POLY_GLOBAL: OnceLock<Option<bool>> = OnceLock::new();
static DEC_USE_POLY_7: OnceLock<Option<bool>> = OnceLock::new();
static DEC_USE_POLY_3: OnceLock<Option<bool>> = OnceLock::new();

#[inline]
fn env_usize(name: &str, default: usize) -> usize {
    env::var(name).ok().and_then(|v| v.parse::<usize>().ok()).unwrap_or(default)
}

#[inline]
fn env_bool(name: &str, default: bool) -> bool {
    env::var(name)
        .ok()
        .map(|v| {
            let v = v.trim();
            v == "1" || v.eq_ignore_ascii_case("true") || v.eq_ignore_ascii_case("on") || v.eq_ignore_ascii_case("yes")
        })
        .unwrap_or(default)
}

#[inline]
fn env_present(name: &str) -> bool {
    env::var_os(name).is_some()
}

#[inline]
fn any_env_present(names: &[&str]) -> bool {
    names.iter().any(|n| env_present(n))
}

#[inline]
fn round_up_to_multiple(x: usize, a: usize) -> usize {
    if a == 0 { return x; }
    if x == 0 { return 0; }
    ((x + a - 1) / a) * a
}

#[inline]
fn round_down_to_multiple(x: usize, a: usize) -> usize {
    if a == 0 { return x; }
    (x / a) * a
}

#[inline]
fn get_stage1_chunk_params() -> (usize, usize, usize) {
    // Allow zeros as sentinels to enable baked defaults downstream (ALIGN=M, TARGET=128)
    let mult = *ST1_MULT.get_or_init(|| env_usize("DSD2DXD_STAGE1_CHUNK_MULT", 0));
    let align = *ST1_ALIGN.get_or_init(|| env_usize("DSD2DXD_STAGE1_CHUNK_ALIGN", 0));
    let target = *ST1_TARGET.get_or_init(|| env_usize("DSD2DXD_STAGE1_CHUNK_TARGET", 128));
    (mult, align, target)
}

#[inline]
fn compute_stage1_chunk(total_groups: usize, m: usize, _l: u32) -> usize {
    let (mult, mut align, target) = get_stage1_chunk_params();
    // Default chunk: round TARGET≈128 up to multiple of M
    let mut chunk = if target > 0 {
        round_up_to_multiple(target, m)
    } else if mult > 0 {
        m.saturating_mul(mult)
    } else {
        round_up_to_multiple(128, m)
    };
    if chunk == 0 { chunk = m.max(1); }
    // Default ALIGN=M if unset
    if align == 0 { align = m.max(1); }
    if chunk > total_groups { chunk = total_groups; }
    // apply alignment by rounding down; keep within [min, total_groups]
    let min_allowed = align.min(total_groups).max(1);
    chunk = round_down_to_multiple(chunk, align).max(min_allowed).min(total_groups);
    chunk
}

#[inline]
fn stage1_use_lut() -> bool {
    let opt = ST1_USE_LUT.get_or_init(|| {
        if env_present("DSD2DXD_STAGE1_USE_LUT") { Some(env_bool("DSD2DXD_STAGE1_USE_LUT", false)) } else { None }
    });
    opt.unwrap_or(false)
}

#[inline]
fn get_decim_chunk_params() -> (usize, usize, usize) {
    // Allow zeros as sentinels to enable baked defaults downstream (ALIGN=D, TARGET=64)
    let mult = *DEC_MULT.get_or_init(|| env_usize("DSD2DXD_DECIM_CHUNK_MULT", 0));
    let align = *DEC_ALIGN.get_or_init(|| env_usize("DSD2DXD_DECIM_CHUNK_ALIGN", 0));
    let target = *DEC_TARGET.get_or_init(|| env_usize("DSD2DXD_DECIM_CHUNK_TARGET", 64));
    (mult, align, target)
}

#[inline]
fn compute_decim_chunk_len(decim: usize) -> usize {
    let (mult, mut align, target) = get_decim_chunk_params();
    let mut chunk = if target > 0 {
        round_up_to_multiple(target, decim)
    } else if mult > 0 {
        decim.saturating_mul(mult)
    } else {
        round_up_to_multiple(64, decim)
    };
    if chunk == 0 { chunk = decim.max(1); }
    if align == 0 { align = decim.max(1); }
    // ensure reasonable minimum and apply alignment (round down)
    chunk = round_down_to_multiple(chunk, align).max(align).max(decim).min(usize::MAX / 2);
    chunk
}

#[inline]
fn decim_use_poly(decim: usize) -> bool {
    // Per-factor override takes precedence, then global, else default true
    match decim {
        7 => {
            let local = DEC_USE_POLY_7.get_or_init(|| {
                if env_present("DSD2DXD_DECIM7_USE_POLY") { Some(env_bool("DSD2DXD_DECIM7_USE_POLY", true)) } else { None }
            });
            if let Some(b) = *local { return b; }
        }
        3 => {
            let local = DEC_USE_POLY_3.get_or_init(|| {
                if env_present("DSD2DXD_DECIM3_USE_POLY") { Some(env_bool("DSD2DXD_DECIM3_USE_POLY", true)) } else { None }
            });
            if let Some(b) = *local { return b; }
        }
        _ => {}
    }
    let global = DEC_USE_POLY_GLOBAL.get_or_init(|| {
        if env_present("DSD2DXD_DECIM_USE_POLY") { Some(env_bool("DSD2DXD_DECIM_USE_POLY", true)) } else { None }
    });
    global.unwrap_or(true)
}

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

fn build_stage1_phases_from_right_half(right_half: &[f64], l: u32) -> Vec<Vec<f64>> {
    // Reconstruct full symmetric taps and polyphase decomposition
    let mut full: Vec<f64> = right_half.iter().rev().cloned().collect();
    full.extend_from_slice(right_half);
    let mut phases: Vec<Vec<f64>> = vec![Vec::new(); l as usize];
    for (i, &c) in full.iter().enumerate() {
        phases[i % l as usize].push(c);
    }
    phases
}

// Toggle slow-domain Stage1 polyphase (debug / diagnostic).
// Set to true to enable Stage1PolySlow path; keep false for production.
// Slow-domain Stage1 polyphase is always active for L=5 now; legacy toggle removed.

// ====================================================================================
// Generalized equiripple L/M resampler covering:
//   - L=5,  M=294: (×5 -> /14) -> /7 -> /3  -> 96 kHz
//   - L=5,  M=147: (×5 -> /7)  -> /7 -> /3  -> 192 kHz
//   - L=10, M=147: (×10 -> /7) -> /7 -> /3  -> 384 kHz
//  ...
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
    // Direct evaluation taps per phase (same order as LUT grouping, newest-first)
    phase_taps: Vec<Vec<f64>>,
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
        let phase_taps = build_stage1_phases_from_right_half(right_half, l);
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
            phase_taps,
            acc: 0,
            phase_mod: 0,
            input_count: 0,
            input_delay,
            primed: false,
        };

        // One-time Stage1 diagnostics when env vars are set (prints to stderr, without -v)
        if ST1_DIAG_ONCE.set(()).is_ok() && any_env_present(&[
            "DSD2DXD_STAGE1_CHUNK_MULT",
            "DSD2DXD_STAGE1_CHUNK_TARGET",
            "DSD2DXD_STAGE1_CHUNK_ALIGN",
            "DSD2DXD_STAGE1_PAR_THREADS",
            "DSD2DXD_STAGE1_PAR_MIN_GROUPS",
            "DSD2DXD_STAGE1_PAR_AUTO",
            "DSD2DXD_STAGE1_PAR_MAX",
            "DSD2DXD_STAGE1_USE_LUT",
        ]) {
            // Report groups/phase range and effective chunk + threading summary
            let lut_ref = me.lut.as_ref();
            let (min_g, max_g) = if lut_ref.is_empty() {
                (0usize, 0usize)
            } else {
                let mut min_g = usize::MAX;
                let mut max_g = 0usize;
                for phase in lut_ref.iter() {
                    let g = phase.len();
                    if g < min_g { min_g = g; }
                    if g > max_g { max_g = g; }
                }
                (min_g, max_g)
            };
            let (st1_mult, st1_align, st1_target) = get_stage1_chunk_params();
            let example_groups = max_g;
            let chunk_eff = if example_groups > 0 {
                compute_stage1_chunk(example_groups, m as usize, l)
            } else { 0 };
            let (threads_fixed, min_groups_thr) = Self::get_stage1_parallel_params();
            let auto = *ST1_PAR_AUTO.get_or_init(|| env::var("DSD2DXD_STAGE1_PAR_AUTO").map(|v| v=="1"||v.eq_ignore_ascii_case("true")).unwrap_or(false));
            let par_max = *ST1_PAR_MAX.get_or_init(|| env_usize("DSD2DXD_STAGE1_PAR_MAX", usize::MAX));
            let hw = thread::available_parallelism().map(|n| n.get()).unwrap_or(1);
            let effective_threads = if threads_fixed > 0 {
                threads_fixed.min(example_groups).max(1)
            } else if auto {
                example_groups.min(hw).min(par_max).max(1)
            } else { 0 };
            let engaged = effective_threads > 0 && example_groups >= min_groups_thr;
            let par_max_str = if par_max == usize::MAX { "unlimited".to_string() } else { par_max.to_string() };
            let mode = if stage1_use_lut() { "lut" } else { "direct" };
            eprintln!(
                "[CFG] Stage1 L={} M={}: groups/phase={}..{}, chunk_eff={} mode={} (params: MULT={}, TARGET={}, ALIGN={} [0=>M])",
                l, m, min_g, max_g, chunk_eff, mode, st1_mult, st1_target, st1_align
            );
            eprintln!(
                "[CFG] Stage1 threading: fixed={}, auto={}, hw={}, min_groups={}, max={}, effective_threads_for_groups={}, engaged={}",
                threads_fixed, auto, hw, min_groups_thr, par_max_str, effective_threads, engaged
            );
        }
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

    // reverse8 was removed; not needed by the newest-first byte extractor.

    // Build a byte from 8 consecutive bits ending at start_idx in newest-first order.
    // Bit k corresponds to the sign at (start_idx - k) modulo ring capacity.
    #[inline]
    fn byte_from_bits_newest_first(&self, start_idx: usize) -> u8 {
        let mut b: u8 = 0;
        let mut idx = start_idx;
        for k in 0..8u8 {
            let word = idx >> 6;
            let bit = idx & 63;
            let one = ((self.ring_bits[word] >> bit) & 1) as u8;
            b |= one << k; // bit k is the sample at (start_idx - k)
            // subtract 1 modulo capacity: (idx - 1) & mask == (idx + mask) & mask
            idx = (idx + self.bits_mask) & self.bits_mask;
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
        // Compute using LUT or direct taps based on env toggle
        let sum = if stage1_use_lut() {
            let phase_lut = &self.lut[phase];
            self.sum_phase_groups_threaded(phase_lut)
        } else {
            self.sum_phase_direct(phase)
        };
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
            // Use LUTs or direct taps based on env toggle
            let sum = if stage1_use_lut() {
                let phase_lut = &self.lut[phase];
                self.sum_phase_groups_threaded(phase_lut)
            } else {
                self.sum_phase_direct(phase)
            };
            emit(sum);
            self.phase_mod = (self.phase_mod + (self.m % self.l)) % self.l;
        }
    }
}

impl Stage1Poly {
    // Sum all phase groups using the bit-packed ring and per-group LUTs, but
    // process groups in chunks of up to `chunk_bytes` to reduce loop overhead
    // and improve locality of byte extraction. Behavior is identical to the
    // simple per-group loop.
    #[inline]
    fn sum_phase_groups_chunked(&self, phase_lut: &Vec<Vec<f64>>, chunk_bytes: usize) -> f64 {
        let mut sum = 0.0;
        let total_groups = phase_lut.len();
        if total_groups == 0 {
            return 0.0;
        }
        let mut bidx = (self.wbits + self.bits_mask) & self.bits_mask; // newest bit index
        let capb = self.bits_mask + 1;
        let mut g = 0usize;
        while g < total_groups {
            let this_chunk = core::cmp::min(chunk_bytes, total_groups - g);
            // Extract bytes for this chunk and accumulate
            let mut k = 0usize;
            while k < this_chunk {
                // For group (g + k), the head bit index is bidx - 8*(k)
                let idx = (bidx + capb - ((k as usize) << 3)) & self.bits_mask;
                let byte = self.byte_from_bits_newest_first(idx);
                let group_lut = &phase_lut[g + k];
                sum += group_lut[byte as usize];
                k += 1;
            }
            // Advance bidx by the number of groups processed in this chunk
            bidx = (bidx + capb - ((this_chunk as usize) << 3)) & self.bits_mask;
            g += this_chunk;
        }
        sum
    }

    #[inline]
    fn byte_from_bits_newest_first_static(ring_bits: &[u64], bits_mask: usize, start_idx: usize) -> u8 {
        let mut b: u8 = 0;
        let mut idx = start_idx;
        for k in 0..8u8 {
            let word = idx >> 6;
            let bit = idx & 63;
            let one = ((ring_bits[word] >> bit) & 1) as u8;
            b |= one << k;
            idx = (idx + bits_mask) & bits_mask;
        }
        b
    }

    #[inline]
    fn get_stage1_parallel_params() -> (usize, usize) {
        let threads = *ST1_PAR_THREADS.get_or_init(|| env_usize("DSD2DXD_STAGE1_PAR_THREADS", 0));
        let min_groups = *ST1_PAR_MIN_GROUPS.get_or_init(|| env_usize("DSD2DXD_STAGE1_PAR_MIN_GROUPS", 128));
        (threads, min_groups)
    }

    // Optional threaded summation of all groups using scoped threads. Enabled only when
    // DSD2DXD_STAGE1_PAR_THREADS > 0 and total_groups >= min_groups. For safer iteration,
    // we restrict to m==21 by default, matching the heavy Accum path.
    fn sum_phase_groups_threaded(&self, phase_lut: &Vec<Vec<f64>>) -> f64 {
        let total_groups = phase_lut.len();
        if total_groups == 0 {
            return 0.0;
        }
        let (mut threads, min_groups) = Self::get_stage1_parallel_params();
        let auto = *ST1_PAR_AUTO.get_or_init(|| env::var("DSD2DXD_STAGE1_PAR_AUTO").map(|v| v=="1"||v.eq_ignore_ascii_case("true")).unwrap_or(false));
        let par_max = *ST1_PAR_MAX.get_or_init(|| env_usize("DSD2DXD_STAGE1_PAR_MAX", usize::MAX));

        if threads == 0 && auto {
            let hw = thread::available_parallelism().map(|n| n.get()).unwrap_or(1);
            threads = total_groups.min(hw).min(par_max).max(1);
        }
        if threads == 0 || total_groups < min_groups {
            // Fallback to chunked path with baked defaults
            let chunk = compute_stage1_chunk(total_groups, self.m as usize, self.l);
            return self.sum_phase_groups_chunked(phase_lut, chunk);
        }

        let threads = threads.min(total_groups).max(1);
        let capb = self.bits_mask + 1;
        let bidx_base = (self.wbits + self.bits_mask) & self.bits_mask;
        let ring_bits = &self.ring_bits;
        let bits_mask = self.bits_mask;

        let mut partials = vec![0.0f64; threads];
        thread::scope(|scope| {
            for (ti, part) in partials.iter_mut().enumerate() {
                let start = (ti * total_groups) / threads;
                let end = ((ti + 1) * total_groups) / threads;
                if start >= end { continue; }
                let phase_lut_ref = phase_lut;
                let ring_bits_ref = ring_bits;
                scope.spawn(move || {
                    let mut sum = 0.0f64;
                    // Starting index for group `start`: bidx_base - 8*start
                    let mut idx = (bidx_base + capb - ((start as usize) << 3)) & bits_mask;
                    for g in start..end {
                        let byte = Self::byte_from_bits_newest_first_static(ring_bits_ref, bits_mask, idx);
                        let group_lut = &phase_lut_ref[g];
                        sum += group_lut[byte as usize];
                        idx = (idx + capb - 8) & bits_mask;
                    }
                    *part = sum;
                });
            }
        });
        partials.into_iter().sum()
    }

    // Direct evaluation of the active phase using per-tap signs from the bit ring.
    // Equivalent to LUT path but without lookup tables.
    #[inline]
    fn sum_phase_direct(&self, phase: usize) -> f64 {
        let taps = &self.phase_taps[phase];
        if taps.is_empty() { return 0.0; }
        let mut sum = 0.0f64;
        let mut idx = (self.wbits + self.bits_mask) & self.bits_mask; // newest bit index
        let capb = self.bits_mask + 1;
        for &c in taps {
            let word = idx >> 6;
            let bit = idx & 63;
            let one = (self.ring_bits[word] >> bit) & 1;
            let sign = if one == 1 { 1.0 } else { -1.0 };
            sum += c * sign;
            idx = (idx + capb - 1) & self.bits_mask; // move to older bit
        }
        sum
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
        let me = Self {
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
        };

        // One-time decimator diagnostics per decim factor when env vars are set
        if any_env_present(&[
            "DSD2DXD_DECIM_CHUNK_MULT",
            "DSD2DXD_DECIM_CHUNK_TARGET",
            "DSD2DXD_DECIM_CHUNK_ALIGN",
            "DSD2DXD_DECIM_USE_POLY",
            "DSD2DXD_DECIM7_USE_POLY",
            "DSD2DXD_DECIM3_USE_POLY",
        ]) {
            static DECIM_DIAG_ONCE: OnceLock<Mutex<HashMap<usize, bool>>> = OnceLock::new();
            let printed_map = DECIM_DIAG_ONCE.get_or_init(|| Mutex::new(HashMap::new()));
            let mut guard = printed_map.lock().unwrap();
            if !guard.contains_key(&decim) {
                let (mult, align, target) = get_decim_chunk_params();
                let chunk_eff = compute_decim_chunk_len(decim);
                let use_poly = decim_use_poly(decim);
                eprintln!(
                    "[CFG] Decimator D={}: taps={} center={} chunk_eff={} mode={} (params: MULT={}, TARGET={}, ALIGN={} [0=>D])",
                    decim, len, center, chunk_eff, if use_poly {"polyphase"} else {"direct"}, mult, target, align
                );
                guard.insert(decim, true);
            }
        }
        me
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

        // Choose convolution path based on env toggle
        let acc = if decim_use_poly(self.decim) {
            // Use chunked polyphase convolution to improve cache/locality of ring accesses.
            let chunk_len = compute_decim_chunk_len(self.decim);
            unsafe { self.convolve_polyphase_chunked(chunk_len) }
        } else {
            unsafe { self.convolve_direct() }
        };
        Some(acc)
    }

    // ----- Convolution implementations -----
    #[inline(always)]
    #[allow(dead_code)]
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

    // Chunked variant of the polyphase convolution. Processes inner tap loops in
    // chunks to reduce loop overhead and improve index arithmetic locality.
    #[inline(always)]
    unsafe fn convolve_polyphase_chunked(&self, chunk_len: usize) -> f64 {
        let newest = (self.w + self.ring.len() - 1) & self.mask; // index of x[t]
        let mut total = 0.0;
        let cap = self.ring.len();
        for (p, phase_taps) in self.phases.iter().enumerate() {
            let n = phase_taps.len();
            if n == 0 { continue; }
            let idx0 = (newest + cap - p) & self.mask; // x[t - p]
            let mut k = 0usize;
            while k < n {
                let take = core::cmp::min(chunk_len, n - k);
                // Compute starting index for this chunk (k-th tap)
                let mut idx = (idx0 + cap - (self.decim * k) % cap) & self.mask;
                // Accumulate this chunk
                let end = k + take;
                while k < end {
                    let c = *phase_taps.get_unchecked(k);
                    total += c * *self.ring.get_unchecked(idx);
                    idx = (idx + cap - self.decim) & self.mask;
                    k += 1;
                }
            }
        }
        total
    }

    // Direct full-rate symmetric FIR convolution (no polyphase decomposition).
    // Evaluates y[t] = sum_{k=0..len-1} h[k] * x[t - k] when push() determines an output is due.
    #[inline(always)]
    unsafe fn convolve_direct(&self) -> f64 {
        let newest = (self.w + self.ring.len() - 1) & self.mask; // index of x[t]
        let mut total = 0.0;
        let mut idx = newest;
        for &c in &self._full {
            total += c * *self.ring.get_unchecked(idx);
            idx = (idx + self.ring.len() - 1) & self.mask;
        }
        total
    }
}
