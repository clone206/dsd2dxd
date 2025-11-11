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

use crate::byte_precalc_decimator::BytePrecalcDecimator;
use crate::byte_precalc_decimator::bit_reverse_u8;
use crate::byte_precalc_decimator::select_precalc_taps;
use crate::dsd::DSD_64_RATE;
use crate::input::InputContext;
use crate::lm_resampler::LMResampler;
use crate::lm_resampler::compute_decim_and_upsample;
use crate::output::OutputContext;
use id3::TagLike;
use log::warn;
use log::{debug, info, trace};
use std::error::Error;
use std::ffi::OsString;
use std::io;
use std::path::Path;
use std::path::PathBuf;
use std::time::Instant;

pub struct ConversionContext {
    in_ctx: InputContext,
    out_ctx: OutputContext,
    filt_type: char,
    dsd_data: Vec<u8>,
    append_rate_suffix: bool,
    precalc_decims: Option<Vec<BytePrecalcDecimator>>,
    eq_lm_resamplers: Option<Vec<LMResampler>>,
    total_dsd_bytes_processed: u64,
    upsample_ratio: u32,
    decim_ratio: i32,
    diag_bits_in: u64,
    diag_expected_frames_floor: u64,
    diag_frames_out: u64,
    base_dir: PathBuf,
}

impl ConversionContext {
    pub fn new(
        in_ctx: InputContext,
        out_ctx: OutputContext,
        filt_type: char,
        append_rate_suffix: bool,
        base_dir: PathBuf,
    ) -> Result<Self, Box<dyn Error>> {
        let dsd_bytes_per_chan = in_ctx.block_size as usize;
        let channels = in_ctx.channels_num as usize;

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
            filt_type,
            dsd_data: vec![0; dsd_bytes_per_chan * channels],
            append_rate_suffix,
            precalc_decims: None,
            eq_lm_resamplers: None,
            total_dsd_bytes_processed: 0,
            upsample_ratio,
            decim_ratio,
            diag_bits_in: 0,
            diag_expected_frames_floor: 0,
            diag_frames_out: 0,
            base_dir,
        };

        ctx.setup_resamplers()?;

