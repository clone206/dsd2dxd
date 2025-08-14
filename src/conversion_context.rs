use crate::audio_file::AudioFileFormat;
use crate::dither::Dither;
use crate::dsd2pcm::Dxd;
use crate::dsdin_sys::DSD_64_RATE;
use crate::input::InputContext;
use crate::output::OutputContext;
use std::error::Error;
use std::fs::File;
use std::io::{self, Read, Write, Seek, SeekFrom};
use std::path::{Path, PathBuf};

pub struct ConversionContext {
    in_ctx: InputContext,
    out_ctx: OutputContext,
    dither: Dither,

    dsd_data: Vec<u8>,
    float_data: Vec<f64>,
    pcm_data: Vec<u8>,
    dxds: Vec<Dxd>,

    clips: i32,
    last_samps_clipped_low: i32,
    last_samps_clipped_high: i32,
    verbose_mode: bool,
}

impl ConversionContext {
    pub fn new(
        in_ctx: InputContext,
        out_ctx: OutputContext,
        dither: Dither,
        verbose_param: bool,
    ) -> Result<Self, Box<dyn Error>> {
        let dsd_bytes_per_chan = in_ctx.block_size as usize; // bytes per channel
        let channels = in_ctx.channels_num as usize;
        let bytes_per_sample = out_ctx.bytes_per_sample as usize;
        let lsb_first = in_ctx.lsbit_first != 0;
        let dsd_rate = in_ctx.dsd_rate;
        let decim_ratio = out_ctx.decim_ratio;
        let filt_type = out_ctx.filt_type;

        // Decimated PCM samples per channel: 8 DSD bits per byte
        let pcm_samples_per_chan = (dsd_bytes_per_chan * 8) / decim_ratio as usize;

        let mut ctx = Self {
            in_ctx,
            out_ctx,
            dither,
            // Input buffer: bytes_per_chan * channels
            dsd_data: vec![0; dsd_bytes_per_chan * channels],
            // Per-channel float buffer sized for one channel’s decimated output
            float_data: vec![0.0; pcm_samples_per_chan],
            // Interleaved PCM buffer for all channels
            pcm_data: vec![0; pcm_samples_per_chan * channels * bytes_per_sample],
            dxds: (0..channels)
                .map(|_| {
                    Dxd::new(
                        filt_type,
                        lsb_first,
                        decim_ratio,
                        dsd_rate,
                    )
                })
                .collect::<Result<Vec<_>, _>>()?,
            clips: 0,
            last_samps_clipped_low: 0,
            last_samps_clipped_high: 0,
            verbose_mode: verbose_param,
        };

        ctx.out_ctx.set_rate(ctx.calculate_out_rate());
        ctx.out_ctx
            .set_block_size(ctx.in_ctx.block_size, ctx.in_ctx.channels_num);
        ctx.out_ctx.init_file()?;
        ctx.dither.init();

        Ok(ctx)
    }

