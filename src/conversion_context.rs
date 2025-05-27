use crate::input::InputContext;
use crate::output::OutputContext;
use crate::dither::Dither;
use crate::dsdin_sys::DSD_64_RATE;
use crate::dsd2pcm::Dxd;
use std::error::Error;

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
        // TODO: Implement file writing
        Ok(())
    }

    pub fn do_conversion(&mut self) -> Result<(), Box<dyn Error>> {
        self.check_conv()?;

        // Initialize vectors
        self.dsd_data.resize(self.in_ctx.block_size as usize, 0);
        self.float_data.resize(
            (self.in_ctx.block_size / self.out_ctx.decim_ratio * self.in_ctx.channels_num) as usize,
            0.0,
        );
        self.pcm_data.resize(self.out_ctx.out_block_size as usize, 0);
        
        // Initialize dxd instances for each channel
        self.dxds.clear();
        for _ in 0..self.in_ctx.channels_num {
            self.dxds.push(crate::dsd2pcm::Dxd::new(
                self.out_ctx.filt_type,
                self.in_ctx.lsbit_first != 0, // Convert i32 to bool
                self.out_ctx.decim_ratio,
                self.in_ctx.dsd_rate,
            )?);
        }

        self.process_blocks()?;
        self.write_file()?;

        Ok(())
    }

    fn process_blocks(&mut self) -> Result<(), Box<dyn Error>> {
        let mut bytes_left = self.in_ctx.audio_length;
        
        while bytes_left > 0 {
            let bytes_to_read = std::cmp::min(bytes_left, self.in_ctx.block_size as i64);
            
            self.verbose(&format!("Reading {} bytes", bytes_to_read), true);
            // TODO: Implement DSD read functionality
            
            for chan in 0..self.in_ctx.channels_num {
                let dxd = &mut self.dxds[chan as usize];
                dxd.translate(
                    bytes_to_read as usize,
                    &self.dsd_data,
                    self.in_ctx.dsd_stride as isize,
                    &mut self.float_data,
                    1,
                    self.out_ctx.decim_ratio,
                );
                
                // Scale and dither
                {
                    let scale_factor = self.out_ctx.scale_factor;
                    for sample in &mut self.float_data {
                        *sample *= scale_factor;
                        self.dither.process_samp(sample, chan);
                    }
                }
                
                // Write to output buffer
                {
                    let peak_level = self.out_ctx.peak_level;
                    let bits = self.out_ctx.bits;
                    let output = self.out_ctx.output;
                    let lsbit_first = self.in_ctx.lsbit_first;
                    let bytes_per_sample = self.out_ctx.bytes_per_sample;
                    let mut clip_stats = ClipStats::default();
                    
                    // Process all samples first to determine clipping
                    let processed_samples: Vec<_> = self.float_data.iter().map(|&sample| {
                        match bits {
                            32 if output != 'f' => ProcessedSample::Float(sample),
                            _ => {
                                let value = Self::my_round(sample);
                                let value_i32 = value.try_into()
                                    .unwrap_or(if value < 0 { i32::MIN } else { i32::MAX });
                                if value_i32 < -peak_level {
                                    clip_stats.low += 1;
                                    ProcessedSample::Int(-peak_level)
                                } else if value_i32 > peak_level {
                                    clip_stats.high += 1;
                                    ProcessedSample::Int(peak_level)
                                } else {
                                    ProcessedSample::Int(value_i32)
                                }
                            }
                        }
                    }).collect();

                    // Update clip statistics
                    if clip_stats.low > 0 || clip_stats.high > 0 {
                        self.update_clip_stats(clip_stats.low > 0, clip_stats.high > 0);
                    }

                    // Write processed samples to buffer
                    let mut out_ptr = &mut self.pcm_data[..];
                    for sample in processed_samples {
                        match sample {
                            ProcessedSample::Float(f) => {
                                let bytes = (f as f32).to_le_bytes();
                                out_ptr[..4].copy_from_slice(&bytes);
                                out_ptr = &mut out_ptr[4..];
                            },
                            ProcessedSample::Int(i) => {
                                if lsbit_first == 1 {
                                    for j in 0..bytes_per_sample {
                                        out_ptr[j as usize] = ((i as u64 >> (j * 8)) & 0xFF) as u8;
                                    }
                                    out_ptr = &mut out_ptr[bytes_per_sample as usize..];
                                } else {
                                    // TODO: Implement MSB-first writing
                                }
                            }
                        }
                    }
                }
            }
            
            bytes_left -= bytes_to_read;
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