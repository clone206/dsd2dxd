use crate::input::InputContext;
use crate::output::OutputContext;
use crate::dither::Dither;
use crate::dsdin_sys::DSD_64_RATE;
use crate::dsd2pcm::Dxd;
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
        out_ctx: OutputContext,  // Take ownership
        dither: Dither,         // Take ownership
        verbose_param: bool,
    ) -> Result<Self, Box<dyn Error>> {
        let mut ctx = Self {
            in_ctx,
            out_ctx,    // No need to clone
            dither,     // No need to clone
            dsd_data: Vec::new(),
            float_data: Vec::new(),
            pcm_data: Vec::new(),
            dxds: Vec::new(),
            clips: 0,
            last_samps_clipped_low: 0,
            last_samps_clipped_high: 0,
            verbose_mode: verbose_param,
        };

        ctx.out_ctx.set_rate(ctx.calculate_out_rate());
        ctx.out_ctx.set_block_size(ctx.in_ctx.block_size, ctx.in_ctx.channels_num);
        ctx.out_ctx.init_file()?;
        ctx.dither.init();

        Ok(ctx)
    }

    fn calculate_out_rate(&self) -> i32 {
        // Convert all values to i32 before operations
        ((DSD_64_RATE as i32) * self.in_ctx.dsd_rate / self.out_ctx.decim_ratio) as i32
    }

    // Add write_file implementation
    fn write_file(&mut self) -> Result<(), Box<dyn Error>> {
        if self.out_ctx.output != 's' {
            if let Some(ref mut file) = self.out_ctx.file {
                file.write_all(&self.pcm_data)?;
            } else {
                self.out_ctx.open_output_file()?;
                if let Some(ref mut file) = self.out_ctx.file {
                    file.write_all(&self.pcm_data)?;
                }
            }
        } else {
            let stdout = io::stdout();
            let mut handle = stdout.lock();
            handle.write_all(&self.pcm_data)?;
            handle.flush()?;
        }
        Ok(())
    }

    pub fn do_conversion(&mut self) -> Result<(), Box<dyn Error>> {
        self.check_conv()?;

        // Calculate buffer sizes correctly - account for interleaving
        let samples_per_channel = self.in_ctx.block_size as usize;
        let dsd_block_size = if self.in_ctx.interleaved {
            samples_per_channel * self.in_ctx.channels_num as usize
        } else {
            samples_per_channel
        };
        
        let float_samples_per_channel = dsd_block_size / self.out_ctx.decim_ratio as usize;
        let float_block_size = float_samples_per_channel * self.in_ctx.channels_num as usize;
        let pcm_block_size = float_block_size * self.out_ctx.bytes_per_sample as usize;

        self.verbose(&format!("DSD block size per channel: {}", dsd_block_size), true);
        self.verbose(&format!("Float block size total: {}", float_block_size), true);
        self.verbose(&format!("PCM block size total: {}", pcm_block_size), true);

        // Initialize vectors with correct sizes
        self.dsd_data = vec![0; dsd_block_size];
        self.float_data = vec![0.0; float_block_size];
        self.pcm_data = vec![0; pcm_block_size];

        // Initialize dxd instances for each channel
        self.dxds = (0..self.in_ctx.channels_num)
            .map(|_| {
                Dxd::new(
                    self.out_ctx.filt_type,
                    self.in_ctx.lsbit_first != 0,
                    self.out_ctx.decim_ratio,
                    self.in_ctx.dsd_rate,
                )
            })
            .collect::<Result<Vec<_>, _>>()?;

        self.process_blocks()?;
        Ok(())
    }

    fn process_blocks(&mut self) -> Result<(), Box<dyn Error>> {
        let mut bytes_left = self.in_ctx.audio_length;
        
        self.verbose(&format!("Starting to process blocks. Total bytes: {}", bytes_left), true);
        
        while bytes_left > 0 {
            let bytes_to_read = std::cmp::min(bytes_left, self.in_ctx.block_size as i64) as usize;
            self.verbose(&format!("Attempting to read {} bytes", bytes_to_read), true);
            
            // Ensure our buffers are large enough
            if bytes_to_read > self.dsd_data.len() {
                self.dsd_data.resize(bytes_to_read, 0);
            }

            // Read input data
            let read_size = if self.in_ctx.std_in {
                match io::stdin().read(&mut self.dsd_data[..bytes_to_read]) {
                    Ok(n) => {
                        if n == 0 {
                            self.verbose("EOF reached", true);
                            break;
                        }
                        self.verbose(&format!("Read {} bytes from stdin", n), true);
                        self.verbose(&format!("First few DSD bytes: {:?}", 
                            &self.dsd_data[..std::cmp::min(10, n)]), true);
                        n
                    }
                    Err(e) => return Err(Box::new(e))
                }
            } else {
                // ... file reading code ...
                0
            };

            // Calculate sizes based on what we actually read
            let bytes_per_channel = read_size / self.in_ctx.channels_num as usize;
            let float_len = bytes_per_channel / self.out_ctx.decim_ratio as usize;
            
            // Ensure float buffer is large enough
            let required_float_size = float_len * self.in_ctx.channels_num as usize;
            if required_float_size > self.float_data.len() {
                self.float_data.resize(required_float_size, 0.0);
            }

            // Process each channel
            for chan in 0..self.in_ctx.channels_num {
                let (dsd_start, dsd_stride) = if self.in_ctx.interleaved {
                    (chan as usize, self.in_ctx.channels_num as usize)
                } else {
                    (chan as usize * bytes_per_channel, 1)
                };

                let float_start = chan as usize * float_len;
                
                // Process DSD to PCM conversion
                if let Some(dxd) = self.dxds.get_mut(chan as usize) {
                    dxd.translate(
                        bytes_per_channel,
                        &self.dsd_data[dsd_start..],
                        dsd_stride as isize,
                        &mut self.float_data[float_start..float_start + float_len],
                        1,
                        self.out_ctx.decim_ratio,
                    );
                }
            }

            // After channel processing, convert float data to PCM
            let mut pcm_offset = 0;
            let peak_level = self.out_ctx.peak_level;
            let scale_factor = self.out_ctx.scale_factor;
            let bits = self.out_ctx.bits;
            let output = self.out_ctx.output;
            let bytes_per_sample = self.out_ctx.bytes_per_sample;

            // Process samples in chunks to avoid borrow conflict
            let samples: Vec<_> = self.float_data[..required_float_size]
                .iter()
                .map(|&sample| {
                    let mut sample = sample * scale_factor;
                    self.dither.process_samp(&mut sample, 0);
                    sample
                })
                .collect();

            // Convert processed samples to PCM
            for &sample in &samples {
                if bits == 32 && output != 'f' {
                    let value = Self::my_round(sample) as i32;
                    let clamped = self.clamp(-peak_level, value, peak_level);
                    Self::write_lsbf(
                        &mut &mut self.pcm_data[pcm_offset..], 
                        clamped as u64, 
                        bytes_per_sample
                    );
                } else {
                    Self::write_float(&mut &mut self.pcm_data[pcm_offset..], sample);
                }
                pcm_offset += bytes_per_sample as usize;
            }

            // Write the processed block
            if pcm_offset > 0 {
                self.write_file()?;
            }

            bytes_left -= read_size as i64;
            self.verbose(&format!("Processed {} bytes, {} remaining", read_size, bytes_left), true);
        }

        Ok(())
    }

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
            (x - 0.5) as i64
        } else {
            (x + 0.5) as i64
        }
    }

    fn write_float<'a>(ptr: &'a mut &'a mut [u8], sample: f64) {
        let bytes = (sample as f32).to_le_bytes();
        ptr[..4].copy_from_slice(&bytes);
        *ptr = &mut ptr[4..];
    }

    fn write_lsbf<'a>(ptr: &'a mut &'a mut [u8], word: u64, bytes_per_sample: i32) {
        for i in 0..bytes_per_sample {
            ptr[i as usize] = ((word >> (i * 8)) & 0xFF) as u8;
        }
        *ptr = &mut ptr[bytes_per_sample as usize..];
    }

    fn clamp<T: PartialOrd + Copy>(&mut self, min: T, v: T, max: T) -> T {
        if v < min {
            if self.last_samps_clipped_low == 1 {
                self.clips += 1;
            }
            self.last_samps_clipped_low += 1;
            min
        } else if v > max {
            if self.last_samps_clipped_high == 1 {
                self.clips += 1;
            }
            self.last_samps_clipped_high += 1;
            max
        } else {
            v
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