use crate::audio_file::AudioFileFormat;
use crate::byte_precalc_decimator::bit_reverse_u8;
use crate::byte_precalc_decimator::BytePrecalcDecimator;
use crate::dither::Dither;
// NEW: 576 kHz -> /3 (final 192 kHz)
use crate::dsdin_sys::DSD_64_RATE;
use crate::filters::{
    HTAPS_16TO1_XLD, HTAPS_32TO1, HTAPS_D2P, HTAPS_DDR_16TO1_CHEB, HTAPS_DDR_16TO1_EQ,
    HTAPS_DDR_32TO1_CHEB, HTAPS_DDR_32TO1_EQ, HTAPS_DDR_64TO1_CHEB, HTAPS_DDR_64TO1_EQ,
    HTAPS_DSD256_128TO1_EQ, HTAPS_DSD256_32TO1_EQ, HTAPS_DSD256_64TO1_EQ, HTAPS_DSD64_16TO1_EQ,
    HTAPS_DSD64_32TO1_EQ, HTAPS_DSD64_8TO1_EQ, HTAPS_XLD,
};
use crate::input::InputContext;
use crate::lm_resampler::LMResampler;
use crate::output::OutputContext;
use id3::TagLike;
use std::error::Error;
//use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::Path;
use std::time::Instant;

fn abbrev_rate(rate: u32) -> Option<&'static str> {
    match rate {
        88_200 => Some("88_2K"),
        96_000 => Some("96K"),
        176_400 => Some("176_4K"),
        192_000 => Some("192K"),
        352_800 => Some("352_8K"),
        384_000 => Some("384K"),
        _ => None,
    }
}

pub struct ConversionContext {
    in_ctx: InputContext,
    out_ctx: OutputContext,
    dither: Dither,
    filt_type: char,
    dsd_data: Vec<u8>,
    float_data: Vec<f64>,
    pcm_data: Vec<u8>,
    clips: i32,
    last_samps_clipped_low: i32,
    last_samps_clipped_high: i32,
    verbose_mode: bool,
    append_rate_suffix: bool,
    precalc_decims: Option<Vec<BytePrecalcDecimator>>,
    eq_lm_resamplers: Option<Vec<LMResampler>>,
    total_dsd_bytes_processed: u64,
    upsample_ratio: u32,
    decim_ratio: i32,
    diag_bits_in: u64,
    diag_expected_frames_floor: u64,
    diag_frames_out: u64,
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
        let dsd_bytes_per_chan = in_ctx.block_size as usize;
        let channels = in_ctx.channels_num as usize;
        let bytes_per_sample = out_ctx.bytes_per_sample as usize;
        let dsd_rate = in_ctx.dsd_rate;

        let (decim_ratio, upsample_ratio) = Self::compute_decim_and_upsample(&in_ctx, &out_ctx);

        // Worst-case frames per channel per input block:
        // ceil((bits_in * L) / M). Add small slack for LM paths to avoid edge truncation.
        let bits_per_chan = dsd_bytes_per_chan * 8;
        let frames_max = ((bits_per_chan * (upsample_ratio as usize)) + (decim_ratio.abs() as usize - 1))
            / (decim_ratio.abs() as usize);
        let lm_slack = if upsample_ratio > 1 { 16 } else { 0 };
        let out_frames_capacity = frames_max + lm_slack;

        // Build ctx first so we can use ctx.verbose instead of manual verbose checks
        let mut ctx = Self {
            in_ctx,
            out_ctx,
            dither,
            filt_type,
            dsd_data: vec![0; dsd_bytes_per_chan * channels],
            float_data: vec![0.0; out_frames_capacity],
            pcm_data: vec![0; out_frames_capacity * channels * bytes_per_sample],
            clips: 0,
            last_samps_clipped_low: 0,
            last_samps_clipped_high: 0,
            verbose_mode: verbose_param,
            append_rate_suffix: false,
            precalc_decims: None,
            eq_lm_resamplers: None,
            total_dsd_bytes_processed: 0,
            upsample_ratio,
            decim_ratio,
            diag_bits_in: 0,
            diag_expected_frames_floor: 0,
            diag_frames_out: 0,
        };

