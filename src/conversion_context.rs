/*
 Copyright (c) 2023 clone206

 This file is part of dsd2dxd

 dsd2dxd is free software: you can redistribute it and/or modify it
 under the terms of the GNU General Public License as published by the
 Free Software Foundation, either version 3 of the License, or
 (at your option) any later version.

 dsd2dxd is distributed in the hope that it will be useful, but
 WITHOUT ANY WARRANTY; without even the implied warranty of
 MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE. See the
 GNU General Public License for more details.
 You should have received a copy of the GNU General Public License
 along with dsd2dxd. If not, see <https://www.gnu.org/licenses/>.
*/

use crate::byte_precalc_decimator::bit_reverse_u8;
use crate::byte_precalc_decimator::select_precalc_taps;
use crate::byte_precalc_decimator::BytePrecalcDecimator;
use crate::dither::Dither;
use crate::dsd::DSD_64_RATE;
use crate::input::InputContext;
use crate::lm_resampler::compute_decim_and_upsample;
use crate::lm_resampler::LMResampler;
use crate::output::OutputContext;
use flac_codec::metadata;
use flac_codec::metadata::PictureType;
use id3::TagLike;
use std::error::Error;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::Path;
use std::time::Instant;

pub struct ConversionContext {
    in_ctx: InputContext,
    out_ctx: OutputContext,
    dither: Dither,
    filt_type: char,
    dsd_data: Vec<u8>,
    float_data: Vec<f64>,
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
    reader: Box<dyn Read>,
}

