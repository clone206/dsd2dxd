#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stage1Mode {
    SlotSim,
    Accum,
}
use crate::filters::HTAPS_288K_3TO1_EQ;
use crate::filters::HTAPS_2MHZ_7TO1_EQ;
use crate::filters::HTAPS_1_34MHZ_7TO1_EQ;
use crate::filters::HTAPS_DSDX10_21TO1_EQ;
use crate::filters::HTAPS_DDRX10_21TO1_EQ;
use crate::filters::HTAPS_DSDX5_7TO1_EQ;
use crate::filters::HTAPS_2_68MHZ_7TO1_EQ;
use crate::filters::HTAPS_DDRX5_14TO1_EQ; // ADD first-stage half taps (5× up, 14:1 down)
use std::collections::HashMap;
use std::env;
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;
// no extra traits needed
use ringbuf::{traits::*, HeapCons, HeapProd, HeapRb};

// Global LUT cache for Stage1, shared across instances/channels.
// Keyed by (L, len(right_half), hash(contents of right_half)) to avoid rebuilding.
type Stage1LutTable = Vec<Vec<[f64; 256]>>;
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct Stage1LutKey {
    l: u32,
    n: u32,
    hash: u64,
}

static STAGE1_LUT_CACHE: OnceLock<Mutex<HashMap<Stage1LutKey, Arc<Stage1LutTable>>>> =
    OnceLock::new();
// f32 LUT variant removed; we always use f64 LUTs now.

// One-time config diagnostics guards
static ST1_DIAG_ONCE: OnceLock<()> = OnceLock::new();

// ------------------------------------------------------------------------------------
// Tuning knobs (via env) for chunk sizing in Stage1
// Sensible baked-in defaults (used when env vars are unset):
//   Stage1: TARGET≈96 groups, aligned to M (ALIGN=M)
// Stage1 env vars (override defaults):
//   DSD2DXD_STAGE1_CHUNK_MULT   (usize, default 0)   -> if >0, chunk = M * MULT
//   DSD2DXD_STAGE1_CHUNK_TARGET (usize, default 128) -> if >0, round up to multiple of M near TARGET
//   DSD2DXD_STAGE1_CHUNK_ALIGN  (usize, default 0)   -> if 0, ALIGN=M; else round chunk down to multiple of ALIGN

static ST1_MULT: OnceLock<usize> = OnceLock::new();
static ST1_ALIGN: OnceLock<usize> = OnceLock::new();
static ST1_TARGET: OnceLock<usize> = OnceLock::new();

// Stage1 threading controls (optional, default disabled)
// DSD2DXD_STAGE1_PAR_THREADS: fixed number of threads per output (default 0 = disabled unless AUTO)
// DSD2DXD_STAGE1_PAR_MIN_GROUPS: minimum LUT groups to enable threading (default 128)
// DSD2DXD_STAGE1_PAR_AUTO: if set to 1/true, auto threads = min(groups, hw_threads, PAR_MAX) when PAR_THREADS==0
// DSD2DXD_STAGE1_PAR_MAX: cap the auto thread count (default: unlimited)
static ST1_PAR_THREADS: OnceLock<usize> = OnceLock::new();
static ST1_PAR_MIN_GROUPS: OnceLock<usize> = OnceLock::new();
static ST1_PAR_AUTO: OnceLock<bool> = OnceLock::new();
static ST1_PAR_MAX: OnceLock<usize> = OnceLock::new();

// Stage1 LUT toggles removed: LUT is always enabled with f64 precision.
// Fixed-capacity LM staging buffers (overridable via env)
// DSD2DXD_S1TMP_CAP: capacity for Stage1->Stage2 buffer (default 131072 samples)
// DSD2DXD_S2TMP_CAP: capacity for Stage2->Stage3 buffer (default 32768 samples)
static S1TMP_CAP: OnceLock<usize> = OnceLock::new();
static S2TMP_CAP: OnceLock<usize> = OnceLock::new();
// Optional toggles to disable ring FIFOs and use linear temp buffers (A/B for diagnostics)
static RING_S1_ENABLE: OnceLock<bool> = OnceLock::new();
static RING_S2_ENABLE: OnceLock<bool> = OnceLock::new();

// Use ringbuf crate for SPSC ring buffers.

#[inline]
fn env_usize(name: &str, default: usize) -> usize {
    env::var(name)
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(default)
}

