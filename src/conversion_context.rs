use crate::dither::Dither;
use crate::dsd2pcm::Dxd;
use crate::dsdin_sys::DSD_64_RATE;
use crate::input::InputContext;
use crate::output::OutputContext;
use std::error::Error;
use std::fs::File;
use std::io::{self, Read, Write};

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
        // All calculations in usize
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
                'f' => "file",
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
        self.process_blocks()
    }

    fn process_blocks(&mut self) -> Result<(), Box<dyn Error>> {
        let mut bytes_left = if self.in_ctx.std_in {
            i64::MAX
        } else {
            self.in_ctx.audio_length
        };

        while bytes_left > 0 {
            let read_size = if self.in_ctx.std_in {
                match io::stdin().read_exact(&mut self.dsd_data) {
                    Ok(()) => self.in_ctx.block_size * self.in_ctx.channels_num,
                    Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => break,
                    Err(e) => return Err(Box::new(e)),
                }
            } else {
                0
            };

            // Bytes per channel in this block
            let dsd_bytes_per_chan = (read_size as usize) / (self.in_ctx.channels_num as usize);

            // Correct decimated PCM frames per channel: (bytes * 8) / decim
            let pcm_samples = (dsd_bytes_per_chan * 8) / (self.out_ctx.decim_ratio as usize);

            // Ensure float buffer is big enough (and clean it)
            if self.float_data.len() < pcm_samples {
                self.float_data.resize(pcm_samples, 0.0);
            } else {
                self.float_data[..pcm_samples].fill(0.0);
            }

            // Clear PCM buffer slice we will write
            let needed_pcm_bytes = pcm_samples
                * (self.in_ctx.channels_num as usize)
                * (self.out_ctx.bytes_per_sample as usize);
            if self.pcm_data.len() < needed_pcm_bytes {
                self.pcm_data.resize(needed_pcm_bytes, 0);
            } else {
                self.pcm_data[..needed_pcm_bytes].fill(0);
            }

            for chan in 0..self.in_ctx.channels_num {
                // Channel start offset in the DSD byte stream
                let dsd_chan_offset = if self.in_ctx.interleaved {
                    chan as usize
                } else {
                    chan as usize * dsd_bytes_per_chan
                };

                if let Some(dxd) = self.dxds.get_mut(chan as usize) {
                    dxd.translate(
                        dsd_bytes_per_chan,                                // input length in BYTES
                        &self.dsd_data[dsd_chan_offset..],
                        if self.in_ctx.interleaved {
                            self.in_ctx.channels_num as isize
                        } else {
                            1
                        },
                        &mut self.float_data[..pcm_samples],               // output slice
                        1,
                        self.out_ctx.decim_ratio,
                    )?;

                    // Pack to interleaved s24le
                    let mut pcm_pos = chan as usize * self.out_ctx.bytes_per_sample as usize;
                    for s in 0..pcm_samples {
                        // Single scaling: scale_factor already equals 2^23 for 24-bit
                        let mut qin: f64 = self.float_data[s] * self.out_ctx.scale_factor;

                        // Dither in quantizer units, then quantize
                        self.dither.process_samp(&mut qin, chan as usize);
                        let value = Self::my_round(qin) as i32;

                        // Clamp and write 3 bytes LE
                        let clamped = self.clamp_value(-8_388_608, value, 8_388_607);
                        let mut out_idx = pcm_pos;
                        self.write_int(&mut out_idx, clamped, self.out_ctx.bytes_per_sample as usize);

                        pcm_pos += (self.in_ctx.channels_num as usize)
                            * (self.out_ctx.bytes_per_sample as usize);
                    }
                }
            }

            // Write exactly the bytes we produced
            let pcm_block_bytes = needed_pcm_bytes;
            if pcm_block_bytes > 0 {
                self.write_block(pcm_block_bytes)?;
            }

            bytes_left -= read_size as i64;
        }

        Ok(())
    }

    fn write_block(&mut self, pcm_bytes: usize) -> Result<(), Box<dyn Error>> {
        if pcm_bytes == 0 || pcm_bytes > self.pcm_data.len() {
            return Ok(());
        }

        // Write the block
        if self.out_ctx.output != 's' {
            if let Some(ref mut file) = self.out_ctx.file {
                file.write_all(&self.pcm_data[..pcm_bytes])?;
                file.flush()?;
            }
        } else {
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