    fn calculate_out_rate(&self) -> i32 {
        // Convert all values to i32 before operations
        ((DSD_64_RATE as i32) * self.in_ctx.dsd_rate / self.out_ctx.decim_ratio) as i32
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
        let parent = self.in_ctx.parent_path.as_ref().map(|p| p.as_path()).unwrap_or(Path::new(""));
        let stem = self
            .in_ctx
            .file_path
            .as_ref()
            .and_then(|p| p.file_stem())
            .and_then(|s| s.to_str())
            .unwrap_or("output");
        parent.join(format!("{}.{}", stem, ext)).to_string_lossy().into_owned()
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
                'w' => "wav",     // FIX print label
                'a' => "aiff",    // FIX print label
                'f' => "flac",    // FIX print label
                _ => "unknown",
            }
        );
        eprintln!("Decimation Ratio: {}", self.out_ctx.decim_ratio);
        eprintln!("Output Sample Rate: {} Hz", self.out_ctx.rate);
        eprintln!(
            "Filter Type: {}",
            match self.out_ctx.filt_type {
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
        eprintln!("");

        // Process blocks
        self.process_blocks()?;

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

        Ok(())
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
                if bytes_remaining >= frame_size as i64 { frame_size } else { break }
            } else {
                frame_size
            };
            if to_read == 0 { break; }

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

            // Bytes per channel in this read
            let block_remaining: usize = read_size / channels;

            // Decimated PCM samples per channel (UNCHANGED — keep your existing formula)
            let pcm_samples: usize =
                (block_remaining * 8) / (self.out_ctx.decim_ratio as usize);

            // Authoritative frames per channel to produce this block (matches C++ outCtx.pcmBlockSize)
            let pcm_frames_per_chan = self.out_ctx.pcm_block_size as usize;

            // Buffer sizes based on pcm_frames_per_chan (do not change your pcm_samples line)
            if self.float_data.len() < pcm_frames_per_chan {
                self.float_data.resize(pcm_frames_per_chan, 0.0);
            } else {
                self.float_data[..pcm_frames_per_chan].fill(0.0);
            }

            let pcm_block_bytes = pcm_frames_per_chan
                * (self.in_ctx.channels_num as usize)
                * (self.out_ctx.bytes_per_sample as usize);

            if self.pcm_data.len() < pcm_block_bytes {
                self.pcm_data.resize(pcm_block_bytes, 0);
            } else {
                self.pcm_data[..pcm_block_bytes].fill(0);
            }

            // Debug-only span check to avoid OOB in FFI translate()
            for chan in 0..self.in_ctx.channels_num {
                let dsd_chan_offset = chan as usize * chan_offset_base;

                #[cfg(debug_assertions)]
                {
                    let stride_usize = if dsd_stride >= 0 { dsd_stride as usize } else { 0 };
                    if stride_usize > 0 && block_remaining > 0 {
                        let needed = dsd_chan_offset
                            + (block_remaining - 1).saturating_mul(stride_usize)
                            + 1;
                        debug_assert!(
                            needed <= self.dsd_data.len(),
                            "DSD span OOB: need {}, have {} (offset={}, stride={}, block_remaining={})",
                            needed,
                            self.dsd_data.len(),
                            dsd_chan_offset,
                            stride_usize,
                            block_remaining
                        );
                    }
                }

                if let Some(dxd) = self.dxds.get_mut(chan as usize) {
                    dxd.translate(
                        block_remaining,                                   // bytes per channel
                        &self.dsd_data[dsd_chan_offset..],                 // channel start
                        dsd_stride,                                        // stride
                        &mut self.float_data[..pcm_frames_per_chan],       // output floats
                        1,
                        self.out_ctx.decim_ratio,
                    )?;

                    if self.out_ctx.output == 's' {
                        // Pack exactly pcm_frames_per_chan frames to interleaved bytes for stdout
                        let mut pcm_pos = chan as usize * self.out_ctx.bytes_per_sample as usize;
                        for s in 0..pcm_frames_per_chan {
                            let mut qin: f64 = self.float_data[s] * self.out_ctx.scale_factor;
                            self.dither.process_samp(&mut qin, chan as usize);
                            let value = Self::my_round(qin) as i32;
                            let clamped = self.clamp_value(-8_388_608, value, 8_388_607);

                            let mut out_idx = pcm_pos;
                            self.write_int(&mut out_idx, clamped, self.out_ctx.bytes_per_sample as usize);

                            pcm_pos += (self.in_ctx.channels_num as usize)
                                * (self.out_ctx.bytes_per_sample as usize);
                        }
                    } else {
                        // File formats: push samples into AudioFile buffers
                        // Use the same count we produced (pcm_frames_per_chan)
                        if self.out_ctx.bits == 32 {
                            for s in 0..pcm_frames_per_chan {
                                self.out_ctx.push_samp(self.float_data[s] as f32, chan as usize);
                            }
                        } else {
                            for s in 0..pcm_frames_per_chan {
                                let mut qin: f64 = self.float_data[s] * self.out_ctx.scale_factor;
                                self.dither.process_samp(&mut qin, chan as usize);
                                let value = Self::my_round(qin) as i32;
                                let clamped = self.clamp_value(-8_388_608, value, 8_388_607);
                                self.out_ctx.push_samp(clamped, chan as usize);
                            }
                        }
                    }
                }
            }

            // Write the complete interleaved block only for stdout
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
        }

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
        if self.in_ctx.dsd_rate == 2 && ![16, 32, 64].contains(&self.out_ctx.decim_ratio) {
            return Err("Only decimation value of 16, 32, or 64 allowed with dsd128 input.".into());
        } else if self.in_ctx.dsd_rate == 1 && ![8, 16, 32].contains(&self.out_ctx.decim_ratio) {
            return Err("Only decimation value of 8, 16, or 32 allowed with dsd64 input.".into());
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

// Helper types
#[derive(Default)]
struct ClipStats {
    low: i32,
    high: i32,
}

#[derive(Clone, Copy)]
enum ProcessedSample {
    Float(f64),
    Int(i32),
}
