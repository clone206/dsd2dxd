use crate::audio_file::AudioFileFormat;
use crate::dither::Dither;
use crate::dsd2pcm::Dxd;
use crate::dsd2pcm::HTAPS_288K_3TO1_EQ;
use crate::dsd2pcm::HTAPS_2MHZ_7TO1_EQ; // NEW second-stage (7:1)
use crate::dsd2pcm::HTAPS_4MHZ_7TO1_EQ; // NEW: ~4.032 MHz -> /7
use crate::dsd2pcm::HTAPS_576K_3TO1_EQ; // NEW: 576 kHz -> /3 (final 192 kHz)
use crate::dsd2pcm::HTAPS_DDRX5_14TO1_EQ; // ADD first-stage half taps (5× up, 14:1 down)
use crate::dsd2pcm::HTAPS_DDRX5_7TO_1_EQ; // NEW: 10*DSD -> /7
use crate::dsd2pcm::HTAPS_DDR_64TO1_CHEB;
use crate::dsdin_sys::DSD_64_RATE;
use crate::fir_convolve::{BytePrecalcDecimator, FirConvolve}; // keep
use crate::input::InputContext;
use crate::output::OutputContext;
use std::error::Error;
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::Path;
use std::time::Instant;

// ADD (near top, after use statements)
const MAKEUP_GAIN_FRAC_PATH_14DB: f64 = 5.011_872_336_272_722; // 10^(14/20)

// ====================================================================================
pub struct ConversionContext {
    in_ctx: InputContext,
    out_ctx: OutputContext,
    dither: Dither,
    filt_type: char,
    dsd_data: Vec<u8>,
    float_data: Vec<f64>,
    pcm_data: Vec<u8>,
    dxds: Vec<Dxd>,
    clips: i32,
    last_samps_clipped_low: i32,
    last_samps_clipped_high: i32,
    verbose_mode: bool,
    // ADD: Optional Chebyshev 64:1 decimators (one per channel)
    cheb64_decims: Option<Vec<Cheb64Decimator>>,
    // Generalized equiripple L/M resamplers (covers 5/294, 5/147, 10/147)
    eq_lm_resamplers: Option<Vec<EquiLMResampler>>,
    // Debug flag: dump after first stage (×L -> /decim1), for any L/M
    lm_dump_stage1: bool,
    total_dsd_bytes_processed: u64, // ADD: accumulate total input DSD bytes read
    upsample_ratio: u32, // NEW: L in L/M fractional (zero‑stuff) paths; 1 for pure integer decim
    decim_ratio: i32,
}

// Chebyshev 64:1 decimator using BytePrecalcDecimator (byte-level table lookups)
struct Cheb64Decimator {
    fast: BytePrecalcDecimator,
}
impl Cheb64Decimator {
    fn new() -> Self {
        let fast = BytePrecalcDecimator::new(&HTAPS_DDR_64TO1_CHEB, 64)
            .expect("BytePrecalcDecimator init failed (64:1)");
        Self { fast }
    }
    #[inline]
    fn process_bytes(&mut self, bytes: &[u8], out: &mut [f64]) -> usize {
        self.fast.process_bytes(bytes, out)
    }
    #[inline]
    fn reset(&mut self) {
        self.fast.reset();
    }
}

// ====================================================================================
// Generalized equiripple L/M resampler covering:
//   - L=5,  M=294: (×5 -> /14) -> /7 -> /3  -> 96 kHz
//   - L=5,  M=147: (×5 -> /7)  -> /7 -> /3  -> 192 kHz
//   - L=10, M=147: (×10 -> /7) -> /7 -> /3  -> 384 kHz
// Stage1 dump now supported for any M: output after first stage (×L -> /decim1).
// ====================================================================================
struct EquiLMResampler {
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
    // Decimation denominator M (147 or 294) for info
    m_total: i32,
}