impl ConversionContext {
    pub fn new(
        in_ctx: InputContext,
        out_ctx: OutputContext,
        dither: Dither,
        filt_type: char,
        verbose_param: bool,
        append_rate_suffix: bool,
    ) -> Result<Self, Box<dyn Error>> {
        let dsd_bytes_per_chan = in_ctx.block_size as usize;
        let channels = in_ctx.channels_num as usize;
        let dsd_rate = in_ctx.dsd_rate;

        let (decim_ratio, upsample_ratio) =
            compute_decim_and_upsample(in_ctx.dsd_rate, out_ctx.rate);

        // Worst-case frames per channel per input block:
        // ceil((bits_in * L) / M). Add small slack for LM paths to avoid edge truncation.
        let bits_per_chan = dsd_bytes_per_chan * 8;
        let frames_max = ((bits_per_chan * (upsample_ratio as usize))
            + (decim_ratio.abs() as usize - 1))
            / (decim_ratio.abs() as usize);
        let lm_slack = if upsample_ratio > 1 { 16 } else { 0 };
        let out_frames_capacity = frames_max + lm_slack;

        let mut ctx = Self {
            in_ctx,
            out_ctx,
            dither,
            filt_type,
            dsd_data: vec![0; dsd_bytes_per_chan * channels],
            float_data: vec![0.0; out_frames_capacity],
            clips: 0,
            last_samps_clipped_low: 0,
            last_samps_clipped_high: 0,
            verbose_mode: verbose_param,
            append_rate_suffix,
            precalc_decims: None,
            eq_lm_resamplers: None,
            total_dsd_bytes_processed: 0,
            upsample_ratio,
            decim_ratio,
            diag_bits_in: 0,
            diag_expected_frames_floor: 0,
            diag_frames_out: 0,
            reader: Box::new(io::empty()), // Placeholder
        };

        if upsample_ratio > 1 {
            ctx.eq_lm_resamplers = Some(
                (0..channels)
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
            ctx.out_ctx.scale_factor *= ctx.upsample_ratio as f64;
            ctx.verbose(
                &format!(
                    "[DBG] L/M path makeup gain: ×{} (scale_factor now {:.6})",
                    ctx.upsample_ratio, ctx.out_ctx.scale_factor
                ),
                true,
            );
        } else if let Some(taps) = select_precalc_taps(ctx.filt_type, dsd_rate, ctx.decim_ratio) {
            ctx.precalc_decims = Some(
                (0..channels)
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
            return Err(format!(
                "Taps not found for ratio {} / filter '{}' (dsd_rate {}). ",
                ctx.decim_ratio, ctx.filt_type, dsd_rate
            )
            .into());
        }

        ctx.out_ctx
            .init(out_frames_capacity, ctx.in_ctx.channels_num)?;
        ctx.dither.init();
        Ok(ctx)
    }

    pub fn do_conversion(&mut self) -> Result<(), Box<dyn Error>> {
        self.check_conv()?;
        self.reader = self.get_reader(!self.in_ctx.std_in)?;
        eprintln!(
            "Dither type: {}",
            self.dither.dither_type().to_ascii_uppercase()
        );
        let wall_start = Instant::now();

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

        self.verbose("[DBG] Detailed output length diagnostics:", true);
        if self.verbose_mode {
            self.report_in_out();
        }

        Ok(())
    }

    fn get_reader(&mut self, reading_from_file: bool) -> Result<Box<dyn Read>, Box<dyn Error>> {
        if reading_from_file {
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
            Ok(Box::new(file))
        } else {
            Ok(Box::new(io::stdin().lock()))
        }
    }

    #[inline(always)]
    fn read_frame(
        &mut self,
        bytes_remaining: u64,
        frame_size: usize,
    ) -> Result<usize, Box<dyn Error>> {
        // stdin always reads frame_size
        let to_read: usize = if !self.in_ctx.std_in {
            if bytes_remaining >= frame_size as u64 {
                frame_size
            } else {
                bytes_remaining as usize
            }
        } else {
            frame_size
        };

        // Read one frame identically for stdin and file
        let read_size: usize = match self.reader.read_exact(&mut self.dsd_data[..to_read]) {
            Ok(()) => to_read,
            Err(e) => return Err(Box::new(e)),
        };
        // Track bytes processed (all channels)
        self.total_dsd_bytes_processed += read_size as u64;

        Ok(read_size)
    }

    fn process_blocks(&mut self) -> Result<(), Box<dyn Error>> {
        let channels = self.in_ctx.channels_num as usize;
        let frame_size: usize = (self.in_ctx.block_size as usize) * channels;

        // Open a unified reader (stdin or file), seeking if needed
        let reading_from_file = !self.in_ctx.std_in;

        let mut bytes_remaining: u64 = if reading_from_file {
            self.in_ctx.audio_length
        } else {
            frame_size as u64
        };

        loop {
            // Read one frame identically for stdin and file
            let read_size: usize = match self.read_frame(bytes_remaining, frame_size) {
                Ok(s) => s,
                Err(e) => {
                    if let Some(io_err) = e.downcast_ref::<io::Error>() {
                        if io_err.kind() == io::ErrorKind::UnexpectedEof {
                            break;
                        }
                    }
                    return Err(e);
                }
            };

            // Bytes per channel in this read
            let bytes_per_channel: usize = read_size / channels;

            if self.verbose_mode {
                self.diag_bits_in += (bytes_per_channel as u64) * 8;
                self.diag_expected_frames_floor =
                    (self.diag_bits_in * self.upsample_ratio as u64) / self.decim_ratio as u64;
            }

            // Track actual frames produced per channel (set by channel 0)
            let mut samples_used_per_chan = 0usize;

            // Per-channel processing loop (handles both LM and integer paths)
            for chan in 0..channels {
                samples_used_per_chan = self.process_channel(chan, bytes_per_channel);
                self.write_to_buffer(samples_used_per_chan, chan);
            }

            // Derive actual byte count (may differ from estimate in rational path)
            let pcm_frame_bytes =
                samples_used_per_chan * channels * (self.out_ctx.bytes_per_sample as usize);

            if self.verbose_mode {
                self.diag_frames_out += samples_used_per_chan as u64;
            }

            if self.out_ctx.output == 's' && pcm_frame_bytes > 0 {
                self.out_ctx.write_stdout(pcm_frame_bytes)?;
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

    #[inline(always)]
    fn write_to_buffer(&mut self, samples_used_per_chan: usize, chan: usize) {
        // Output / packing for channel
        if self.out_ctx.output == 's' {
            // Interleave into pcm_data (handle float vs integer separately)
            let bps = self.out_ctx.bytes_per_sample as usize; // 4 for 32-bit float
            let mut pcm_pos = chan * bps;
            for s in 0..samples_used_per_chan {
                let mut out_idx = pcm_pos;
                if self.out_ctx.bits == 32 {
                    // 32-bit float path
                    let mut q = self.float_data[s];
                    self.scale_and_dither(&mut q);
                    self.out_ctx.pack_float(&mut out_idx, q);
                } else {
                    // Integer path: dither + clamp + write_int
                    let mut qin: f64 = self.float_data[s];
                    self.scale_and_dither(&mut qin);
                    let quantized = self.quantize(&mut qin);
                    self.out_ctx.pack_int(&mut out_idx, quantized);
                }
                pcm_pos += self.in_ctx.channels_num as usize * bps;
            }
        } else if self.out_ctx.bits == 32 {
            for s in 0..samples_used_per_chan {
                let mut q = self.float_data[s];
                self.scale_and_dither(&mut q);
                self.out_ctx.push_samp(q as f32, chan);
            }
        } else {
            for s in 0..samples_used_per_chan {
                let mut qin: f64 = self.float_data[s];
                self.scale_and_dither(&mut qin);
                let quantized = self.quantize(&mut qin);
                self.out_ctx.push_samp(quantized, chan);
            }
        }
    }

    #[inline(always)]
    fn scale_and_dither(&mut self, sample: &mut f64) {
        *sample *= self.out_ctx.scale_factor;
        self.dither.process_samp(sample);
    }

    #[inline(always)]
    fn quantize(&mut self, qin: &mut f64) -> i32 {
        let value = Self::my_round(*qin) as i32;
        let peak = self.out_ctx.peak_level as i32;
        self.clamp_value(-peak, value, peak - 1)
    }

    // Unified per-channel processing: handles both LM (rational) and integer paths.
    // Returns the number of frames produced into self.float_data for this channel.
    #[inline(always)]
    fn process_channel(&mut self, chan: usize, bytes_per_channel: usize) -> usize {
        // Build contiguous per-channel byte buffer with proper bit endianness.
        let dsd_chan_offset = chan * self.in_ctx.dsd_chan_offset as usize;
        let dsd_stride = self.in_ctx.dsd_stride as isize;
        let lsb_first = self.in_ctx.lsbit_first != 0;
        let stride = if dsd_stride >= 0 {
            dsd_stride as usize
        } else {
            0
        };
        let mut chan_bytes = Vec::with_capacity(bytes_per_channel);
        let lm_mode = self.eq_lm_resamplers.is_some();

        for i in 0..bytes_per_channel {
            let byte_index = dsd_chan_offset + i * stride;
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

    // Basename + proper extension, or generic for stdin
    fn derive_output_path(&self) -> String {
        let ext = match self.out_ctx.output.to_ascii_lowercase() {
            'w' => "wav",
            'a' => "aif",
            'f' => "flac",
            _ => "out",
        };
        let suffix = if let Some((uscore, _dot)) = self.abbrev_rate_pair(self.out_ctx.rate as u32) {
            format!("_{}", uscore)
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

        // If an output path is specified, join it with the relative parent path
        if let Some(ref out_dir) = self.out_ctx.path {
            let rel_parent = self.in_ctx.parent_path.as_ref().map(|p| p.as_path()).unwrap_or(Path::new(""));
            let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
            let rel = rel_parent.strip_prefix(&cwd).unwrap_or(rel_parent);
            return Path::new(out_dir).join(rel).join(name).to_string_lossy().into_owned();
        }

        parent.join(name).to_string_lossy().into_owned()
    }

    fn write_file(&mut self) -> Result<(), Box<dyn Error>> {
        eprintln!("Saving to file...");
        let out_path = self.derive_output_path();

        self.verbose(&format!("Derived output path: {}", out_path), true);

        if let Some(mut tag) = self.in_ctx.tag.as_ref().cloned() {
            // If -a/--append was requested and an album tag exists, append " [<Sample Rate>]" (dot-delimited) to album
            if self.append_rate_suffix {
                self.append_album_suffix(&mut tag);
            }

            if self.out_ctx.output.to_ascii_lowercase() == 'f' {
                self.verbose("Preparing Vorbis Comment for FLAC...", true);
                self.id3_to_flac_meta(&tag);
            }
            self.out_ctx.save_file(&out_path)?;

            if self.out_ctx.output.to_ascii_lowercase() != 'f' {
                let path_out = Path::new(&out_path);
                // Write ID3 tags directly
                self.verbose("Writing ID3 tags to file.", true);
                tag.write_to_path(path_out, tag.version())?;
            }
        } else {
            self.verbose("Input file has no tag; skipping tag copy.", true);
            self.out_ctx.save_file(&out_path)?;
        }

        Ok(())
    }

    fn id3_to_flac_meta(&mut self, tag: &id3::Tag) {
        let unix_datetime = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or_default();
        let mut vorbis = metadata::VorbisComment {
            vendor_string: format!(
                "dsd2dxd v{} Unix datetime {}",
                env!("CARGO_PKG_VERSION"),
                unix_datetime
            ),
            fields: Vec::new(),
        };

        if let Some(artist) = tag.artist() {
            vorbis.insert("ARTIST", artist);
        }
        if let Some(album) = tag.album() {
            vorbis.insert("ALBUM", album);
        }
        if let Some(title) = tag.title() {
            vorbis.insert("TITLE", title);
        }
        if let Some(track) = tag.track() {
            vorbis.insert("TRACKNUMBER", &track.to_string());
        }
        if let Some(disc) = tag.disc() {
            vorbis.insert("DISCNUMBER", &disc.to_string());
        }
        if let Some(year) = tag.year() {
            vorbis.insert("DATE", &year.to_string());
        }
        if let Some(comment_frame) = tag.get("COMM") {
            if let id3::Content::Comment(ref comm) = comment_frame.content() {
                vorbis.insert("COMMENT", &comm.text);
            }
        }

        self.out_ctx.set_vorbis(vorbis);

        for pic in tag.pictures() {
            let pic_type: PictureType = if pic.picture_type == id3::frame::PictureType::CoverFront {
                flac_codec::metadata::PictureType::FrontCover
            } else if pic.picture_type == id3::frame::PictureType::CoverBack {
                flac_codec::metadata::PictureType::BackCover
            } else {
                continue;
            };
            self.verbose(&format!("Adding ID3 Picture: {}", pic), true);
            let picture = flac_codec::metadata::Picture::new(
                pic_type,
                pic.description.clone(),
                pic.data.clone(),
            );
            if let Ok(my_pic) = picture {
                self.out_ctx.add_picture(my_pic);
            }
        }
    }

    fn append_album_suffix(&self, tag: &mut id3::Tag) {
        if let Some(album) = tag.album() {
            if let Some((_uscore, dot)) = self.abbrev_rate_pair(self.out_ctx.rate as u32) {
                let mut new_album = String::from(album);
                new_album.push_str(&format!(" [{}]", dot));
                tag.set_album(new_album);
            }
        }
    }

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
        // 294:1 constraint (must be E; allowed for DSD128 and DSD256)
        if self.decim_ratio == 294
            && !(self.filt_type == 'E' && (self.in_ctx.dsd_rate == 2 || self.in_ctx.dsd_rate == 4))
        {
            return Err("294:1 decimation is only supported for DSD128 or DSD256 with the Equiripple filter.".into());
        }
        // 147:1 constraint (must be E and one of the allowed dsd rates: 64/128/256)
        if self.decim_ratio == 147 && self.filt_type != 'E' {
            return Err("147:1 decimation is only supported with the Equiripple filter.".into());
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
            "[DIAG] DSD bits in per channel: {}  L={}  M={}",
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

    // Helper function for clip stats
    #[inline(always)]
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

    #[inline(always)]
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

    #[inline(always)]
    fn my_round(x: f64) -> i64 {
        if x < 0.0 {
            (x - 0.5).floor() as i64
        } else {
            (x + 0.5).floor() as i64
        }
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

    /// Returns both underscore and dot-delimited abbreviated sample rates, e.g. ("88_2K", "88.2K")
    fn abbrev_rate_pair(&self, rate: u32) -> Option<(&'static str, &'static str)> {
        if !self.append_rate_suffix {
            return None;
        }
        match rate {
            88_200 => Some(("88_2K", "88.2K")),
            96_000 => Some(("96K", "96K")),
            176_400 => Some(("176_4K", "176.4K")),
            192_000 => Some(("192K", "192K")),
            352_800 => Some(("352_8K", "352.8K")),
            384_000 => Some(("384K", "384K")),
            _ => None,
        }
    }
}