        // Fractional (L/M) path stays as-is (stage1 dump removed permanently).
        if ctx.filt_type == 'E' && (decim_ratio == 294 || decim_ratio == 147 || decim_ratio == 588)
        {
            let ch = ctx.in_ctx.channels_num as usize;
            ctx.eq_lm_resamplers = Some(
                (0..ch)
                    .map(|_i| {
                        LMResampler::new(
                            ctx.upsample_ratio,
                            decim_ratio,
                            ctx.verbose_mode,
                            ctx.out_ctx.rate as u32,
                        )
                    })
                    .collect(),
            );
            if ctx.upsample_ratio > 1 {
                let l = ctx.upsample_ratio as f64;
                ctx.out_ctx.scale_factor *= l;
                ctx.verbose(
                    &format!(
                        "[DBG] L/M path makeup gain: ×{} (scale_factor now {:.6})",
                        ctx.upsample_ratio, ctx.out_ctx.scale_factor
                    ),
                    true,
                );
            }
        }

        // Integer simple decimation path: attempt universal precalc selection.
        if ctx.eq_lm_resamplers.is_none() {
            if let Some(taps) = Self::select_precalc_taps(ctx.filt_type, dsd_rate, ctx.decim_ratio)
            {
                let ch = ctx.in_ctx.channels_num as usize;
                ctx.precalc_decims = Some(
                    (0..ch)
                        .map(|_| {
                            BytePrecalcDecimator::new(taps, ctx.decim_ratio as u32)
                                .expect("Precalc BytePrecalcDecimator init failed")
                        })
                        .collect(),
                );
                ctx.verbose(
                    &format!(
                        "Precalc decimator enabled (ratio {}:1, filter '{}', dsd_rate {}).",
                        ctx.decim_ratio, ctx.filt_type, dsd_rate
                    ),
                    true,
                );
            } else {
                eprintln!(
                    "Precalc taps not found for ratio {} / filter '{}' (dsd_rate {}). ",
                    ctx.decim_ratio, ctx.filt_type, dsd_rate
                );
            }
        }