#[inline]
fn env_bool(name: &str, default: bool) -> bool {
    env::var(name)
        .ok()
        .map(|v| {
            let v = v.trim();
            v == "1"
                || v.eq_ignore_ascii_case("true")
                || v.eq_ignore_ascii_case("on")
                || v.eq_ignore_ascii_case("yes")
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
    if a == 0 {
        return x;
    }
    if x == 0 {
        return 0;
    }
    ((x + a - 1) / a) * a
}

#[inline]
fn round_down_to_multiple(x: usize, a: usize) -> usize {
    if a == 0 {
        return x;
    }
    (x / a) * a
}

#[inline]
fn get_stage1_chunk_params() -> (usize, usize, usize) {
    // Allow zeros as sentinels to enable baked defaults downstream (ALIGN=M, TARGET=128)
    let mult = *ST1_MULT.get_or_init(|| env_usize("DSD2DXD_STAGE1_CHUNK_MULT", 0));
    let align = *ST1_ALIGN.get_or_init(|| env_usize("DSD2DXD_STAGE1_CHUNK_ALIGN", 0));
    let target = *ST1_TARGET.get_or_init(|| env_usize("DSD2DXD_STAGE1_CHUNK_TARGET", 256));
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
    if chunk == 0 {
        chunk = m.max(1);
    }
    // Default ALIGN=M if unset
    if align == 0 {
        align = m.max(1);
    }
    if chunk > total_groups {
        chunk = total_groups;
    }
    // apply alignment by rounding down; keep within [min, total_groups]
    let min_allowed = align.min(total_groups).max(1);
    chunk = round_down_to_multiple(chunk, align)
        .max(min_allowed)
        .min(total_groups);
    chunk
}

// stage1_use_lut removed: LUT is always used

// (Polyphase decimator env toggles removed)

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

fn build_stage1_lut_from_right_half(right_half: &[f64], l: u32) -> Vec<Vec<[f64; 256]>> {
    // Reconstruct full symmetric taps and polyphase decomposition
    let mut full: Vec<f64> = right_half.iter().rev().cloned().collect();
    full.extend_from_slice(right_half);
    let mut phases: Vec<Vec<f64>> = vec![Vec::new(); l as usize];
    for (i, &c) in full.iter().enumerate() {
        phases[i % l as usize].push(c);
    }
    // Build 8-bit LUT per phase and per group of 8 taps (newest-first order)
    let mut out: Vec<Vec<[f64; 256]>> = Vec::with_capacity(phases.len());
    for taps in &phases {
        let n = taps.len();
        let groups = (n + 7) / 8;
        let mut phase_lut: Vec<[f64; 256]> = Vec::with_capacity(groups);
        for g in 0..groups {
            let base = g * 8;
            let mut coeffs: [f64; 8] = [0.0; 8];
            for j in 0..8 {
                let idx = base + j;
                if idx < n {
                    coeffs[j] = taps[idx];
                }
            }
            let mut group_lut = [0.0f64; 256];
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

fn get_or_build_stage1_lut(right_half: &[f64], l: u32) -> Arc<Vec<Vec<[f64; 256]>>> {
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

// f32 LUT builders/cache removed

// Direct-eval phase taps builder removed (always using LUT now)

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
    two_phase_lm147_384: Option<TwoPhaseLM147>,
    // Reusable internal SPSC ring buffers via ringbuf crate
    s1_prod: HeapProd<f64>,
    s1_cons: HeapCons<f64>,
    s2_prod: HeapProd<f64>,
    s2_cons: HeapCons<f64>,
    s2_scratch: Vec<f64>,
    // Reusable buffer for batching Stage1 outputs before pushing to s1 ring
    s1_scratch: Vec<f64>,
}
impl LMResampler {
    pub fn new(l: u32, m: i32, verbose: bool, out_rate: u32) -> Self {
        let ring_s1 = *RING_S1_ENABLE.get_or_init(|| env_bool("DSD2DXD_RING_S1", true));
        let ring_s2 = *RING_S2_ENABLE.get_or_init(|| env_bool("DSD2DXD_RING_S2", true));
        let s1_cap = *S1TMP_CAP.get_or_init(|| env_usize("DSD2DXD_S1TMP_CAP", 131_072));
        let s2_cap = *S2TMP_CAP.get_or_init(|| env_usize("DSD2DXD_S2TMP_CAP", 131_072));
        // Build ring buffers
        let s1_rb = HeapRb::<f64>::new(s1_cap.max(2));
        let (s1_prod, s1_cons) = s1_rb.split();
        let s2_rb = HeapRb::<f64>::new(s2_cap.max(2));
        let (s2_prod, s2_cons) = s2_rb.split();
        match m {
            294 => {
                // Original cascade Stage1 definitions
                let s1 = Stage1Poly::new(&HTAPS_DDRX5_14TO1_EQ, l, 14);
                let s = Self {
                    // Stage 2 (decimator by 7)
                    poly2: Some(DecimFIRSym::new_from_half(&HTAPS_2MHZ_7TO1_EQ, 7)),
                    // Stage 3 (decimator by 3)
                    poly3: Some(DecimFIRSym::new_from_half(&HTAPS_288K_3TO1_EQ, 3)),
                    two_phase_lm147_384: None,
                    stage1_poly: Some(s1),
                    s1_prod,
                    s1_cons,
                    s2_prod,
                    s2_cons,
                    s2_scratch: Vec::new(),
                    s1_scratch: Vec::new(),
                };
                if verbose {
                    eprintln!(
                            "[DBG] Equiripple L/M path: L={} M=294 — (×L -> /14 -> /7 -> /3) [Stage2/3 direct].",
                            l
                        );
                    eprintln!(
                        "[DBG] LM ring caps: s1_tmp_cap={} , s2_tmp_cap={} , ring_s1={}, ring_s2={}",
                        s1_cap, s2_cap, ring_s1, ring_s2
                    );
                }
                s
            }
            147 => {
                // DSD128 -> 384k two‑phase path (×10 -> /21 -> /7) DEFAULT for L=10
                if out_rate == 384_000 {
                    if verbose {
                        eprintln!("[DBG] Two-phase L={}/M=147 path enabled: (×L -> /21 (poly) -> /7) => 384k", l);
                    }
                    // Minimal placeholders (not used directly)
                    return Self {
                        poly2: None,
                        poly3: None,
                        two_phase_lm147_384: Some(TwoPhaseLM147::new(l, out_rate)),
                        stage1_poly: None,
                        s1_prod,
                        s1_cons,
                        s2_prod,
                        s2_cons,
                        s2_scratch: Vec::new(),
                        s1_scratch: Vec::new(),
                    };
                }

                if out_rate == 192_000 {
                    if verbose {
                        eprintln!("[DBG] Two-phase L={}/M=147 path enabled: (×L -> /21 (poly) -> /7) => 192k", l);
                    }
                    // Minimal placeholders (not used directly)
                    return Self {
                        poly2: None,
                        poly3: None,
                        two_phase_lm147_384: Some(TwoPhaseLM147::new(l, out_rate)),
                        stage1_poly: None,
                        s1_prod,
                        s1_cons,
                        s2_prod,
                        s2_cons,
                        s2_scratch: Vec::new(),
                        s1_scratch: Vec::new(),
                    };
                }

                // (Existing selection logic unchanged, just swap Stage2/3 construction)
                let (stage1_half, right2, right3, label) = if l == 5 && out_rate == 96_000 {
                    (
                        &HTAPS_DSDX5_7TO1_EQ[..],
                        &HTAPS_2MHZ_7TO1_EQ[..],
                        &HTAPS_288K_3TO1_EQ[..],
                        "96k (DSD×5, 2MHz, 288k)",
                    )
                } else {
                    panic!("Unsupported L={} M=147 out_rate={} for Stage1/2/3 selection", l, out_rate);
                };

                if verbose {
                    eprintln!(
                            "[DBG] Equiripple L/M path: L={} M=147 — (×L -> /7 -> /7 -> /3) using {} [Stage2/3 direct].",
                            l, label
                        );
                }
                let s1 = Stage1Poly::new(stage1_half, l, 7);
                let me = Self {
                    // Stage 2 (decimator by 7)
                    poly2: Some(DecimFIRSym::new_from_half(right2, 7)),
                    // Stage 3 (decimator by 3)
                    poly3: Some(DecimFIRSym::new_from_half(right3, 3)),
                    two_phase_lm147_384: None,
                    stage1_poly: Some(s1),
                    s1_prod,
                    s1_cons,
                    s2_prod,
                    s2_cons,
                    s2_scratch: Vec::new(),
                    s1_scratch: Vec::new(),
                };
                if verbose {
                    eprintln!(
                        "[DBG] LM ring caps: s1_tmp_cap={} , s2_tmp_cap={} , ring_s1={}, ring_s2={}",
                        s1_cap, s2_cap, ring_s1, ring_s2
                    );
                }
                return me;
            }
            _ => panic!("Unsupported L/M combination: L={} M={}", l, m),
        }
    }

    // Removed legacy per-byte path (push_byte_lm); block processing is the default

    // Block-style processing to reduce per-byte call overhead.
    // Returns number of PCM frames produced into `out`.
    #[inline(always)]
    pub fn process_bytes_lm(&mut self, bytes: &[u8], lsb_first: bool, out: &mut [f64]) -> usize {
        // Two-phase path delegates to specialized helper
        if let Some(tp) = self.two_phase_lm147_384.as_mut() {
            return tp.process_bytes(bytes, lsb_first, out);
        }
        let (Some(ref mut s1), Some(ref mut p2), Some(ref mut p3)) = (
            self.stage1_poly.as_mut(),
            self.poly2.as_mut(),
            self.poly3.as_mut(),
        ) else {
            return 0;
        };

        // If rings are disabled via env, fallback to linear block processing (A/B diagnostic)
        let ring_s1 = *RING_S1_ENABLE.get_or_init(|| env_bool("DSD2DXD_RING_S1", true));
        let ring_s2 = *RING_S2_ENABLE.get_or_init(|| env_bool("DSD2DXD_RING_S2", true));
        if !ring_s1 || !ring_s2 {
            // Stage 1: accumulate all y1 into a temporary linear buffer
            let mut y1: Vec<f64> = Vec::with_capacity(bytes.len().saturating_mul(2));
            for &byte in bytes {
                if lsb_first {
                    for b in 0..8 {
                        let bit = (byte >> b) & 1;
                        s1.push_all(bit, |v| y1.push(v));
                    }
                } else {
                    for b in (0..8).rev() {
                        let bit = (byte >> b) & 1;
                        s1.push_all(bit, |v| y1.push(v));
                    }
                }
            }
            // Stage 2 into temp
            let mut y2: Vec<f64> = vec![0.0; y1.len().saturating_add(8)];
            let n2 = p2.process_block(&y1, &mut y2);
            // Stage 3 into out
            return p3.process_block(&y2[..n2], out);
        }

        // Stream via ring FIFOs: Stage1 -> s1_ring -> Stage2 -> s2_ring -> Stage3 -> out
        let y1_per_byte_ub = ((8 * s1.l as usize) + (s1.m as usize) - 1) / (s1.m as usize);
        let y1_per_byte_ub = y1_per_byte_ub.max(1);
        let mut produced_total = 0usize;
        let mut i = 0usize;

        // Drain Stage3 (s2 -> out) as much as fits into out
        let drain_stage3 = |p3: &mut DecimFIRSym, out: &mut [f64], produced: &mut usize| {
            while !self.s2_cons.is_empty() && *produced < out.len() {
                let max_in = (out.len() - *produced) * p3.decim;
                if max_in == 0 {
                    break;
                }
                let (s2a, _s2b) = self.s2_cons.as_slices();
                if s2a.is_empty() {
                    break;
                }
                let take = core::cmp::min(s2a.len(), max_in);
                let n3 = p3.process_block(&s2a[..take], &mut out[*produced..]);
                unsafe {
                    self.s2_cons.advance_read_index(take);
                }
                *produced += n3;
                if n3 == 0 {
                    break;
                }
            }
        };

        // Move as much as possible from s1 -> s2 given current vacancies
        let mut bridge_s1_to_s2 = |p2: &mut DecimFIRSym| loop {
            if self.s1_cons.is_empty() {
                break;
            }
            let (vac_a, _vac_b) = self.s2_prod.vacant_slices();
            let first_run = vac_a.len();
            if first_run == 0 {
                break;
            }
            let (in1a, _in1b) = self.s1_cons.as_slices();
            if in1a.is_empty() {
                break;
            }
            let max_in_for_out2 = first_run.saturating_mul(p2.decim);
            let take_in = core::cmp::min(in1a.len(), max_in_for_out2);
            if take_in == 0 {
                break;
            }
            if self.s2_scratch.len() < first_run {
                self.s2_scratch.resize(first_run, 0.0);
            }
            let n2 = p2.process_block(&in1a[..take_in], &mut self.s2_scratch[..first_run]);
            unsafe {
                self.s1_cons.advance_read_index(take_in);
            }
            if n2 > 0 {
                let wrote = self.s2_prod.push_slice(&self.s2_scratch[..n2]);
                debug_assert_eq!(wrote, n2);
            }
            if n2 == 0 {
                break;
            }
        };

        while i < bytes.len() || !self.s1_cons.is_empty() || !self.s2_cons.is_empty() {
            if produced_total >= out.len() && i >= bytes.len() {
                break;
            }

            // Fill Stage1 ring in batches bounded by ring vacancy and worst-case outputs/byte
            if i < bytes.len() {
                let mut free = self.s1_prod.vacant_len();
                if free < y1_per_byte_ub {
                    // Free downstream and retry
                    bridge_s1_to_s2(p2);
                    drain_stage3(p3, out, &mut produced_total);
                    free = self.s1_prod.vacant_len();
                }
                let max_bytes = free / y1_per_byte_ub;
                if max_bytes > 0 {
                    let take = core::cmp::min(max_bytes, bytes.len() - i);
                    // Generate Stage1 outputs for this chunk into s1_scratch
                    self.s1_scratch.clear();
                    let target_cap = take.saturating_mul(y1_per_byte_ub);
                    if self.s1_scratch.capacity() < target_cap {
                        self.s1_scratch
                            .reserve(target_cap - self.s1_scratch.capacity());
                    }
                    for &byte in &bytes[i..i + take] {
                        if lsb_first {
                            for b in 0..8 {
                                let bit = (byte >> b) & 1;
                                s1.push_all(bit, |y1| self.s1_scratch.push(y1));
                            }
                        } else {
                            for b in (0..8).rev() {
                                let bit = (byte >> b) & 1;
                                s1.push_all(bit, |y1| self.s1_scratch.push(y1));
                            }
                        }
                    }
                    // Push all generated y1 in one go (fits by construction)
                    let wrote = self.s1_prod.push_slice(&self.s1_scratch);
                    debug_assert_eq!(wrote, self.s1_scratch.len());
                    i += take;
                }
            }

            // Bridge s1 -> s2 as far as possible, then drain s2 -> out
            bridge_s1_to_s2(p2);
            if produced_total < out.len() {
                drain_stage3(p3, out, &mut produced_total);
            }

            if i >= bytes.len() && self.s1_cons.is_empty() && self.s2_cons.is_empty() {
                break;
            }
        }
        produced_total
    }
}

// Unified two‑phase L∈{10,20} / (21*7) path for DSD64 or DSD128 -> 384 kHz
struct TwoPhaseLM147 {
    stage1: Stage1Poly,  // ×L /21 polyphase (L=10 or 20)
    stage2: DecimFIRSym, // /7 (2.688 MHz -> 384 kHz)
    // Reusable buffer for stage1 outputs as SPSC ring
    s1_prod: HeapProd<f64>,
    s1_cons: HeapCons<f64>,
    l: u32,
}

impl TwoPhaseLM147 {
    fn new(l: u32, out_rate: u32) -> Self {
        let s1_cap = *S1TMP_CAP.get_or_init(|| env_usize("DSD2DXD_S1TMP_CAP", 131_072));
        let rb = HeapRb::<f64>::new(s1_cap.max(2));
        let (s1_prod, s1_cons) = rb.split();
        // Select tap sets based on target output rate
        //  - 384k: use DDRX10_21TO1 first stage and 2.68MHz /7 second stage
        //  - 192k: use DSDX10_21TO1 first stage and 1.34MHz /7 second stage
        let (taps_stage1, taps_stage2) = if out_rate == 192_000 {
            (&HTAPS_DSDX10_21TO1_EQ[..], &HTAPS_1_34MHZ_7TO1_EQ[..])
        } else {
            (&HTAPS_DDRX10_21TO1_EQ[..], &HTAPS_2_68MHZ_7TO1_EQ[..])
        };
        Self {
            stage1: Stage1Poly::new(taps_stage1, l, 21),
            stage2: DecimFIRSym::new_from_half(taps_stage2, 7),
            s1_prod,
            s1_cons,
            l,
        }
    }

    // Removed legacy per-byte path (push_byte); block processing is the default

    // Block-style processing for two-phase (Stage1 /21 -> /7) path.
    #[inline]
    fn process_bytes(&mut self, bytes: &[u8], lsb_first: bool, out: &mut [f64]) -> usize {
        // Stream using ring FIFO for stage1 -> stage2 -> out
        let y1_per_byte_ub = ((8 * self.l as usize) + 21 - 1) / 21; // ceil(8*L/21)
        let y1_per_byte_ub = y1_per_byte_ub.max(1);
        let mut produced_total = 0usize;
        let mut i = 0usize;
        while (i < bytes.len()) || (!self.s1_cons.is_empty()) {
            if produced_total >= out.len() && i >= bytes.len() {
                break;
            }

            // Fill stage1 ring (with backpressure)
            if i < bytes.len() {
                let free = self.s1_prod.vacant_len();
                let max_bytes = free / y1_per_byte_ub;
                if max_bytes > 0 {
                    let take = core::cmp::min(max_bytes, bytes.len() - i);
                    let mut write_s1 = |y1: f64| {
                        if self.s1_prod.try_push(y1).is_err() {
                            // Free space by draining into out via stage2
                            let max_in =
                                (out.len() - produced_total).saturating_mul(self.stage2.decim);
                            if max_in > 0 {
                                let (in1a, _in1b) = self.s1_cons.as_slices();
                                if !in1a.is_empty() {
                                    let take_in = core::cmp::min(in1a.len(), max_in);
                                    let n = self.stage2.process_block(
                                        &in1a[..take_in],
                                        &mut out[produced_total..],
                                    );
                                    unsafe {
                                        self.s1_cons.advance_read_index(take_in);
                                    }
                                    produced_total += n;
                                }
                            }
                            // Retry once
                            let _ = self.s1_prod.try_push(y1);
                        }
                    };
                    for &byte in &bytes[i..i + take] {
                        if lsb_first {
                            for b in 0..8 {
                                let bit = (byte >> b) & 1;
                                self.stage1.push_all(bit, |y1| write_s1(y1));
                            }
                        } else {
                            for b in (0..8).rev() {
                                let bit = (byte >> b) & 1;
                                self.stage1.push_all(bit, |y1| write_s1(y1));
                            }
                        }
                    }
                    i += take;
                }
            }

            // Drain into out, bounded so outputs fit
            if produced_total < out.len() {
                let max_in = (out.len() - produced_total) * self.stage2.decim;
                let (in1a, _in1b) = self.s1_cons.as_slices();
                if !in1a.is_empty() {
                    let take_in = core::cmp::min(in1a.len(), max_in);
                    let n = self
                        .stage2
                        .process_block(&in1a[..take_in], &mut out[produced_total..]);
                    unsafe {
                        self.s1_cons.advance_read_index(take_in);
                    }
                    produced_total += n;
                } else if i >= bytes.len() {
                    break;
                }
            } else {
                break;
            }
        }
        produced_total
    }
}

// S1EvalMode removed: only LUT(f64) path remains

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
    // Precomputed LUTs (f64 only)
    lut_f64: Option<Arc<Vec<Vec<[f64; 256]>>>>,
    // Byte-oriented ring mirroring newest-first 8-bit windows per input bit
    byte_ring: Vec<u8>,
    byte_mask: usize,
    wbyte: usize,
    rolling_byte: u8,
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
        // Always use f64 LUTs
        let lut_f64: Option<Arc<Vec<Vec<[f64; 256]>>>> = Some(get_or_build_stage1_lut(right_half, l));
        // Determine maximum groups across phases to size the byte ring
        let mut groups_max = 0usize;
        if let Some(ref tbl) = lut_f64 {
            for phase in tbl.iter() {
                groups_max = groups_max.max(phase.len());
            }
        }
        let byte_cap = (groups_max + 16).next_power_of_two().max(32);
        let me = Self {
            l,
            m,
            ring_bits: vec![0u64; (cap + 63) / 64],
            bits_mask: cap - 1,
            wbits: 0,
            mode: if m == 21 {
                Stage1Mode::Accum
            } else {
                Stage1Mode::SlotSim
            },
            delay_high,
            high_index: 0,
            phase1: 0,
            lut_f64,
            byte_ring: vec![0u8; byte_cap],
            byte_mask: byte_cap - 1,
            wbyte: 0,
            rolling_byte: 0,
            acc: 0,
            phase_mod: 0,
            input_count: 0,
            input_delay,
            primed: false,
        };

        // One-time Stage1 diagnostics when env vars are set (prints to stderr, without -v)
        if ST1_DIAG_ONCE.set(()).is_ok()
            && any_env_present(&[
                "DSD2DXD_STAGE1_CHUNK_MULT",
                "DSD2DXD_STAGE1_CHUNK_TARGET",
                "DSD2DXD_STAGE1_CHUNK_ALIGN",
                "DSD2DXD_STAGE1_PAR_THREADS",
                "DSD2DXD_STAGE1_PAR_MIN_GROUPS",
                "DSD2DXD_STAGE1_PAR_AUTO",
                "DSD2DXD_STAGE1_PAR_MAX",
            ])
        {
            // Report groups/phase range and effective chunk + threading summary
            let (min_g, max_g) = if let Some(ref tbl) = me.lut_f64 {
                if tbl.is_empty() {
                    (0, 0)
                } else {
                    let mut min_g = usize::MAX;
                    let mut max_g = 0usize;
                    for phase in tbl.iter() {
                        let g = phase.len();
                        if g < min_g {
                            min_g = g;
                        }
                        if g > max_g {
                            max_g = g;
                        }
                    }
                    (min_g, max_g)
                }
            } else {
                (0, 0)
            };
            let (st1_mult, st1_align, st1_target) = get_stage1_chunk_params();
            let example_groups = max_g;
            let chunk_eff = if example_groups > 0 {
                compute_stage1_chunk(example_groups, m as usize, l)
            } else {
                0
            };
            let (threads_fixed, min_groups_thr) = Self::get_stage1_parallel_params();
            let auto = *ST1_PAR_AUTO.get_or_init(|| {
                env::var("DSD2DXD_STAGE1_PAR_AUTO")
                    .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
                    .unwrap_or(false)
            });
            let par_max =
                *ST1_PAR_MAX.get_or_init(|| env_usize("DSD2DXD_STAGE1_PAR_MAX", usize::MAX));
            let hw = thread::available_parallelism()
                .map(|n| n.get())
                .unwrap_or(1);
            let effective_threads = if threads_fixed > 0 {
                threads_fixed.min(example_groups).max(1)
            } else if auto {
                example_groups.min(hw).min(par_max).max(1)
            } else {
                0
            };
            let engaged = effective_threads > 0 && example_groups >= min_groups_thr;
            let par_max_str = if par_max == usize::MAX {
                "unlimited".to_string()
            } else {
                par_max.to_string()
            };
            eprintln!(
                "[CFG] Stage1 L={} M={}: groups/phase={}..{}, chunk_eff={} mode=lut(f64) (params: MULT={}, TARGET={}, ALIGN={} [0=>M])",
                l, m, min_g, max_g, chunk_eff, st1_mult, st1_target, st1_align
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
        // Update rolling byte (newest-first in LSB) and write to byte ring
        let b = if is_pos { 1u8 } else { 0u8 };
        self.rolling_byte = ((self.rolling_byte << 1) & 0xFF) | b;
        self.byte_ring[self.wbyte] = self.rolling_byte;
        self.wbyte = (self.wbyte + 1) & self.byte_mask;
    }

    // reverse8 was removed; not needed by the newest-first byte extractor.

    // Note: legacy byte extraction helpers removed (unused).

    #[inline(always)]
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
        let tbl = self.lut_f64.as_ref().unwrap();
        let sum = self.sum_phase_groups_threaded(&tbl[phase][..]);
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
            let tbl = self.lut_f64.as_ref().unwrap();
            let sum = self.sum_phase_groups_threaded(&tbl[phase][..]);
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
    fn sum_phase_groups_chunked(&self, phase_lut: &[[f64; 256]], chunk_bytes: usize) -> f64 {
        let mut sum = 0.0;
        let total_groups = phase_lut.len();
        if total_groups == 0 {
            return 0.0;
        }
        // Start at last written rolling byte (newest 8-bit window)
        let mut bidx = (self.wbyte + self.byte_mask) & self.byte_mask; // newest byte index (time t)
        let capb = self.byte_mask + 1;
        let mask = self.byte_mask;
        let mut g = 0usize;
        while g < total_groups {
            let this_chunk = core::cmp::min(chunk_bytes, total_groups - g);
            // Extract bytes for this chunk and accumulate (unrolled by 8 -> 4)
            let mut k = 0usize;
            // 8-way unroll with 4 accumulators
            while k + 8 <= this_chunk {
                let idx0 = (bidx + capb - ((k as usize) << 3)) & mask;
                let idx1 = (idx0 + capb - 8) & mask;
                let idx2 = (idx1 + capb - 8) & mask;
                let idx3 = (idx2 + capb - 8) & mask;
                let idx4 = (idx3 + capb - 8) & mask;
                let idx5 = (idx4 + capb - 8) & mask;
                let idx6 = (idx5 + capb - 8) & mask;
                let idx7 = (idx6 + capb - 8) & mask;

                let b0 = unsafe { *self.byte_ring.get_unchecked(idx0) } as usize;
                let b1 = unsafe { *self.byte_ring.get_unchecked(idx1) } as usize;
                let b2 = unsafe { *self.byte_ring.get_unchecked(idx2) } as usize;
                let b3 = unsafe { *self.byte_ring.get_unchecked(idx3) } as usize;
                let b4 = unsafe { *self.byte_ring.get_unchecked(idx4) } as usize;
                let b5 = unsafe { *self.byte_ring.get_unchecked(idx5) } as usize;
                let b6 = unsafe { *self.byte_ring.get_unchecked(idx6) } as usize;
                let b7 = unsafe { *self.byte_ring.get_unchecked(idx7) } as usize;

                let mut s0 = 0.0f64;
                let mut s1 = 0.0f64;
                let mut s2 = 0.0f64;
                let mut s3 = 0.0f64;
                unsafe {
                    s0 += *phase_lut.get_unchecked(g + k + 0).get_unchecked(b0);
                    s1 += *phase_lut.get_unchecked(g + k + 1).get_unchecked(b1);
                    s2 += *phase_lut.get_unchecked(g + k + 2).get_unchecked(b2);
                    s3 += *phase_lut.get_unchecked(g + k + 3).get_unchecked(b3);
                    s0 += *phase_lut.get_unchecked(g + k + 4).get_unchecked(b4);
                    s1 += *phase_lut.get_unchecked(g + k + 5).get_unchecked(b5);
                    s2 += *phase_lut.get_unchecked(g + k + 6).get_unchecked(b6);
                    s3 += *phase_lut.get_unchecked(g + k + 7).get_unchecked(b7);
                }
                sum += (s0 + s1) + (s2 + s3);
                k += 8;
            }
            // 4-way unroll with 4 accumulators
            while k + 4 <= this_chunk {
                let idx0 = (bidx + capb - ((k as usize) << 3)) & mask;
                let idx1 = (idx0 + capb - 8) & mask;
                let idx2 = (idx1 + capb - 8) & mask;
                let idx3 = (idx2 + capb - 8) & mask;
                let b0 = unsafe { *self.byte_ring.get_unchecked(idx0) } as usize;
                let b1 = unsafe { *self.byte_ring.get_unchecked(idx1) } as usize;
                let b2 = unsafe { *self.byte_ring.get_unchecked(idx2) } as usize;
                let b3 = unsafe { *self.byte_ring.get_unchecked(idx3) } as usize;
                let mut s0 = 0.0f64;
                let mut s1 = 0.0f64;
                let mut s2 = 0.0f64;
                let mut s3 = 0.0f64;
                unsafe {
                    s0 += *phase_lut.get_unchecked(g + k + 0).get_unchecked(b0);
                    s1 += *phase_lut.get_unchecked(g + k + 1).get_unchecked(b1);
                    s2 += *phase_lut.get_unchecked(g + k + 2).get_unchecked(b2);
                    s3 += *phase_lut.get_unchecked(g + k + 3).get_unchecked(b3);
                }
                sum += (s0 + s1) + (s2 + s3);
                k += 4;
            }
            while k < this_chunk {
                let idx = (bidx + capb - ((k as usize) << 3)) & mask;
                let byte = unsafe { *self.byte_ring.get_unchecked(idx) } as usize;
                unsafe { sum += *phase_lut.get_unchecked(g + k).get_unchecked(byte); }
                k += 1;
            }
            // Advance bidx by the number of groups processed in this chunk
            bidx = (bidx + capb - ((this_chunk as usize) << 3)) & mask;
            g += this_chunk;
        }
        sum
    }

    // Note: legacy static byte extractor removed (unused).

    #[inline]
    fn get_stage1_parallel_params() -> (usize, usize) {
        let threads = *ST1_PAR_THREADS.get_or_init(|| env_usize("DSD2DXD_STAGE1_PAR_THREADS", 0));
        let min_groups =
            *ST1_PAR_MIN_GROUPS.get_or_init(|| env_usize("DSD2DXD_STAGE1_PAR_MIN_GROUPS", 128));
        (threads, min_groups)
    }

    // Optional threaded summation of all groups using scoped threads. Enabled only when
    // DSD2DXD_STAGE1_PAR_THREADS > 0 and total_groups >= min_groups. For safer iteration,
    // we restrict to m==21 by default, matching the heavy Accum path.
    fn sum_phase_groups_threaded(&self, phase_lut: &[[f64; 256]]) -> f64 {
        let total_groups = phase_lut.len();
        if total_groups == 0 {
            return 0.0;
        }
        let (mut threads, min_groups) = Self::get_stage1_parallel_params();
        let auto = *ST1_PAR_AUTO.get_or_init(|| {
            env::var("DSD2DXD_STAGE1_PAR_AUTO")
                .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
                .unwrap_or(false)
        });
        let par_max = *ST1_PAR_MAX.get_or_init(|| env_usize("DSD2DXD_STAGE1_PAR_MAX", usize::MAX));

        if threads == 0 && auto {
            let hw = thread::available_parallelism()
                .map(|n| n.get())
                .unwrap_or(1);
            threads = total_groups.min(hw).min(par_max).max(1);
        }
        if threads == 0 || total_groups < min_groups {
            // Fallback to chunked path with baked defaults
            let chunk = compute_stage1_chunk(total_groups, self.m as usize, self.l);
            return self.sum_phase_groups_chunked(phase_lut, chunk);
        }

        let threads = threads.min(total_groups).max(1);
        let capb = self.byte_mask + 1;
        let bidx_base = (self.wbyte + self.byte_mask) & self.byte_mask; // index of newest byte (time t)
        let byte_ring = &self.byte_ring;
        let byte_mask = self.byte_mask;

        let mut partials = vec![0.0f64; threads];
        thread::scope(|scope| {
            for (ti, part) in partials.iter_mut().enumerate() {
                let start = (ti * total_groups) / threads;
                let end = ((ti + 1) * total_groups) / threads;
                if start >= end {
                    continue;
                }
                let phase_lut_ref = phase_lut;
                scope.spawn(move || {
                    let mut sum = 0.0f64;
                    // Starting index for group `start`: time t - 8*start
                    let mut idx = (bidx_base + capb - ((start as usize) << 3)) & byte_mask;
                    for g in start..end {
                        let byte = byte_ring[idx] as usize;
                        let group_lut = &phase_lut_ref[g];
                        sum += group_lut[byte];
                        idx = (idx + capb - 8) & byte_mask;
                    }
                    *part = sum;
                });
            }
        });
        partials.into_iter().sum()
    }
    // Direct-eval path removed; LUT is always used
}

// --- ====================================================================================
// ====================================================================================
// Lightweight decimator structure (D=7 or 3), direct symmetric convolution only
#[derive(Debug)]
struct DecimFIRSym {
    _full: Vec<f64>, // full symmetric taps
    _len: usize,
    _half: usize,
    _has_center: bool,
    center: usize,         // (len-1)/2, used for initial output schedule
    decim: usize,          // decimation factor D
    ring: Vec<f64>,
    mask: usize,
    w: usize,             // next write index
    count: usize,         // total samples seen
    next_out_t: usize,    // next t index at which an output is due
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
        // No polyphase decomposition: direct symmetric convolution
        let me = Self {
            _full: full,
            _len: len,
            _half: half,
            _has_center: has_center,
            center,
            decim,
            ring: vec![0.0; cap],
            mask: cap - 1,
            w: 0,
            count: 0,
            next_out_t: center,
        };
        me
    }

    // Removed legacy per-sample push(); use process_block for efficiency

    // Block processing: feed a slice of inputs and write produced outputs into `out`.
    // Returns number of output samples written.
    #[inline(always)]
    fn process_block(&mut self, input: &[f64], out: &mut [f64]) -> usize {
        let mut produced = 0usize;
        for &x in input {
            // Write newest sample
            self.ring[self.w] = x;
            self.w = (self.w + 1) & self.mask;
            let t = self.count;
            self.count += 1;

            // Produce only when we've reached the next scheduled output time
            if t != self.next_out_t {
                continue;
            }
            // Advance next output time for future samples
            self.next_out_t = self.next_out_t.saturating_add(self.decim);

            let acc = unsafe { self.convolve_direct() };
            if produced < out.len() {
                out[produced] = acc;
                produced += 1;
            }
        }
        produced
    }

    // ----- Convolution implementations -----

    // Direct full-rate symmetric FIR convolution (no polyphase decomposition).
    // Evaluates y[t] = sum_{k=0..len-1} h[k] * x[t - k] when push() determines an output is due.
    #[inline(always)]
    unsafe fn convolve_direct(&self) -> f64 {
        let cap = self.ring.len();
        let newest = (self.w + cap - 1) & self.mask; // index of x[t]
        let mut total = 0.0;

        // Pairwise sum using symmetry: for k in 0..half-1
        // y += h[k] * (x[t-k] + x[t-(len-1-k)])
        let len = self._len;
        let half = self._half;
        let has_center = self._has_center;
        let center = self.center;

        // Indices for front (newest side) and back (oldest side)
        let mut idx_front = newest;                              // x[t - 0]
        let mut idx_back = (newest + cap - (len - 1)) & self.mask; // x[t - (len-1)]

        let mut k = 0usize;
        while k < half {
            let c = *self._full.get_unchecked(k);
            let xf = *self.ring.get_unchecked(idx_front);
            let xb = *self.ring.get_unchecked(idx_back);
            total += c * (xf + xb);
            // advance indices
            idx_front = (idx_front + cap - 1) & self.mask; // older by 1
            idx_back = (idx_back + 1) & self.mask;         // newer by 1
            k += 1;
        }

        // Center tap (odd length)
        if has_center {
            let ic = (newest + cap - center) & self.mask;
            total += *self._full.get_unchecked(center) * *self.ring.get_unchecked(ic);
        }
        total
    }
}
