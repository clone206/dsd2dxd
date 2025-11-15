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
use crate::byte_precalc_decimator::select_precalc_taps;
use crate::dsd::DSD_64_RATE;
use crate::input::InputContext;
use crate::lm_resampler::LMResampler;
use crate::lm_resampler::compute_decim_and_upsample;
use crate::output::OutputContext;
use id3::TagLike;
use log::error;
use log::warn;
use log::{debug, info, trace};
use std::error::Error;
use std::ffi::OsString;
use std::io;
use std::path::Path;
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::Instant;

const RETRIES: usize = 1; // Max retries for progress send
pub const ONE_HUNDRED_PERCENT: f32 = 100.0;

pub struct ConversionContext {
    in_ctx: InputContext,
    out_ctx: OutputContext,
    filt_type: char,
    append_rate_suffix: bool,
    precalc_decims: Option<Vec<BytePrecalcDecimator>>,
    eq_lm_resamplers: Option<Vec<LMResampler>>,
    upsample_ratio: u32,
    decim_ratio: i32,
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
        let dsd_bytes_per_chan = in_ctx.block_size() as usize;
        let (decim_ratio, upsample_ratio) =
            compute_decim_and_upsample(in_ctx.dsd_rate(), out_ctx.rate);

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
            append_rate_suffix,
            precalc_decims: None,
            eq_lm_resamplers: None,
            upsample_ratio,
            decim_ratio,
            diag_expected_frames_floor: 0,
            diag_frames_out: 0,
            base_dir,
        };

        ctx.setup_resamplers()?;
        ctx.out_ctx
            .init(out_frames_capacity, ctx.in_ctx.channels_num())?;

        debug!(
            "Dither type: {}",
            ctx.out_ctx.dither.dither_type().to_ascii_uppercase()
        );
        Ok(ctx)
    }

    /// Get the input file name without the parent path
    pub fn input_file_name(&self) -> String {
        self.in_ctx.file_name().to_string_lossy().into_owned()
    }

    fn setup_resamplers(&mut self) -> Result<(), Box<dyn Error>> {
        if self.upsample_ratio > 1 {
            let mut resamplers =
                Vec::with_capacity(self.in_ctx.channels_num() as usize);
            for _ in 0..self.in_ctx.channels_num() {
                resamplers.push(LMResampler::new(
                    self.upsample_ratio,
                    self.decim_ratio,
                    self.out_ctx.rate as u32,
                )?);
            }
            self.eq_lm_resamplers = Some(resamplers);
            self.out_ctx.scale_factor *= self.upsample_ratio as f64;
            trace!(
                "L/M path makeup gain: ×{} (scale_factor now {:.6})",
                self.upsample_ratio, self.out_ctx.scale_factor
            );
        } else if let Some(taps) = select_precalc_taps(
            self.filt_type,
            self.in_ctx.dsd_rate(),
            self.decim_ratio,
        ) {
            self.precalc_decims = Some(
                (0..self.in_ctx.channels_num())
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
                self.decim_ratio, self.filt_type, self.in_ctx.dsd_rate()
            );
        } else {
            return Err(format!(
                "Taps not found for ratio {} / filter '{}' (dsd_rate {}). ",
                self.decim_ratio, self.filt_type, self.in_ctx.dsd_rate()
            )
            .into());
        }
        Ok(())
    }

    pub fn do_conversion(
        &mut self,
        sender: mpsc::Sender<f32>,
    ) -> Result<(), Box<dyn Error>> {
        self.check_conv()?;
        let wall_start = Instant::now();

        self.process_blocks(&sender)?;

        // Wait for receiver to drop before continuing
        while sender.send(ONE_HUNDRED_PERCENT).is_ok() {
            continue;
        }
        let dsp_elapsed = wall_start.elapsed();

        debug!("Clipped {} times.", self.out_ctx.clips);

        if self.out_ctx.output != 's'
            && let Err(e) = self.write_file()
        {
            error!("Error writing file: {e}");
        }
        let total_elapsed = wall_start.elapsed();

        if self.in_ctx.bytes_processed() > 0 {
            self.report_timing(dsp_elapsed, total_elapsed);
        }

        self.report_in_out();

        Ok(())
    }

    fn process_blocks(
        &mut self,
        sender: &mpsc::Sender<f32>,
    ) -> Result<(), Box<dyn Error>> {
        let channels = self.in_ctx.channels_num() as usize;

        loop {
            // Read one frame identically for stdin and file
            let chan_buffs =
                match self.in_ctx.read_frame() {
                    Ok(buffs) => buffs,
                    Err(e) => {
                        if let Some(io_err) = e.downcast_ref::<io::Error>()
                            && io_err.kind()
                                == io::ErrorKind::UnexpectedEof
                        {
                            break;
                        }
                        return Err(e);
                    }
                };

            // Track actual frames produced per channel (set by channel 0)
            let mut samples_used_per_chan = 0usize;

            // Per-channel processing loop (handles both LM and integer paths)
            for chan in 0..channels {
                // Scope the immutable borrow so it ends before we mutably borrow `self`.
                let chan_bytes: Vec<u8> = chan_buffs[chan].to_vec();
                samples_used_per_chan =
                    self.process_channel(chan, chan_bytes);
                self.out_ctx.write_to_buffer(samples_used_per_chan, chan);
            }

            let pcm_frame_bytes =
                self.track_io(samples_used_per_chan, &sender);

            if self.out_ctx.output == 's' && pcm_frame_bytes > 0 {
                self.out_ctx.write_stdout(pcm_frame_bytes)?;
            }

            if !self.in_ctx.std_in() && self.in_ctx.bytes_remaining() <= 0 {
                break;
            }
        } // end loop

        Ok(())
    }

    /// Collect and send diagnostic info about input/output progress
    fn track_io(
        &mut self,
        samples_used_per_chan: usize,
        sender: &mpsc::Sender<f32>,
    ) -> usize {
        self.diag_expected_frames_floor = (self.in_ctx.chan_bits_processed()
            * self.upsample_ratio as u64)
            / self.decim_ratio as u64;
        self.diag_frames_out += samples_used_per_chan as u64;

        for i in 0..=RETRIES {
            match sender.send(
                (self.in_ctx.bytes_processed() as f32
                    / self.in_ctx.audio_length() as f32)
                    * ONE_HUNDRED_PERCENT,
            ) {
                Ok(_) => break,
                Err(_) => {
                    if i == RETRIES {
                        trace!(
                            "Progress channel blocked after {} retries.",
                            RETRIES
                        );
                    } else {
                        continue;
                    }
                }
            }
        }

        return samples_used_per_chan
            * self.out_ctx.channels_num as usize
            * self.out_ctx.bytes_per_sample as usize;
    }

    // Unified per-channel processing: handles both LM (rational) and integer paths.
    // Returns the number of frames produced into self.float_data for this channel.
    #[inline(always)]
    fn process_channel(
        &mut self,
        chan: usize,
        chan_bytes: Vec<u8>,
    ) -> usize {
        if let Some(resamps) = self.eq_lm_resamplers.as_mut() {
            // LM path: use rational resampler, honor actual produced count
            let rs = &mut resamps[chan];
            return rs.process_bytes_lm(
                &chan_bytes,
                &mut self.out_ctx.float_data,
            );
        } else if let Some(ref mut v) = self.precalc_decims {
            // Integer path: use precalc decimator; conventionally return the estimate
            let dec = &mut v[chan];
            return dec
                .process_bytes(&chan_bytes, &mut self.out_ctx.float_data);
        } else {
            return 0;
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

        if self.in_ctx.std_in() {
            filename.push("output");
            if !suffix.is_empty() {
                filename.push(suffix);
            }
            filename.push(format!(".{}", ext));
            return PathBuf::from(filename);
        }

        filename.push(
            self.in_ctx
                .in_path()
                .clone()
                .and_then(|p| {
                    p.file_stem().map(|stem| stem.to_os_string())
                })
                .unwrap_or_else(|| OsString::from("output")),
        );

        debug!("Derived base filename: {}", filename.to_string_lossy());

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
            if self.in_ctx.std_in() {
                return Ok(out_dir.clone());
            }
            let rel =
                parent.strip_prefix(&self.base_dir).unwrap_or(parent);
            let full_dir = Path::new(out_dir).join(rel);

            if !full_dir.exists() {
                std::fs::create_dir_all(&full_dir)?;
            }
            Ok(full_dir)
        } else if self.in_ctx.std_in() {
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
        if self.out_ctx.path.is_none() || self.in_ctx.std_in() {
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
        debug!("Saving to file...");

        let parent = self
            .in_ctx
            .parent_path()
            .as_ref()
            .map(|p| p.as_path())
            .unwrap_or(Path::new(""));

        let out_dir = self.derive_output_dir(parent)?;
        let out_filename = self.get_out_filename_path();

        let out_path = out_dir.join(&out_filename);

        debug!("Derived output path: {}", out_path.display());

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
                warn!("Failed to copy artwork to output directory: {}", e);
            }
        }

        if let Some(mut tag) = self.in_ctx.tag().as_ref().cloned() {
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
        if let Some(path) = &self.in_ctx.in_path()
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
        let channels = self.in_ctx.channels_num() as u64;
        // Bytes per channel
        let bytes_per_chan = self.in_ctx.bytes_processed() / channels;
        let bits_per_chan = bytes_per_chan * 8;
        let dsd_base_rate =
            (DSD_64_RATE as u64) * (self.in_ctx.dsd_rate() as u64); // samples/sec per channel
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
        debug!(
            "{} bytes processed in {:02}:{:02}:{:02}",
            self.in_ctx.bytes_processed(), h, m, s,
        );
        info!(
            "DSP speed: {:.2}x, End-to-end: {:.2}x",
            speed_dsp, speed_total
        );
    }

    // ---- Diagnostics: expected vs actual output length (verbose only) ----
    fn report_in_out(&self) {
        trace!("Detailed output length diagnostics:");
        let ch = self.in_ctx.channels_num().max(1) as u64;
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
            self.in_ctx.chan_bits_processed(),
            self.upsample_ratio,
            self.decim_ratio
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