        ctx.out_ctx.set_channels_num(ctx.in_ctx.channels_num);
        ctx.out_ctx.init_file()?;
        ctx.dither.init();
        Ok(ctx)
    }

    fn verbose(&self, message: &str, new_line: bool) {
        if self.verbose_mode {
            if new_line {
                eprintln!("{}", message);
            } else {
                eprint!("{}", message);
            }
        }
    }

    // NEW: central mapping from (filter type, dsd_rate, decimation ratio) to half-tap tables.
    // Returns Some(&half_taps) if we can drive a single-stage BytePrecalcDecimator; otherwise None.
    fn select_precalc_taps(
        filt_type: char,
        dsd_rate: i32,
        decim_ratio: i32,
    ) -> Option<&'static [f64]> {
        match decim_ratio {
            // 128:1 (DSD256 -> 88.2 kHz), Equiripple only
            128 => {
                if filt_type == 'E' && dsd_rate == 4 {
                    Some(&HTAPS_DSD256_128TO1_EQ)
                } else {
                    None
                }
            }
            // 8:1 (DSD64 only) – 'D' uses HTAPS_D2P, 'X' uses HTAPS_XLD, 'E' uses new equiripple, others fallback
            8 => {
                if dsd_rate == 1 {
                    match filt_type {
                        'D' => Some(&HTAPS_D2P),
                        'X' => Some(&HTAPS_XLD),
                        'E' => Some(&HTAPS_DSD64_8TO1_EQ),
                        _ => None,
                    }
                } else {
                    None
                }
            }
            // 16:1
            16 => match filt_type {
                'X' => Some(&HTAPS_16TO1_XLD),
                // E – equiripple: now support DSD64 with dedicated table, DSD128 with DDR table
                'E' => {
                    if dsd_rate == 1 {
                        Some(&HTAPS_DSD64_16TO1_EQ)
                    } else if dsd_rate == 2 {
                        Some(&HTAPS_DDR_16TO1_EQ)
                    } else {
                        None
                    }
                }
                // C – Chebyshev only provided for DSD128; fallback None for others
                'C' => {
                    if dsd_rate == 2 {
                        Some(&HTAPS_DDR_16TO1_CHEB)
                    } else {
                        None
                    }
                }
                _ => None,
            },
            // 32:1
            32 => match filt_type {
                'X' => Some(&HTAPS_32TO1),
                'E' => {
                    if dsd_rate == 1 {
                        Some(&HTAPS_DSD64_32TO1_EQ)
                    } else if dsd_rate == 4 {
                        // New dedicated DSD256 32:1 equiripple half taps
                        Some(&HTAPS_DSD256_32TO1_EQ)
                    } else {
                        Some(&HTAPS_DDR_32TO1_EQ)
                    }
                }
                'C' => Some(&HTAPS_DDR_32TO1_CHEB),
                _ => None,
            },
            // 64:1
            64 => match filt_type {
                'E' => {
                    if dsd_rate == 4 {
                        Some(&HTAPS_DSD256_64TO1_EQ)
                    } else {
                        Some(&HTAPS_DDR_64TO1_EQ)
                    }
                }
                'C' => Some(&HTAPS_DDR_64TO1_CHEB),
                'X' | 'D' => Some(&HTAPS_DDR_64TO1_EQ),
                _ => None,
            },
            _ => None,
        }
    }

    // Derive output path like the C++ writeFile(): basename + proper extension, or "output.xxx" for stdin
    fn derive_output_path(&self) -> String {
        let ext = match self.out_ctx.output.to_ascii_lowercase() {
            'w' => "wav",
            'a' => "aif",
            'f' => "flac",
            _ => "out",
        };
        let suffix = if self.append_rate_suffix {
            if let Some(abbrev) = abbrev_rate(self.out_ctx.rate as u32) {
                format!("_{}", abbrev)
            } else {
                String::new()
            }
        } else {
            String::new()
        };
        if self.in_ctx.std_in {
            let base = if suffix.is_empty() {
                "output".to_string()
            } else {
                format!("output{}", suffix)
            };
            return format!("{}.{}", base, ext);
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
        let name = if suffix.is_empty() {
            format!("{}.{}", stem, ext)
        } else {
            format!("{}{}.{}", stem, suffix, ext)
        };
        parent.join(name).to_string_lossy().into_owned()
    }

    pub fn do_conversion(&mut self) -> Result<(), Box<dyn Error>> {
        self.check_conv()?;
        eprintln!(
            "Dither type: {}",
            self.dither.dither_type().to_ascii_uppercase()
        );
        let wall_start = Instant::now();

        // (Configuration prints intentionally unconditional; leave as-is)

        // Unified processing loop (handles both LM and integer paths)
        self.process_blocks()?;
        let dsp_elapsed = wall_start.elapsed();

        eprintln!("Clipped {} times.", self.clips);
        eprintln!("");

        if self.out_ctx.output != 's' {
            if let Err(e) = self.write_file() {
                eprintln!("Error writing file: {e}");
            }
        }
        let total_elapsed = wall_start.elapsed();

        if self.total_dsd_bytes_processed > 0 {
            self.report_timing(dsp_elapsed, total_elapsed);
        }

        // Replace direct verbose_mode check with verbose call wrapping a marker + invocation
        // (report_in_out already prints; gate with a cheap verbose boolean guard here)
        self.verbose("[DBG] Detailed output length diagnostics:", true);
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

    pub fn set_append_rate_suffix(&mut self, value: bool) {
        self.append_rate_suffix = value;
    }

    // ---- Diagnostics: expected vs actual output length (verbose only) ----
    fn report_in_out(&self) {
        let ch = self.in_ctx.channels_num.max(1) as u64;
        let bps = self.out_ctx.bytes_per_sample as u64;
        let expected_frames = self.diag_expected_frames_floor;
        let actual_frames = self.diag_frames_out;
        // Estimate latency (frames not emitted at start) for rational path
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
            "[DIAG] DSD bits in: {}  L={}  M={}",
            self.diag_bits_in, self.upsample_ratio, self.decim_ratio
        );
        eprintln!(
            "[DIAG] Expected frames (floor): {}  Actual frames: {}  Diff: {} ({:.5}%)",
            expected_frames, actual_frames, diff_frames, pct
        );
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

        self.verbose(&format!("Derived output path: {}", out_path), true);

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

        if let Some(mut tag) = self.in_ctx.tag.clone() {
            self.verbose("Copying ID3 tags from input file...", true);
            // If -a/--append was requested and an album tag exists, append " [PCM]" to album
            if self.append_rate_suffix {
                if let Some(album) = tag.album() {
                    // Avoid duplicating the suffix if already present
                    if !album.ends_with(" [PCM]") {
                        let mut new_album = String::from(album);
                        new_album.push_str(" [PCM]");
                        tag.set_album(new_album);
                    }
                }
            }
            let path_out = Path::new(&out_path);
            tag.write_to_path(path_out, tag.version())?;
        } else {
            self.verbose("Input file has no tag; skipping tag copy.", true);
        }

        Ok(())
    }

    // Unified L/M rational path channel processor
    // Unified per-channel processing: handles both LM (rational) and integer paths.
    // Returns the number of frames produced into self.float_data for this channel.
    #[inline(always)]
    fn process_channel(
        &mut self,
        chan: usize,
        bytes_per_channel: usize,
        dsd_chan_offset: usize,
        dsd_stride: isize,
        lm_mode: bool,
    ) -> usize {
        // Build contiguous per-channel byte buffer with proper bit endianness.
        let lsb_first = self.in_ctx.lsbit_first != 0;
        let stride = if dsd_stride >= 0 {
            dsd_stride as usize
        } else {
            0
        };
        let mut chan_bytes = Vec::with_capacity(bytes_per_channel);

        for i in 0..bytes_per_channel {
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

        if lm_mode {
            // LM path: use rational resampler, honor actual produced count
            let Some(resamps) = self.eq_lm_resamplers.as_mut() else {
                return 0;
            };
            let rs = &mut resamps[chan];
            return rs.process_bytes_lm(&chan_bytes, true, &mut self.float_data);
        } else {
            // Integer path: use precalc decimator; conventionally return the estimate
            let Some(ref mut v) = self.precalc_decims else {
                return 0;
            };
            let dec = &mut v[chan];
            return dec.process_bytes(&chan_bytes, &mut self.float_data);
        }
    }

    // Dedicated LM loop removed: consolidated into process_blocks

    fn process_blocks(&mut self) -> Result<(), Box<dyn Error>> {
        let channels = self.in_ctx.channels_num as usize;
        let frame_size: usize = (self.in_ctx.block_size as usize) * channels;

        // Open a unified reader (stdin or file), seeking if needed
        let reading_from_file = !self.in_ctx.std_in;
        let mut reader: Box<dyn Read> = if reading_from_file {
            // Obtain an owned File by cloning the handle from InputContext, then seek if needed.
            let mut file = self
                .in_ctx
                .file
                .as_ref()
                .ok_or_else(|| io::Error::new(io::ErrorKind::Other, "Missing input file handle"))?
                .try_clone()?;
            if self.in_ctx.audio_pos > 0 {
                file.seek(SeekFrom::Start(self.in_ctx.audio_pos as u64))?;
                self.verbose(
                    &format!(
                        "Seeked to audio start position: {}",
                        file.stream_position()?
                    ),
                    true,
                );
            }
            Box::new(file)
        } else {
            Box::new(io::stdin().lock())
        };

        // Initialize bytes_remaining like C++
        let mut bytes_remaining: u64 = if reading_from_file {
            if self.in_ctx.audio_length > 0 {
                self.in_ctx.audio_length
            } else {
                frame_size as u64
            }
        } else {
            frame_size as u64
        };

        loop {
            // Read only full frames from file; stdin always reads frame_size
            let to_read: usize = if reading_from_file {
                if bytes_remaining >= frame_size as u64 {
                    frame_size
                } else {
                    bytes_remaining as usize
                }
            } else {
                frame_size
            };
            if to_read == 0 {
                break;
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
            let bytes_per_channel: usize = read_size / channels;

            // ---- DIAGNOSTICS: accumulate input bits & recompute expected frames ----
            if self.verbose_mode {
                self.diag_bits_in += (bytes_per_channel as u64) * 8;
                self.diag_expected_frames_floor =
                    (self.diag_bits_in * self.upsample_ratio as u64) / self.decim_ratio as u64;
            }

            // Compute per-block output buffer requirement so LM path can consume all inputs
            let bits_in = bytes_per_channel * 8;
            let l = self.upsample_ratio as usize;
            let m = self.decim_ratio as usize;
            let lm_mode_now = self.eq_lm_resamplers.is_some();
            let estimate_frames = if lm_mode_now {
                (bits_in * l + (m - 1)) / m
            } else {
                bits_in / m
            };

            // float_data sized worst-case in new(); no per-loop growth needed
            if self.out_ctx.output == 's' {
                // Provide a small fixed headroom to avoid edge truncation at stage boundaries
                let slack = if lm_mode_now { 16 } else { 0 };
                let buf_needed = estimate_frames.saturating_add(slack);
                let bps = self.out_ctx.bytes_per_sample as usize;
                let need_bytes = buf_needed.saturating_mul(channels).saturating_mul(bps);
                if self.pcm_data.len() < need_bytes {
                    self.pcm_data.resize(need_bytes, 0);
                }
            }

            // Track actual frames produced per channel (set by channel 0)
            let mut frames_used_per_chan = 0usize;

            // Per-channel processing loop (handles both LM and integer paths)
            for chan in 0..channels {
                let dsd_chan_offset = chan * self.in_ctx.dsd_chan_offset as usize;

                let produced = self.process_channel(
                    chan,
                    bytes_per_channel,
                    dsd_chan_offset,
                    self.in_ctx.dsd_stride as isize,
                    lm_mode_now,
                );
                if chan == 0 {
                    frames_used_per_chan = produced;
                }

                // Output / packing per channel
                if self.out_ctx.output == 's' {
                    // Interleave into pcm_data (handle float vs integer separately)
                    let bps = self.out_ctx.bytes_per_sample as usize; // 4 for 32-bit float
                    let mut pcm_pos = chan * bps;
                    for s in 0..frames_used_per_chan {
                        let mut out_idx = pcm_pos;
                        if self.out_ctx.bits == 32 {
                            // 32-bit float path: scale only, no dithering/clipping
                            let mut q = self.float_data[s] * self.out_ctx.scale_factor;
                            self.dither.process_samp(&mut q, chan);
                            self.write_float(&mut out_idx, q);
                        } else {
                            // Integer path: dither + clamp + write_int
                            let mut qin: f64 = self.float_data[s] * self.out_ctx.scale_factor;
                            self.dither.process_samp(&mut qin, chan);
                            let value = Self::my_round(qin) as i32;
                            let peak = self.out_ctx.peak_level as i32;
                            let clamped = self.clamp_value(-peak, value, peak - 1);
                            self.write_int(&mut out_idx, clamped, bps);
                        }
                        pcm_pos += channels * bps;
                    }
                } else {
                    // File formats: push samples into AudioFile buffers
                    if self.out_ctx.bits == 32 {
                        for s in 0..frames_used_per_chan {
                            let mut q = self.float_data[s] * self.out_ctx.scale_factor;
                            self.dither.process_samp(&mut q, chan);
                            self.out_ctx.push_samp(q as f32, chan);
                        }
                    } else {
                        for s in 0..frames_used_per_chan {
                            let mut qin: f64 = self.float_data[s] * self.out_ctx.scale_factor;
                            self.dither.process_samp(&mut qin, chan);
                            let value = Self::my_round(qin) as i32;
                            let peak = self.out_ctx.peak_level as i32;
                            let clamped = self.clamp_value(-peak, value, peak - 1);
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
                bytes_remaining -= read_size as u64;
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

    fn check_conv(&self) -> Result<(), Box<dyn Error>> {
        // DSD128 (2x) explicit allow list
        if self.in_ctx.dsd_rate == 2 && ![16, 32, 64, 147, 294].contains(&self.decim_ratio) {
            return Err(
                "Only decimation value of 16, 32, 64, 147, or 294 allowed with DSD128 input."
                    .into(),
            );
        }
        // DSD64 (1x) allow list (8/16/32 always; 147 only with 'E')
        if self.in_ctx.dsd_rate == 1
            && !([8, 16, 32].contains(&self.decim_ratio)
                || (self.decim_ratio == 147 && self.filt_type == 'E'))
        {
            return Err("With DSD64 input, allowed decimation values are 8, 16, 32, or 147 (with Equiripple filter).".into());
        }
        // DSD256 (4x) paths: integer decimations 32, 64, 128, 147, 294 or LM decim 588; all require 'E'
        if self.in_ctx.dsd_rate == 4
            && !(matches!(self.decim_ratio, 32 | 64 | 128 | 147 | 294 | 588)
                && self.filt_type == 'E')
        {
            return Err("With DSD256 input, only 32:1, 64:1, 128:1, 147:1, 294:1 integer or 588:1 (two-stage LM) decimation using the Equiripple filter is supported.".into());
        }
        // 294:1 constraint (must be E; allowed for DSD128 and DSD256)
        if self.decim_ratio == 294
            && !(self.filt_type == 'E' && (self.in_ctx.dsd_rate == 2 || self.in_ctx.dsd_rate == 4))
        {
            return Err("294:1 decimation is only supported for DSD128 or DSD256 with the Equiripple filter.".into());
        }
        // 147:1 constraint (must be E and one of the allowed dsd rates: 64/128/256)
        if self.decim_ratio == 147
            && !(self.filt_type == 'E'
                && (self.in_ctx.dsd_rate == 1
                    || self.in_ctx.dsd_rate == 2
                    || self.in_ctx.dsd_rate == 4))
        {
            return Err("147:1 decimation is only supported with the Equiripple filter for DSD64, DSD128, or DSD256 input.".into());
        }
        Ok(())
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
                // 24-bit container (also used for 20-bit). For 20-bit we left-align by shifting 4.
                let mut v = value;
                if self.out_ctx.bits == 20 {
                    v <<= 4; // align 20 significant bits into the top of 24-bit word (LS 4 bits zero)
                }
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

    // Helper function for clip stats
    #[inline]
    fn update_clip_stats(&mut self, low: bool, high: bool) {
        if low {
            if self.last_samps_clipped_low == 1 {
                self.clips += 1;
            }
            self.last_samps_clipped_low += 1;
            return;
        }
        self.last_samps_clipped_low = 0;
        if high {
            if self.last_samps_clipped_high == 1 {
                self.clips += 1;
            }
            self.last_samps_clipped_high += 1;
            return;
        }
        self.last_samps_clipped_high = 0;
    }

    // Make clamp a method to access self
    #[inline]
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
    fn my_round(x: f64) -> i64 {
        if x < 0.0 {
            (x - 0.5).floor() as i64
        } else {
            (x + 0.5).floor() as i64
        }
    }

    fn compute_decim_and_upsample(in_ctx: &InputContext, out_ctx: &OutputContext) -> (i32, u32) {
        // Determine decimation ratio (M)
        let decim_ratio: i32 = if out_ctx.rate == 96_000 && in_ctx.dsd_rate == 4 {
            // DSD256 -> 96k two-stage LM path: M=588 (×5 -> /21 -> /28)
            588
        } else if (out_ctx.rate == 96_000 && in_ctx.dsd_rate == 2)
            || (out_ctx.rate == 192_000 && in_ctx.dsd_rate == 4)
        {
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
            // Specialized two-phase L choices for 384k: DSD64->L20, DSD128->L10, DSD256->L5
            if in_ctx.dsd_rate == 1 {
                20
            } else if in_ctx.dsd_rate == 2 {
                10
            } else if in_ctx.dsd_rate == 4 {
                5
            } else {
                5
            }
        } else if out_ctx.rate == 192_000 {
            // 192k: DSD64->L10, DSD128->L5, DSD256->L5 (reuses L5 path)
            if in_ctx.dsd_rate == 1 {
                10
            } else if in_ctx.dsd_rate == 2 {
                5
            } else if in_ctx.dsd_rate == 4 {
                5
            } else {
                5
            }
        } else if out_ctx.rate == 96_000 {
            5 // All current supported DSD rates use L=5 toward 96k
        } else {
            5
        };
        (decim_ratio, upsample_ratio)
    }
}
