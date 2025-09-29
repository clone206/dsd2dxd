use crate::audio_file::AudioFileFormat;
use crate::byte_precalc_decimator::bit_reverse_u8;
use crate::byte_precalc_decimator::BytePrecalcDecimator;
use crate::dither::Dither;
use crate::dsd2pcm::Dxd;
// NEW: 576 kHz -> /3 (final 192 kHz)
use crate::dsd2pcm::HTAPS_DDR_64TO1_CHEB;
use crate::dsd2pcm::HTAPS_DDR_64TO1_EQ;
use crate::dsd2pcm::HTAPS_DSD64_32TO1_EQ; // NEW: Equiripple 32:1 half taps for DSD64 -> 88.2 kHz
use crate::dsdin_sys::DSD_64_RATE;
use crate::input::InputContext;
use crate::lm_resampler::EquiLMResampler;
use crate::output::OutputContext;
use std::error::Error;
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::Path;
use std::time::Instant;

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
    // Unified precomputed integer decimator (one active variant):
    //   precalc_ratio = 32  -> DSD64  eq 32:1
    //   precalc_ratio = 64  -> DSD128 cheb or eq 64:1
    precalc_decims: Option<Vec<BytePrecalcDecimator>>,
    // Generalized equiripple L/M resamplers (covers 5/294, 5/147, 10/147)
    eq_lm_resamplers: Option<Vec<EquiLMResampler>>,
    // Debug flag: dump after first stage (×L -> /decim1), for any L/M
    lm_dump_stage1: bool,
    total_dsd_bytes_processed: u64, // ADD: accumulate total input DSD bytes read
    upsample_ratio: u32, // NEW: L in L/M fractional (zero‑stuff) paths; 1 for pure integer decim
    decim_ratio: i32,
    // Diagnostics
    diag_bits_in: u64,               // total DSD input bits seen
    diag_expected_frames_floor: u64, // floor(bits * L / M)
    diag_frames_out: u64,            // actual PCM frames produced (per channel count)
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

        let (decim_ratio, upsample_ratio) = Self::compute_decim_and_upsample(&in_ctx, &out_ctx);
        if verbose_param {
            eprintln!(
                "Selected decimation ratio (M): {} (requested output rate: {})",
                decim_ratio, out_ctx.rate
            );
            eprintln!(
                "Computed upsample ratio (L): {} (M={}, output_rate={})",
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
            precalc_decims: None,
            eq_lm_resamplers: None,
            lm_dump_stage1: false,
            total_dsd_bytes_processed: 0,
            upsample_ratio: upsample_ratio,
            decim_ratio: decim_ratio,
            diag_bits_in: 0,
            diag_expected_frames_floor: 0,
            diag_frames_out: 0,
        };

        // Enable equiripple L/M cascade path when requested (E) and supported ratios
        if ctx.filt_type == 'E'
            && ctx.in_ctx.dsd_rate >= 1
            && (decim_ratio == 294 || decim_ratio == 147)
        {
            let ch = ctx.in_ctx.channels_num as usize;

            // Stage1 dump flag (unchanged)
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
                            ctx.out_ctx.rate, // guide tap selection (96k vs 192k vs 384k)
                        )
                    })
                    .collect(),
            );

            // Apply makeup gain for fractional L/M paths by increasing the global scaling factor.
            // This mirrors existing practice for other L/M paths: scale by L (zero-stuff factor).
            // Examples:
            //   L=5  -> +14.0 dB (≈ ×5.012) ~ ×5
            //   L=10 -> +20.0 dB (×10)
            //   L=20 -> +26.02 dB (×20)
            if ctx.upsample_ratio > 1 {
                let l = ctx.upsample_ratio as f64;
                ctx.out_ctx.scale_factor *= l;
                if ctx.verbose_mode {
                    eprintln!(
                        "[DBG] L/M path makeup gain: ×{} (scale_factor now {:.6})",
                        ctx.upsample_ratio, ctx.out_ctx.scale_factor
                    );
                }
            }
        }

        // Unified precomputed integer decimator path selection (select taps first, then build once)
        {
            let mut precalc_taps: Option<&'static [f64]> = None;
            let mut precalc_label: Option<&'static str> = None;

            if decim_ratio == 32 && ctx.in_ctx.dsd_rate == 1 && ctx.filt_type == 'E' {
                precalc_taps = Some(&HTAPS_DSD64_32TO1_EQ);
                precalc_label = Some("32:1 EQ (DSD64 -> 88.2 kHz)");
            } else if decim_ratio == 64 && ctx.in_ctx.dsd_rate == 2 {
                if ctx.filt_type == 'C' {
                    precalc_taps = Some(&HTAPS_DDR_64TO1_CHEB);
                    precalc_label = Some("64:1 Chebyshev (DSD128 -> 88.2 kHz)");
                } else if ctx.filt_type == 'E' {
                    precalc_taps = Some(&HTAPS_DDR_64TO1_EQ);
                    precalc_label = Some("64:1 Equiripple (DSD128 -> 88.2 kHz)");
                }
            }

            if let Some(taps) = precalc_taps {
                let ch = ctx.in_ctx.channels_num as usize;
                ctx.precalc_decims = Some(
                    (0..ch)
                        .map(|_| {
                            BytePrecalcDecimator::new(taps, decim_ratio as u32)
                                .expect("Precalc BytePrecalcDecimator init failed")
                        })
                        .collect(),
                );
                if ctx.verbose_mode {
                    if let Some(label) = precalc_label {
                        eprintln!(
                            "[DBG] Precalc decimator enabled: {} (ratio {}:1).",
                            label, decim_ratio
                        );
                    } else {
                        eprintln!("[DBG] Precalc decimator enabled (ratio {}:1).", decim_ratio);
                    }
                }
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
        // Start timer BEFORE any heavy work (DSP + file write)
        let wall_start = Instant::now();

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
            if self.out_ctx.bits == 32 {
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

        // Process (DSP)
        self.process_blocks()?;
        let dsp_elapsed = wall_start.elapsed();

        eprintln!("Clipped {} times.", self.clips);
        eprintln!("");

        // Save file for non-stdout outputs using a derived path (like C++)
        if self.out_ctx.output != 's' {
            self.write_file();
        }
        // Total elapsed now includes file write
        let total_elapsed = wall_start.elapsed();

        if self.total_dsd_bytes_processed > 0 {
            self.report_timing(dsp_elapsed, total_elapsed);
        }

        if self.verbose_mode {
            self.report_in_out();
        }

        Ok(())
    }

    // Report timing & speed
    fn report_timing(&self, dsp_elapsed: std::time::Duration, total_elapsed: std::time::Duration) {
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
        let dsp_sec = dsp_elapsed.as_secs_f64().max(1e-9);
        let total_sec = total_elapsed.as_secs_f64().max(1e-9);
        let speed_dsp = audio_seconds / dsp_sec;
        let speed_total = audio_seconds / total_sec;
        // Format H:MM:SS for elapsed
        let total_secs = total_elapsed.as_secs();
        let h = total_secs / 3600;
        let m = (total_secs % 3600) / 60;
        let s = total_secs % 60;
        eprintln!(
            "{} bytes processed in {:02}:{:02}:{:02}  (DSP speed: {:.2}x, End-to-end: {:.2}x)",
            self.total_dsd_bytes_processed, h, m, s, speed_dsp, speed_total
        );
    }

    // ---- Diagnostics: expected vs actual output length (verbose only) ----
    fn report_in_out(&self) {
        let ch = self.in_ctx.channels_num.max(1) as u64;
        let bps = self.out_ctx.bytes_per_sample as u64;
        let expected_frames = self.diag_expected_frames_floor;
        let actual_frames = self.diag_frames_out;
        // Estimate latency (frames not emitted at start) for rational path
        let mut latency_frames_est = 0u64;
        if let Some(ref rvec) = self.eq_lm_resamplers {
            if let Some(r0) = rvec.first() {
                latency_frames_est = r0.output_latency_frames(self.lm_dump_stage1).round() as u64;
            }
        }
        let expected_bytes = expected_frames * ch * bps;
        let actual_bytes = actual_frames * ch * bps;
        let diff_frames = expected_frames as i64 - actual_frames as i64;
        let diff_bytes = expected_bytes as i64 - actual_bytes as i64;
        let pct = if expected_frames > 0 {
            (diff_frames as f64) * 100.0 / (expected_frames as f64)
        } else {
            0.0
        };
        eprintln!("\n[DIAG] Output length accounting:");
        eprintln!(
            "[DIAG] DSD bits in: {}  L={}  M={}  stage1_dump={}",
            self.diag_bits_in, self.upsample_ratio, self.decim_ratio, self.lm_dump_stage1
        );
        eprintln!(
            "[DIAG] Expected frames (floor): {}  Actual frames: {}  Diff: {} ({:.5}%)",
            expected_frames, actual_frames, diff_frames, pct
        );
        if latency_frames_est > 0 {
            let post_latency = expected_frames.saturating_sub(latency_frames_est);
            let residual = post_latency as i64 - actual_frames as i64;
            eprintln!(
                "[DIAG] Est. latency frames: {}  Expected after latency: {}  Residual diff: {}",
                latency_frames_est, post_latency, residual
            );
        }
        eprintln!(
            "[DIAG] Expected bytes: {}  Actual bytes: {}  Diff bytes: {}",
            expected_bytes, actual_bytes, diff_bytes
        );
        eprintln!(
            "[DIAG] Reason for shortfall: FIR group delay (startup) plus unflushed tail at end. \
No data is lost due to buffer resizing; resizing only adjusts capacity."
        );
    }

    fn write_file(&mut self) -> Result<(), Box<dyn Error>> {
        eprintln!("Saving to file...");
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
            'f' => {
                self.out_ctx
                    .save_and_print_file(&out_path, AudioFileFormat::Flac)?;
            }
            _ => {}
        }

        if self.in_ctx.input.to_ascii_lowercase().ends_with(".dsf") {
            use dsf::DsfFile;
            let path = Path::new(&self.in_ctx.input);
            let path_out = Path::new(&out_path);
            let dsf_file = DsfFile::open(path)?;
            if let Some(tag) = dsf_file.id3_tag() {
                tag.write_to_path(path_out, tag.version())?;
            }
        }

        Ok(())
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
                    if produced >= buf_capacity {
                        break;
                    }
                    let bit = (byte >> b) & 1;
                    let got = rs.push_bit_lm(bit, self.lm_dump_stage1);
                    if let Some(y) = got {
                        self.float_data[produced] = y;
                        produced += 1;
                    }
                }
            } else {
                for b in (0..8).rev() {
                    if produced >= buf_capacity {
                        break;
                    }
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

    // NEW: Produce one channel via Equiripple 32:1 integer path (BytePrecalcDecimator)
    fn process_precalc_channel(
        &mut self,
        chan: usize,
        block_remaining: usize,
        dsd_chan_offset: usize,
        dsd_stride: isize,
        out_frames: usize,
    ) {
        let Some(ref mut v) = self.precalc_decims else {
            return;
        };
        let dec = &mut v[chan];
        let lsb_first = self.in_ctx.lsbit_first != 0;
        let stride = if dsd_stride >= 0 {
            dsd_stride as usize
        } else {
            0
        };
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
        let produced = dec.process_bytes(&chan_bytes, &mut self.float_data[..out_frames]);
        if produced < out_frames {
            self.float_data[produced..out_frames].fill(0.0);
        }
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

            // ---- DIAGNOSTICS: accumulate input bits & recompute expected frames ----
            // Total DSD bits seen so far (all channels counted per-channel implicitly by later math)
            self.diag_bits_in += (block_remaining as u64) * 8;
            if self.eq_lm_resamplers.is_some() {
                // Rational path: frames ≈ floor(bits * L / M) (or first-stage denominator if dumping)
                let eff_L = self.upsample_ratio as u64;
                let eff_M = if self.lm_dump_stage1 {
                    if self.decim_ratio == 294 {
                        14
                    } else {
                        7
                    }
                } else {
                    self.decim_ratio
                } as u64;
                self.diag_expected_frames_floor = (self.diag_bits_in * eff_L) / eff_M;
            } else {
                // Integer / Chebyshev / original FIR path
                if self.decim_ratio > 0 {
                    self.diag_expected_frames_floor = self.diag_bits_in / (self.decim_ratio as u64);
                }
            }
            // ------------------------------------------------------------------------

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
            } else if self.precalc_decims.is_some() {
                (bits_in / (self.decim_ratio as usize), false)
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

                // Select per-channel processing path
                if self.precalc_decims.is_some() {
                    // Unified precomputed integer decimator path
                    self.float_data[..estimate_frames].fill(0.0);
                    self.process_precalc_channel(
                        chan,
                        block_remaining,
                        dsd_chan_offset,
                        self.in_ctx.dsd_stride as isize,
                        estimate_frames,
                    );
                    frames_used_per_chan = estimate_frames;
                } else if self.eq_lm_resamplers.is_some() {
                    // Fractional L/M equiripple path
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
                    // Original integer FIR translate path
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
                // TODO: restore float output for stdout (write_float)
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

            // Diagnostics: add actual produced frames (per channel)
            self.diag_frames_out += frames_used_per_chan as u64;

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
        } else if self.in_ctx.dsd_rate == 1
            && !([8, 16, 32].contains(&self.decim_ratio)
                || (self.decim_ratio == 147 && self.filt_type == 'E'))
        {
            return Err(
                "With DSD64 input, allowed decimation values are 8, 16, 32, or 147 (with Equiripple filter)."
                    .into(),
            );
        }
        if self.decim_ratio == 294 && !(self.in_ctx.dsd_rate == 2 && self.filt_type == 'E') {
            return Err(
                "294:1 decimation currently only supported for DSD128 with Equiripple filter."
                    .into(),
            );
        }
        if self.decim_ratio == 147
            && !(self.filt_type == 'E' && (self.in_ctx.dsd_rate == 1 || self.in_ctx.dsd_rate == 2))
        {
            return Err(
                "147:1 decimation is only supported with the Equiripple filter for DSD64 or DSD128 input."
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

    #[inline]
    fn compute_decim_and_upsample(in_ctx: &InputContext, out_ctx: &OutputContext) -> (i32, u32) {
        // Determine decimation ratio (M)
        let decim_ratio: i32 = if out_ctx.rate == 96_000 && in_ctx.dsd_rate == 2 {
            294
        } else if out_ctx.rate == 96_000 || out_ctx.rate == 192_000 || out_ctx.rate == 384_000 {
            147
        } else {
            // Integer ratio attempt (fallback to 64)
            let base = DSD_64_RATE * (in_ctx.dsd_rate as u32);
            if out_ctx.rate > 0 && base % (out_ctx.rate as u32) == 0 {
                (base / (out_ctx.rate as u32)) as i32
            } else {
                64
            }
        };

        // Upsample (L) selection
        let upsample_ratio: u32 = if decim_ratio < 147 {
            1
        } else if out_ctx.rate == 384_000 {
            if in_ctx.dsd_rate == 1 {
                20
            } else {
                10
            }
        } else if out_ctx.rate == 192_000 {
            if in_ctx.dsd_rate == 1 {
                10
            } else {
                5
            }
        } else if out_ctx.rate == 96_000 {
            5
        } else {
            5
        };
        (decim_ratio, upsample_ratio)
    }
}