impl EquiLMResampler {
    fn new(l: u32, m: i32, verbose: bool, dump_stage1: bool, print_config: bool) -> Self {
        match m {
            294 => {
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
                    m_total: 294,
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
                // Note: first-stage taps here handle ×L -> /7 (used for both L=5 and L=10 paths).
                let fir1 = FirConvolve::new(&HTAPS_DDRX5_7TO_1_EQ);
                let fir2 = FirConvolve::new(&HTAPS_4MHZ_7TO1_EQ);
                let fir3 = FirConvolve::new(&HTAPS_576K_3TO1_EQ);
                let full1 = (HTAPS_DDRX5_7TO_1_EQ.len() * 2) as u64;
                let full2 = (HTAPS_4MHZ_7TO1_EQ.len() * 2) as u64;
                let full3 = (HTAPS_576K_3TO1_EQ.len() * 2) as u64;
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
                    m_total: 147,
                };
                if verbose && print_config {
                    if dump_stage1 {
                        eprintln!(
                            "[DBG] Equiripple L/M path: L={} M=147 — STAGE1 DUMP (×L -> /7).",
                            l
                        );
                    } else {
                        eprintln!(
                            "[DBG] Equiripple L/M path: L={} M=147 — (×L -> /7 -> /7 -> /3).",
                            l
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
    fn push_bit_lm(&mut self, bit: u8, dump_stage1: bool) -> Option<f64> {
        let bit_i8: i8 = if bit != 0 { 1 } else { -1 };
        for p in 0..self.up_factor {
            let xi8 = if p == 0 { bit_i8 } else { 0 };
            let y1 = self.fir1.process_sample_i8(xi8);
            let idx_up = self.ups_index;
            self.ups_index += 1;

            // Prime stage 1 delay
            if !self.primed1 {
                if idx_up >= self.delay1 {
                    self.primed1 = true;
                    self.phase1 = 0;
                }
                continue;
            }
            // Stage 1 decimation counter
            self.phase1 += 1;
            if self.phase1 != self.decim1 {
                continue;
            }
            self.phase1 = 0;

            // If dumping stage 1, return immediately at the boundary
            if dump_stage1 {
                return Some(y1);
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
                return Some(y3);
            }
        }
        None
    }
}

// ====================================================================================
// END replacement
// ====================================================================================

impl ConversionContext {
    pub fn new(
        in_ctx: InputContext,
        out_ctx: OutputContext,
        dither: Dither,
        filt_type: char,
        verbose_param: bool,
    ) -> Result<Self, Box<dyn Error>> {
        let dsd_bytes_per_chan = in_ctx.block_size as usize; // bytes per channel
        let channels = in_ctx.channels_num as usize;
        let bytes_per_sample = out_ctx.bytes_per_sample as usize;
        let lsb_first = in_ctx.lsbit_first != 0;
        let dsd_rate = in_ctx.dsd_rate;

        // Determine decimation ratio from desired PCM output rate (Hz).
        // For fractional cascade paths:
        //   96 kHz  -> decim 294 (5/294 path, DSD128 -> 96k)
        //   192 kHz -> decim 147 (5/147 path, DSD128 -> 192k)
        //   384 kHz -> decim 147 (10/147 path, DSD128 -> 384k)
        // Otherwise, fall back to deriving an integer ratio if possible, or default 64.
        let decim_ratio: i32 = if out_ctx.rate == 96_000 {
            294
        } else if out_ctx.rate == 192_000 || out_ctx.rate == 384_000 {
            147
        } else {
            // Attempt automatic integer decimation: base DSD rate = 2.8224e6 * input_rate
            let base = DSD_64_RATE * (in_ctx.dsd_rate as u32);
            if out_ctx.rate > 0 && base % (out_ctx.rate as u32) == 0 {
                (base / (out_ctx.rate as u32)).try_into().unwrap()
            } else {
                // Preserve previous behavior (common default integer path)
                64
            }
        };

        if verbose_param {
            eprintln!(
                "Selected decimation ratio: {} (requested output rate: {})",
                decim_ratio, out_ctx.rate
            );
        }

        // Determine upsample_ratio (L in L/M) based on decimation ratio and target output rate.
        // Logic:
        //   if decim_ratio < 147 -> L = 1 (pure integer decimation path)
        //   else if output_rate == 384000 -> L = 10 (10/147 fractional path)
        //   else -> L = 5 (5/294 or 5/147 fractional path)
        let upsample_ratio: u32 = if decim_ratio < 147 {
            1
        } else if out_ctx.rate == 384_000 {
            10
        } else {
            5
        };
        if verbose_param {
            eprintln!(
                "Computed upsample_ratio (L): {} (decim_ratio={}, output_rate={})",
                upsample_ratio, decim_ratio, out_ctx.rate
            );
        }

        // Decimated PCM samples per channel: 8 DSD bits per byte
        let pcm_samples_per_chan = (dsd_bytes_per_chan * 8) / decim_ratio as usize;
        let mut ctx = Self {
            in_ctx,
            out_ctx,
            dither,
            filt_type,
            // Input buffer: bytes_per_chan * channels
            dsd_data: vec![0; dsd_bytes_per_chan * channels],
            // Per-channel float buffer sized for one channel’s decimated output
            float_data: vec![0.0; pcm_samples_per_chan],
            // Interleaved PCM buffer for all channels
            pcm_data: vec![0; pcm_samples_per_chan * channels * bytes_per_sample],
            dxds: (0..channels)
                .map(|_| Dxd::new(filt_type, lsb_first, decim_ratio, dsd_rate))
                .collect::<Result<Vec<_>, _>>()?,
            clips: 0,
            last_samps_clipped_low: 0,
            last_samps_clipped_high: 0,
            verbose_mode: verbose_param,
            cheb64_decims: None,
            eq_lm_resamplers: None,
            lm_dump_stage1: false,
            total_dsd_bytes_processed: 0,
            upsample_ratio: upsample_ratio, // default; may be overridden below
            decim_ratio: decim_ratio,
        };

        // Enable equiripple L/M cascade path when requested (E) and supported ratios
        if ctx.filt_type == 'E'
            && ctx.in_ctx.dsd_rate == 2
            && (decim_ratio == 294 || decim_ratio == 147)
        {
            let ch = ctx.in_ctx.channels_num as usize;

            // Stage1 dump (env DSD2DXD_DUMP_STAGE1=1/true) generalized for any M
            let dump_stage1 = std::env::var("DSD2DXD_DUMP_STAGE1")
                .map(|v| {
                    let vl = v.to_ascii_lowercase();
                    vl == "1" || vl == "true" || vl == "yes" || vl == "on"
                })
                .unwrap_or(false);
            ctx.lm_dump_stage1 = dump_stage1;

            // Build one resampler per channel; print config once (chan 0)
            ctx.eq_lm_resamplers = Some(
                (0..ch)
                    .map(|i| {
                        EquiLMResampler::new(
                            ctx.upsample_ratio,
                            decim_ratio,
                            ctx.verbose_mode,
                            ctx.lm_dump_stage1,
                            i == 0,
                        )
                    })
                    .collect(),
            );

            // Apply ~+14 dB makeup after full cascades (not for stage1 dump)
            if !ctx.lm_dump_stage1 {
                ctx.out_ctx.scale_factor *= MAKEUP_GAIN_FRAC_PATH_14DB;
                if ctx.verbose_mode {
                    eprintln!(
                        "[DBG] Applied +14 dB makeup to scale_factor (L/M). New scale_factor = {}",
                        ctx.out_ctx.scale_factor
                    );
                }
            }
        }

        // Chebyshev 64:1 path
        if ctx.filt_type == 'C' && ctx.in_ctx.dsd_rate == 2 && decim_ratio == 64 {
            let ch = ctx.in_ctx.channels_num as usize;
            ctx.cheb64_decims = Some((0..ch).map(|_| Cheb64Decimator::new()).collect());
            if ctx.verbose_mode {
                eprintln!("[DBG] Chebyshev 64:1 direct FIR path enabled.");
            }
        }

        ctx.out_ctx.set_channels_num(ctx.in_ctx.channels_num);
        ctx.out_ctx.init_file()?;
        ctx.dither.init();

        Ok(ctx)
    }

    // Derive output path like the C++ writeFile(): basename + proper extension, or "output.xxx" for stdin
    fn derive_output_path(&self) -> String {
        let ext = match self.out_ctx.output.to_ascii_lowercase() {
            'w' => "wav",
            'a' => "aif",
            'f' => "flac",
            _ => "out",
        };
        if self.in_ctx.std_in {
            return format!("output.{}", ext);
        }
        let parent = self
            .in_ctx
            .parent_path
            .as_ref()
            .map(|p| p.as_path())
            .unwrap_or(Path::new(""));
        let stem = self
            .in_ctx
            .file_path
            .as_ref()
            .and_then(|p| p.file_stem())
            .and_then(|s| s.to_str())
            .unwrap_or("output");
        parent
            .join(format!("{}.{}", stem, ext))
            .to_string_lossy()
            .into_owned()
    }

    pub fn do_conversion(&mut self) -> Result<(), Box<dyn Error>> {
        self.check_conv()?;

        // Print configuration information
        eprintln!("\nConfiguration:");
        let input_path = if self.in_ctx.std_in {
            "stdin".to_string()
        } else {
            self.in_ctx
                .file_path
                .as_ref()
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or_default()
        };
        eprintln!("Input: {}", input_path);
        eprintln!(
            "Format: {}",
            if self.in_ctx.interleaved {
                "Interleaved"
            } else {
                "Planar"
            }
        );
        eprintln!("Channels: {}", self.in_ctx.channels_num);
        eprintln!("LSB First: {}", self.in_ctx.lsbit_first != 0);
        eprintln!(
            "DSD Rate: {}",
            if self.in_ctx.dsd_rate == 2 {
                "DSD128"
            } else {
                "DSD64"
            }
        );
        eprintln!(
            "Output Format: {} bit{}",
            self.out_ctx.bits,
            if self.out_ctx.output == 'f' {
                " float"
            } else {
                ""
            }
        );
        eprintln!(
            "Output Type: {}",
            match self.out_ctx.output {
                's' => "stdout",
                'w' => "wav",  // FIX print label
                'a' => "aiff", // FIX print label
                'f' => "flac", // FIX print label
                _ => "unknown",
            }
        );
        eprintln!("Decimation Ratio: {}", self.decim_ratio);
        eprintln!("Output Sample Rate: {} Hz", self.out_ctx.rate);
        eprintln!(
            "Filter Type: {}",
            match self.filt_type {
                'X' => "XLD",
                'D' => "Original",
                'E' => "Equiripple",
                'C' => "Chebyshev",
                _ => "Unknown",
            }
        );
        eprintln!(
            "Dither Type: {}",
            match self.dither.dither_type() {
                't' => "TPDF",
                'r' => "Rectangular",
                'n' => "NJAD",
                'f' => "Float",
                'x' => "None",
                _ => "Unknown",
            }
        );
        eprintln!("Block Size: {} bytes", self.in_ctx.block_size);
        eprintln!("Scaling Factor: {}", self.out_ctx.scale_factor);
        eprintln!("Upsample Ratio (L): {}", self.upsample_ratio);
        eprintln!("");

        // Process blocks
        let wall_start = Instant::now(); // ADD: start timer
        self.process_blocks()?;
        let elapsed = wall_start.elapsed(); // ADD: end timer

        // Save file for non-stdout outputs using a derived path (like C++)
        if self.out_ctx.output != 's' {
            let out_path = self.derive_output_path();
            match self.out_ctx.output.to_ascii_lowercase() {
                'w' => {
                    self.out_ctx
                        .save_and_print_file(&out_path, AudioFileFormat::Wave)?;
                }
                'a' => {
                    self.out_ctx
                        .save_and_print_file(&out_path, AudioFileFormat::Aiff)?;
                }
                _ => {}
            }
        }
        // Report timing & speed
        if self.total_dsd_bytes_processed > 0 {
            let channels = self.in_ctx.channels_num as u64;
            // Bytes per channel
            let bytes_per_chan = self.total_dsd_bytes_processed / channels;
            let bits_per_chan = bytes_per_chan * 8;
            let dsd_base_rate = (DSD_64_RATE as u64) * (self.in_ctx.dsd_rate as u64); // samples/sec per channel
            let audio_seconds = if dsd_base_rate > 0 {
                (bits_per_chan as f64) / (dsd_base_rate as f64)
            } else {
                0.0
            };
            let elapsed_sec = elapsed.as_secs_f64().max(1e-9);
            let speed = audio_seconds / elapsed_sec;
            // Format H:MM:SS for elapsed
            let total_secs = elapsed.as_secs();
            let h = total_secs / 3600;
            let m = (total_secs % 3600) / 60;
            let s = total_secs % 60;
            eprintln!(
                "Conversion Time: {:02}:{:02}:{:02}  (Speed: {:.2}x realtime)",
                h, m, s, speed
            );
        }

        Ok(())
    }

    // ADD: Produce one channel via Chebyshev 64:1 path into float_data
    fn process_cheb64_channel(
        &mut self,
        chan: usize,
        block_remaining: usize, // bytes per channel in this block
        dsd_chan_offset: usize, // starting byte offset for channel
        dsd_stride: isize,      // stride from InputContext
        pcm_frames_per_chan: usize,
    ) {
        if self.float_data.len() < pcm_frames_per_chan {
            return;
        }
        let Some(decims) = self.cheb64_decims.as_mut() else {
            return;
        };
        let dec = &mut decims[chan];
        let lsb_first = self.in_ctx.lsbit_first != 0;
        let stride = if dsd_stride >= 0 {
            dsd_stride as usize
        } else {
            0
        };

        // Gather this channel's bytes (apply bit-reversal if MSB-first)
        // (Alloc on stack if small; otherwise Vec)
        let mut chan_bytes = Vec::with_capacity(block_remaining);
        for i in 0..block_remaining {
            let byte_index = if stride == 0 {
                dsd_chan_offset + i
            } else {
                dsd_chan_offset + i * stride
            };
            if byte_index >= self.dsd_data.len() {
                break;
            }
            let b = self.dsd_data[byte_index];
            chan_bytes.push(if lsb_first { b } else { bit_reverse_u8(b) });
        }
        // Process in one shot
        let produced = dec.process_bytes(&chan_bytes, &mut self.float_data[..pcm_frames_per_chan]);
        if produced < pcm_frames_per_chan {
            self.float_data[produced..pcm_frames_per_chan].fill(0.0);
        }
    }

    // Unified L/M rational path channel processor
    fn process_eq_lm_channel(
        &mut self,
        chan: usize,
        block_remaining: usize,
        dsd_chan_offset: usize,
        dsd_stride: isize,
        buf_capacity: usize,
    ) -> usize {
        if self.float_data.len() < buf_capacity {
            return 0;
        }
        let Some(resamps) = self.eq_lm_resamplers.as_mut() else {
            return 0;
        };
        let rs = &mut resamps[chan];
        let lsb_first = self.in_ctx.lsbit_first != 0;
        let stride = if dsd_stride >= 0 {
            dsd_stride as usize
        } else {
            0
        };
        let mut produced = 0usize;
        for i in 0..block_remaining {
            if produced >= buf_capacity {
                break;
            }
            let byte_index = if stride == 0 {
                dsd_chan_offset + i
            } else {
                dsd_chan_offset + i * stride
            };
            if byte_index >= self.dsd_data.len() {
                break;
            }
            let byte = self.dsd_data[byte_index];
            // iterate bits in correct order
            if lsb_first {
                for b in 0..8 {
                    if produced >= buf_capacity { break; }
                    let bit = (byte >> b) & 1;
                    let got = rs.push_bit_lm(bit, self.lm_dump_stage1);
                    if let Some(y) = got {
                        self.float_data[produced] = y;
                        produced += 1;
                    }
                }
            } else {
                for b in (0..8).rev() {
                    if produced >= buf_capacity { break; }
                    let bit = (byte >> b) & 1;
                    let got = rs.push_bit_lm(bit, self.lm_dump_stage1);
                    if let Some(y) = got {
                        self.float_data[produced] = y;
                        produced += 1;
                    }
                }
            }
        }
        if produced < buf_capacity {
            self.float_data[produced..buf_capacity].fill(0.0);
        }
        produced
    }

    fn process_blocks(&mut self) -> Result<(), Box<dyn Error>> {
        let channels = self.in_ctx.channels_num as usize;
        let frame_size: usize = (self.in_ctx.block_size as usize) * channels;

        // Ensure input buffer can hold one full frame
        if self.dsd_data.len() < frame_size {
            self.dsd_data.resize(frame_size, 0);
        }

        // Open a unified reader (stdin or file), seeking if needed
        let reading_from_file = !self.in_ctx.std_in;
        let mut reader: Box<dyn Read> = if reading_from_file {
            let path = if let Some(p) = &self.in_ctx.file_path {
                p.clone()
            } else {
                std::path::PathBuf::from(&self.in_ctx.input)
            };
            let mut f = File::open(&path)?;
            if self.in_ctx.audio_pos > 0 {
                f.seek(SeekFrom::Start(self.in_ctx.audio_pos as u64))?;
            }
            Box::new(f)
        } else {
            Box::new(io::stdin().lock())
        };

        // Initialize bytes_remaining like C++
        let mut bytes_remaining: i64 = if reading_from_file {
            if self.in_ctx.audio_length > 0 {
                self.in_ctx.audio_length
            } else {
                frame_size as i64
            }
        } else {
            frame_size as i64
        };

        // Use InputContext’s precomputed per-channel offset and stride (match C++)
        let chan_offset_base: usize = self.in_ctx.dsd_chan_offset as usize;
        let dsd_stride: isize = self.in_ctx.dsd_stride as isize;

        loop {
            // Read only full frames from file; stdin always reads frame_size
            let to_read: usize = if reading_from_file {
                if bytes_remaining >= frame_size as i64 {
                    frame_size
                } else {
                    break;
                }
            } else {
                frame_size
            };
            if to_read == 0 {
                break;
            }

            // Make sure the buffer is big enough (defensive for future config changes)
            if self.dsd_data.len() < to_read {
                self.dsd_data.resize(to_read, 0);
            }

            // Read one frame identically for stdin and file
            let read_size: usize = match reader.read_exact(&mut self.dsd_data[..to_read]) {
                Ok(()) => to_read,
                Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => break,
                Err(e) => return Err(Box::new(e)),
            };
            // Track bytes processed (all channels)
            self.total_dsd_bytes_processed += read_size as u64;

            // Bytes per channel in this read
            let block_remaining: usize = read_size / channels;

            // Compute frames per channel for this block:
            // Normal path: floor(bits / decim_ratio)
            // 5/294 path: dynamic; we allocate a ceiling estimate + safety and then use actual produced count.
            let bits_in = block_remaining * 8;
            let (estimate_frames, is_rational_lm) = if self.eq_lm_resamplers.is_some() {
                let l = self.upsample_ratio as usize;
                if self.lm_dump_stage1 {
                    // Stage1 dump after first decimator (14 for M=294, 7 for M=147)
                    let d1 = if self.decim_ratio == 294 { 14 } else { 7 };
                    ((bits_in * l + (d1 - 1)) / d1, true) // ceil(bits*L/d1)
                } else {
                    let m = self.decim_ratio as usize; // 294 or 147
                    ((bits_in * l + (m - 1)) / m, true) // ceil(bits*L/M)
                }
            } else {
                ((bits_in) / (self.decim_ratio as usize), false)
            };
            // Add small safety (max +1) to avoid truncation at block boundaries.
            let buf_needed = estimate_frames + if is_rational_lm { 2 } else { 0 };
            if self.float_data.len() < buf_needed {
                self.float_data.resize(buf_needed, 0.0);
            } else {
                self.float_data[..buf_needed].fill(0.0);
            }

            // Track actual frames produced per channel (set by channel 0)
            let mut frames_used_per_chan = estimate_frames;

            // Ensure pcm_data large enough for potential stdout packing (interleaved)
            let max_block_bytes_needed =
                buf_needed * channels * (self.out_ctx.bytes_per_sample as usize);
            if self.out_ctx.output == 's' && self.pcm_data.len() < max_block_bytes_needed {
                self.pcm_data.resize(max_block_bytes_needed, 0);
            }

            // Per-channel processing loop (was missing causing compile error / stray brace)
            for chan in 0..channels {
                let dsd_chan_offset = chan * self.in_ctx.dsd_chan_offset as usize;

                if self.cheb64_decims.is_some() {
                    // Chebyshev path: fill float_data exactly like translate would
                    self.float_data[..estimate_frames].fill(0.0);
                    self.process_cheb64_channel(
                        chan,
                        block_remaining,
                        dsd_chan_offset,
                        self.in_ctx.dsd_stride as isize,
                        estimate_frames,
                    );
                    frames_used_per_chan = estimate_frames;
                } else if self.eq_lm_resamplers.is_some() {
                    let produced = self.process_eq_lm_channel(
                        chan,
                        block_remaining,
                        dsd_chan_offset,
                        self.in_ctx.dsd_stride as isize,
                        buf_needed,
                    );
                    if chan == 0 {
                        frames_used_per_chan = produced;
                    } else {
                        debug_assert_eq!(frames_used_per_chan, produced);
                    }
                } else if let Some(dxd) = self.dxds.get_mut(chan) {
                    dxd.translate(
                        block_remaining,
                        &self.dsd_data[dsd_chan_offset..],
                        self.in_ctx.dsd_stride as isize,
                        &mut self.float_data[..estimate_frames],
                        1,
                        self.decim_ratio,
                    )?;
                    frames_used_per_chan = estimate_frames;
                }

                // Output / packing per channel
                if self.out_ctx.output == 's' {
                    // Interleave into pcm_data
                    let mut pcm_pos = chan * self.out_ctx.bytes_per_sample as usize;
                    for s in 0..frames_used_per_chan {
                        let mut qin: f64 = self.float_data[s] * self.out_ctx.scale_factor;
                        self.dither.process_samp(&mut qin, chan);
                        let value = Self::my_round(qin) as i32;
                        let clamped = self.clamp_value(-8_388_608, value, 8_388_607);
                        let mut out_idx = pcm_pos;
                        self.write_int(
                            &mut out_idx,
                            clamped,
                            self.out_ctx.bytes_per_sample as usize,
                        );
                        pcm_pos += channels * (self.out_ctx.bytes_per_sample as usize);
                    }
                } else {
                    // File formats: push samples into AudioFile buffers
                    if self.out_ctx.bits == 32 {
                        for s in 0..frames_used_per_chan {
                            let q = self.float_data[s] * self.out_ctx.scale_factor;
                            self.out_ctx.push_samp(q as f32, chan);
                        }
                    } else {
                        for s in 0..frames_used_per_chan {
                            let mut qin: f64 = self.float_data[s] * self.out_ctx.scale_factor;
                            self.dither.process_samp(&mut qin, chan);
                            let value = Self::my_round(qin) as i32;
                            let clamped = self.clamp_value(-8_388_608, value, 8_388_607);
                            self.out_ctx.push_samp(clamped, chan);
                        }
                    }
                }
            } // end channel loop

            // Derive actual byte count (may differ from estimate in rational path)
            let pcm_block_bytes =
                frames_used_per_chan * channels * (self.out_ctx.bytes_per_sample as usize);

            if self.out_ctx.output == 's' && pcm_block_bytes > 0 {
                self.write_block(pcm_block_bytes)?;
            }

            // Decrement for file input using actual read size
            if reading_from_file {
                bytes_remaining -= read_size as i64;
                if bytes_remaining <= 0 {
                    break;
                }
            }
        } // end loop

        Ok(())
    }

    fn write_block(&mut self, pcm_bytes: usize) -> Result<(), Box<dyn Error>> {
        if pcm_bytes == 0 || pcm_bytes > self.pcm_data.len() {
            return Ok(());
        }

        // Only stdout; file formats are saved at end via AudioFile
        if self.out_ctx.output == 's' {
            io::stdout().write_all(&self.pcm_data[..pcm_bytes])?;
            io::stdout().flush()?;
        }
        Ok(())
    }

    // Remove the separate write_file method since we're now writing directly

    // Helper function for clip stats
    fn update_clip_stats(&mut self, low: bool, high: bool) {
        if low {
            if self.last_samps_clipped_low == 1 {
                self.clips += 1;
            }
            self.last_samps_clipped_low += 1;
        } else if high {
            if self.last_samps_clipped_high == 1 {
                self.clips += 1;
            }
            self.last_samps_clipped_high += 1;
        }
    }

    fn check_conv(&self) -> Result<(), Box<dyn Error>> {
        if self.in_ctx.dsd_rate == 2 && ![16, 32, 64, 147, 294].contains(&self.decim_ratio) {
            return Err(
                "Only decimation value of 16, 32, 64, 147, or 294 allowed with dsd128 input."
                    .into(),
            );
        } else if self.in_ctx.dsd_rate == 1 && ![8, 16, 32].contains(&self.decim_ratio) {
            return Err("Only decimation value of 8, 16, or 32 allowed with dsd64 input.".into());
        }
        if self.decim_ratio == 294 && !(self.in_ctx.dsd_rate == 2 && self.filt_type == 'E') {
            return Err(
                "294:1 decimation currently only supported for DSD128 with Equiripple filter."
                    .into(),
            );
        }
        if self.decim_ratio == 147 && !(self.in_ctx.dsd_rate == 2 && self.filt_type == 'E') {
            return Err(
                "147:1 decimation currently only supported for DSD128 with Equiripple filter."
                    .into(),
            );
        }
        Ok(())
    }

    fn my_round(x: f64) -> i64 {
        if x < 0.0 {
            (x - 0.5).floor() as i64
        } else {
            (x + 0.5).floor() as i64
        }
    }

    fn write_float(&mut self, offset: &mut usize, sample: f64) {
        // Convert to f32 and write in little-endian
        let bytes = (sample as f32).to_le_bytes();
        self.pcm_data[*offset..*offset + 4].copy_from_slice(&bytes);
        *offset += 4;
    }

    fn write_int(&mut self, offset: &mut usize, value: i32, bytes: usize) {
        if *offset + bytes > self.pcm_data.len() {
            return;
        }

        match bytes {
            3 => {
                let v = value as i32;
                self.pcm_data[*offset] = (v & 0xFF) as u8;
                self.pcm_data[*offset + 1] = ((v >> 8) & 0xFF) as u8;
                self.pcm_data[*offset + 2] = ((v >> 16) & 0xFF) as u8;
            }
            2 => {
                let v = value as i16;
                let b = v.to_le_bytes();
                self.pcm_data[*offset..*offset + 2].copy_from_slice(&b);
            }
            4 => {
                let b = (value as i32).to_le_bytes();
                self.pcm_data[*offset..*offset + 4].copy_from_slice(&b);
            }
            _ => return,
        }
        *offset += bytes;
    }

    // Add verbose method
    fn verbose(&self, msg: &str, show: bool) {
        if self.verbose_mode && show {
            eprintln!("{}", msg);
        }
    }

    // Make clamp a method to access self
    fn clamp_value(&mut self, min: i32, value: i32, max: i32) -> i32 {
        let mut result = value;
        if value < min {
            result = min;
            self.update_clip_stats(true, false);
        } else if value > max {
            result = max;
            self.update_clip_stats(false, true);
        }
        result
    }
}

// ---- ADD local bit reversal helper (LSB <-> MSB) if not already imported -------------
#[inline]
fn bit_reverse_u8(mut b: u8) -> u8 {
    b = (b & 0xF0) >> 4 | (b & 0x0F) << 4;
    b = (b & 0xCC) >> 2 | (b & 0x33) << 2;
    b = (b & 0xAA) >> 1 | (b & 0x55) << 1;
    b
}