        ctx.out_ctx
            .init(out_frames_capacity, ctx.in_ctx.channels_num)?;
        Ok(ctx)
    }

    fn setup_resamplers(&mut self) -> Result<(), Box<dyn Error>> {
        if self.upsample_ratio > 1 {
            self.eq_lm_resamplers = Some(
                (0..self.in_ctx.channels_num)
                    .map(|_i| {
                        LMResampler::new(
                            self.upsample_ratio,
                            self.decim_ratio,
                            self.out_ctx.rate as u32,
                        )
                    })
                    .collect(),
            );
            self.out_ctx.scale_factor *= self.upsample_ratio as f64;
            trace!(
                "L/M path makeup gain: ×{} (scale_factor now {:.6})",
                self.upsample_ratio, self.out_ctx.scale_factor
            );
        } else if let Some(taps) = select_precalc_taps(
            self.filt_type,
            self.in_ctx.dsd_rate,
            self.decim_ratio,
        ) {
            self.precalc_decims = Some(
                (0..self.in_ctx.channels_num)
                    .map(|_| {
                        BytePrecalcDecimator::new(
                            taps,
                            self.decim_ratio as u32,
                        )
                        .expect("Precalc BytePrecalcDecimator init failed")
                    })
                    .collect(),
            );
            debug!(
                "Precalc decimator enabled (ratio {}:1, filter '{}', dsd_rate {}).",
                self.decim_ratio, self.filt_type, self.in_ctx.dsd_rate
            );
        } else {
            return Err(format!(
                "Taps not found for ratio {} / filter '{}' (dsd_rate {}). ",
                self.decim_ratio, self.filt_type, self.in_ctx.dsd_rate
            )
            .into());
        }
        Ok(())
    }

    pub fn do_conversion(&mut self) -> Result<(), Box<dyn Error>> {
        self.check_conv()?;
        info!(
            "Dither type: {}",
            self.out_ctx.dither.dither_type().to_ascii_uppercase()
        );
        let wall_start = Instant::now();

        self.process_blocks()?;
        let dsp_elapsed = wall_start.elapsed();

        info!("Clipped {} times.", self.out_ctx.clips);
        info!("");

        if self.out_ctx.output != 's'
            && let Err(e) = self.write_file()
        {
            info!("Error writing file: {e}");
        }
        let total_elapsed = wall_start.elapsed();

        if self.total_dsd_bytes_processed > 0 {
            self.report_timing(dsp_elapsed, total_elapsed);
        }

        self.report_in_out();

        Ok(())
    }

    fn process_blocks(&mut self) -> Result<(), Box<dyn Error>> {
        let channels = self.in_ctx.channels_num as usize;
        let frame_size: usize =
            (self.in_ctx.block_size as usize) * channels;

        // Open a unified reader (stdin or file), seeking if needed
        let reading_from_file = !self.in_ctx.std_in;

        let mut bytes_remaining: u64 = if reading_from_file {
            self.in_ctx.audio_length
        } else {
            frame_size as u64
        };

        loop {
            // Read one frame identically for stdin and file
            let read_size: usize = match self.in_ctx.read_frame(
                bytes_remaining,
                frame_size,
                &mut self.dsd_data,
            ) {
                Ok(s) => s,
                Err(e) => {
                    if let Some(io_err) = e.downcast_ref::<io::Error>()
                        && io_err.kind() == io::ErrorKind::UnexpectedEof
                    {
                        break;
                    }
                    return Err(e);
                }
            };

            self.total_dsd_bytes_processed += read_size as u64;

            // Bytes per channel in this read
            let bytes_per_channel: usize = read_size / channels;

            self.diag_bits_in += (bytes_per_channel as u64) * 8;
            self.diag_expected_frames_floor = (self.diag_bits_in
                * self.upsample_ratio as u64)
                / self.decim_ratio as u64;

            // Track actual frames produced per channel (set by channel 0)
            let mut samples_used_per_chan = 0usize;

            // Per-channel processing loop (handles both LM and integer paths)
            for chan in 0..channels {
                samples_used_per_chan =
                    self.process_channel(chan, bytes_per_channel);
                self.out_ctx.write_to_buffer(samples_used_per_chan, chan);
            }

            // Derive actual byte count (may differ from estimate in rational path)
            let pcm_frame_bytes = samples_used_per_chan
                * channels
                * (self.out_ctx.bytes_per_sample as usize);

            self.diag_frames_out += samples_used_per_chan as u64;

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

    // Unified per-channel processing: handles both LM (rational) and integer paths.
    // Returns the number of frames produced into self.float_data for this channel.
    #[inline(always)]
    fn process_channel(
        &mut self,
        chan: usize,
        bytes_per_channel: usize,
    ) -> usize {
        // Build contiguous per-channel byte buffer with proper bit endianness.
        let dsd_chan_offset = chan * self.in_ctx.dsd_chan_offset as usize;
        let dsd_stride = self.in_ctx.dsd_stride as isize;
        let lsb_first = self.in_ctx.lsbit_first;
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
            return rs.process_bytes_lm(
                &chan_bytes,
                true,
                &mut self.out_ctx.float_data,
            );
        } else {
            // Integer path: use precalc decimator; conventionally return the estimate
            let Some(ref mut v) = self.precalc_decims else {
                return 0;
            };
            let dec = &mut v[chan];
            return dec
                .process_bytes(&chan_bytes, &mut self.out_ctx.float_data);
        }
    }

    fn get_out_filename_path(&self) -> PathBuf {
        let ext = match self.out_ctx.output.to_ascii_lowercase() {
            'w' => "wav",
            'a' => "aif",
            'f' => "flac",
            _ => "out",
        };
        let suffix = if let Some((uscore, _dot)) =
            self.abbrev_rate_pair(self.out_ctx.rate as u32)
        {
            format!("_{}", uscore)
        } else {
            "".to_string()
        };

        let mut filename: OsString = OsString::new();

        if self.in_ctx.std_in {
            filename.push("output");
            if !suffix.is_empty() {
                filename.push(suffix);
            }
            filename.push(format!(".{}", ext));
            return PathBuf::from(filename);
        }

        filename.push(
            self.in_ctx
                .in_path
                .clone()
                .and_then(|p| {
                    p.file_stem().map(|stem| stem.to_os_string())
                })
                .unwrap_or_else(|| OsString::from("output")),
        );

        debug!(
            "Derived base filename: {}",
            filename.to_string_lossy()
        );

        if !suffix.is_empty() {
            filename.push(suffix);
        }
        filename.push(format!(".{}", ext));
        let file_path = PathBuf::from(&filename);
        file_path
    }

    fn derive_output_dir(
        &self,
        parent: &Path,
    ) -> Result<PathBuf, Box<dyn Error>> {
        if let Some(ref out_dir) = self.out_ctx.path {
            if self.in_ctx.std_in {
                return Ok(out_dir.clone());
            }
            let rel =
                parent.strip_prefix(&self.base_dir).unwrap_or(parent);
            let full_dir = Path::new(out_dir).join(rel);

            if !full_dir.exists() {
                std::fs::create_dir_all(&full_dir)?;
            }
            Ok(full_dir)
        } else if self.in_ctx.std_in {
            Ok(PathBuf::from(""))
        } else {
            Ok(parent.to_path_buf())
        }
    }

    fn copy_artwork(
        &self,
        source_dir: &Path,
        destination_dir: &Path,
    ) -> Result<(u32, u32), Box<dyn std::error::Error>> {
        if self.out_ctx.path.is_none() || self.in_ctx.std_in {
            return Ok((0, 0));
        }
        let mut copied: u32 = 0;
        let mut total: u32 = 0;

        for entry in std::fs::read_dir(source_dir)? {
            let entry = entry?;
            let source_path = entry.path();

            if !source_path.is_file() {
                continue;
            } else if let Some(ext) = source_path
                .extension()
                .and_then(|e| e.to_str())
                .map(|s| s.to_ascii_lowercase())
                && (ext == "jpg" || ext == "png" || ext == "jpeg")
            {
                total += 1;
                let file_name =
                    source_path.file_name().ok_or("Invalid file name")?;
                let destination_path = destination_dir.join(file_name);

                // Should be atomic, avoiding TOCTOU issues, and only copy if the file doesn't exist
                match std::fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(destination_path)
                {
                    Ok(mut dest_file) => {
                        // If the file was successfully created (meaning it didn't exist),
                        // copy the contents from the source file.
                        let mut source_file =
                            std::fs::File::open(source_path)?;
                        io::copy(&mut source_file, &mut dest_file)?;
                        copied += 1;
                        continue;
                    }
                    Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
                        continue;
                    }
                    Err(e) => {
                        // Handle other potential errors during file creation.
                        return Err(Box::new(e));
                    }
                }
            }
        }
        Ok((copied, total))
    }

    fn write_file(&mut self) -> Result<(), Box<dyn Error>> {
        info!("Saving to file...");

        let parent = self
            .in_ctx
            .parent_path
            .as_ref()
            .map(|p| p.as_path())
            .unwrap_or(Path::new(""));

        let out_dir = self.derive_output_dir(parent)?;
        let out_filename = self.get_out_filename_path();

        let out_path = out_dir.join(&out_filename);

        debug!(
            "Derived output path: {}",
            out_path.display()
        );

        match self.copy_artwork(parent, &out_dir) {
            Ok((_, total)) if total == 0 => {
                debug!("No artwork files to copy.");
            }
            Ok((copied, total)) => {
                debug!(
                    "Copied {} artwork file(s) out of {}.",
                    copied, total
                );
            }
            Err(e) => {
                warn!(
                    "Failed to copy artwork to output directory: {}",
                    e
                );
            }
        }

        if let Some(mut tag) = self.in_ctx.tag.as_ref().cloned() {
            // If -a/--append was requested and an album tag exists, append " [<Sample Rate>]" (dot-delimited) to album
            if self.append_rate_suffix {
                self.append_album_suffix(&mut tag);
            }

            if self.out_ctx.output.to_ascii_lowercase() == 'f' {
                debug!("Preparing Vorbis Comment for FLAC...");
                self.out_ctx.id3_to_flac_meta(&tag);
            }
            self.out_ctx.save_file(&out_path)?;

            if self.out_ctx.output.to_ascii_lowercase() != 'f' {
                // Write ID3 tags directly
                debug!("Writing ID3 tags to file.");
                tag.write_to_path(&out_path, tag.version())?;
            }
        } else {
            debug!("Input file has no tag; skipping tag copy.");
            self.out_ctx.save_file(&out_path)?;
        }

        Ok(())
    }

    fn append_album_suffix(&self, tag: &mut id3::Tag) {
        if let Some(album) = tag.album() {
            if let Some((_uscore, dot)) =
                self.abbrev_rate_pair(self.out_ctx.rate as u32)
            {
                let mut new_album = String::from(album);
                new_album.push_str(&format!(" [{}]", dot));
                tag.set_album(new_album);
            }
        }
    }

    /// Returns both underscore and dot-delimited abbreviated sample rates, e.g. ("88_2K", "88.2K")
    fn abbrev_rate_pair(
        &self,
        rate: u32,
    ) -> Option<(&'static str, &'static str)> {
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

    fn check_conv(&self) -> Result<(), Box<dyn Error>> {
        if let Some(path) = &self.in_ctx.in_path
            && !path.canonicalize()?.starts_with(&self.base_dir)
        {
            return Err(format!(
                "Input file '{}' is outside the base directory of '{}'.",
                path.display(),
                self.base_dir.display()
            )
            .into());
        }

        Ok(())
    }

    // Report timing & speed
    fn report_timing(
        &self,
        dsp_elapsed: std::time::Duration,
        total_elapsed: std::time::Duration,
    ) {
        let channels = self.in_ctx.channels_num as u64;
        // Bytes per channel
        let bytes_per_chan = self.total_dsd_bytes_processed / channels;
        let bits_per_chan = bytes_per_chan * 8;
        let dsd_base_rate =
            (DSD_64_RATE as u64) * (self.in_ctx.dsd_rate as u64); // samples/sec per channel
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
        info!(
            "{} bytes processed in {:02}:{:02}:{:02}  (DSP speed: {:.2}x, End-to-end: {:.2}x)",
            self.total_dsd_bytes_processed,
            h,
            m,
            s,
            speed_dsp,
            speed_total
        );
    }

    // ---- Diagnostics: expected vs actual output length (verbose only) ----
    fn report_in_out(&self) {
        trace!("Detailed output length diagnostics:");
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
        trace!("Output length accounting:");
        trace!(
            "DSD bits in per channel: {}  L={}  M={}",
            self.diag_bits_in, self.upsample_ratio, self.decim_ratio
        );
        trace!(
            "Expected frames (floor): {}  Actual frames: {}  Diff: {} ({:.5}%)",
            expected_frames, actual_frames, diff_frames, pct
        );
        trace!(
            "Expected bytes: {}  Actual bytes: {}  Diff bytes: {}",
            expected_bytes, actual_bytes, diff_bytes
        );
        trace!(
            "Reason for shortfall: FIR group delay (startup) plus unflushed tail at end. \
No data is lost due to buffer resizing; resizing only adjusts capacity."
        );
    }
}
