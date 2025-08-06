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
    
    // Add buffer sizes as fields to track them consistently
    buffer_sizes: BufferSizes,
    dsd_data: Vec<u8>,
    float_data: Vec<f64>,
    pcm_data: Vec<u8>,
    dxds: Vec<Dxd>,
    
    clips: i32,
    last_samps_clipped_low: i32,
    last_samps_clipped_high: i32,
    verbose_mode: bool,
}

// Separate struct to manage buffer sizes
struct BufferSizes {
    dsd_block: usize,
    float_block: usize,
    pcm_block: usize,
}

impl ConversionContext {
    pub fn new(
        in_ctx: InputContext,
        out_ctx: OutputContext,
        dither: Dither,
        verbose_param: bool,
    ) -> Result<Self, Box<dyn Error>> {
        let mut ctx = Self {  // Make ctx mutable
            in_ctx,
            out_ctx,
            dither,
            buffer_sizes: BufferSizes {
                dsd_block: 0,
                float_block: 0,
                pcm_block: 0,
            },
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

        // Initialize buffers
        ctx.initialize_buffers()?;

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
            self.in_ctx.file_path
                .as_ref()
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or_default()
        };
        eprintln!("Input: {}", input_path);
        eprintln!("Format: {}", if self.in_ctx.interleaved { "Interleaved" } else { "Planar" });
        eprintln!("Channels: {}", self.in_ctx.channels_num);
        eprintln!("LSB First: {}", self.in_ctx.lsbit_first != 0);
        eprintln!("DSD Rate: {}", if self.in_ctx.dsd_rate == 2 { "DSD128" } else { "DSD64" });
        eprintln!("Output Format: {} bit{}", 
            self.out_ctx.bits,
            if self.out_ctx.output == 'f' { " float" } else { "" }
        );
        eprintln!("Output Type: {}", match self.out_ctx.output {
            's' => "stdout",
            'f' => "file",
            _ => "unknown"
        });
        eprintln!("Decimation Ratio: {}", self.out_ctx.decim_ratio);
        eprintln!("Output Sample Rate: {} Hz", self.out_ctx.rate);
        eprintln!("Filter Type: {}", match self.out_ctx.filt_type {
            'X' => "XLD",
            'D' => "Original",
            'E' => "Equiripple",
            'C' => "Chebyshev",
            _ => "Unknown"
        });
        eprintln!("Dither Type: {}", match self.dither.dither_type() {
            't' => "TPDF",
            'r' => "Rectangular",
            'n' => "NJAD",
            'f' => "Float",
            'x' => "None",
            _ => "Unknown"
        });
        eprintln!("Block Size: {} bytes", self.in_ctx.block_size);
        eprintln!("");

        // Print buffer configuration
        eprintln!("\nBuffer Configuration:");
        eprintln!("DSD block size: {} bytes", self.buffer_sizes.dsd_block);
        eprintln!("Float block size: {} samples", self.buffer_sizes.float_block);
        eprintln!("PCM block size: {} bytes", self.buffer_sizes.pcm_block);
        eprintln!("");

        // Process blocks
        self.process_blocks()
    }

    fn initialize_buffers(&mut self) -> Result<(), Box<dyn Error>> {
        // All calculations in usize to avoid type mismatches
        let samples_per_channel = self.in_ctx.block_size as usize;
        let channels = self.in_ctx.channels_num as usize;
        let bytes_per_sample = self.out_ctx.bytes_per_sample as usize;
        
        // Calculate decimated PCM samples per channel
        let pcm_samples_per_channel = samples_per_channel / self.out_ctx.decim_ratio as usize;
        
        // Calculate buffer sizes 
        self.buffer_sizes = BufferSizes {
            // DSD buffer holds raw input for all channels
            dsd_block: samples_per_channel * channels,
            
            // Float buffer holds decimated samples per channel
            float_block: pcm_samples_per_channel,  // Changed: only store one channel at a time
            
            // PCM buffer holds final output for all channels
            pcm_block: pcm_samples_per_channel * channels * bytes_per_sample,
        };

        // Allocate buffers with correct sizes
        self.dsd_data = vec![0; self.buffer_sizes.dsd_block];
        self.float_data = vec![0.0; self.buffer_sizes.float_block];
        self.pcm_data = vec![0; self.buffer_sizes.pcm_block];

        // Initialize DXD processors for each channel
        self.dxds = (0..self.in_ctx.channels_num)
            .map(|_| Dxd::new(
                self.out_ctx.filt_type,
                self.in_ctx.lsbit_first != 0,
                self.out_ctx.decim_ratio,
                self.in_ctx.dsd_rate
            ))
            .collect::<Result<Vec<_>, _>>()?;

        Ok(())
    }

    fn process_blocks(&mut self) -> Result<(), Box<dyn Error>> {
        let mut bytes_left = if self.in_ctx.std_in {
            i64::MAX
        } else {
            self.in_ctx.audio_length
        };

        let dsd_block_size = self.in_ctx.block_size as usize * self.in_ctx.channels_num as usize;
        
        while bytes_left > 0 {
            // Read one complete DSD block for all channels
            let read_size = if self.in_ctx.std_in {
                match io::stdin().read_exact(&mut self.dsd_data[..dsd_block_size]) {
                    Ok(()) => dsd_block_size,
                    Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => break,
                    Err(e) => return Err(Box::new(e))
                }
            } else {
                0
            };

            let samples_per_channel = read_size / self.in_ctx.channels_num as usize;
            let pcm_samples = samples_per_channel / self.out_ctx.decim_ratio as usize;
            let mut pcm_offset = 0;

            // Clear PCM buffer for new block
            self.pcm_data.fill(0);

            // Process each channel
            for chan in 0..self.in_ctx.channels_num {
                // Calculate DSD offset for this channel's data
                let dsd_chan_offset = if self.in_ctx.interleaved {
                    chan as usize
                } else {
                    chan as usize * samples_per_channel
                };

                // Clear float buffer for this channel
                self.float_data.fill(0.0);

                if let Some(dxd) = self.dxds.get_mut(chan as usize) {
                    // Process DSD to PCM for this channel
                    dxd.translate(
                        samples_per_channel,
                        &self.dsd_data[dsd_chan_offset..],
                        if self.in_ctx.interleaved { self.in_ctx.channels_num as isize } else { 1 },
                        &mut self.float_data[..pcm_samples],
                        1,
                        self.out_ctx.decim_ratio
                    )?;

                    // Convert float samples to PCM with bounds checking
                    for s in 0..pcm_samples {
                        let mut sample = self.float_data[s];
                        sample *= self.out_ctx.scale_factor;
                        self.dither.process_samp(&mut sample, chan as usize);
                        
                        let mut pcm_pos = (s * self.in_ctx.channels_num as usize + chan as usize) * 
                                     self.out_ctx.bytes_per_sample as usize;
                        
                        // Ensure we don't write past buffer end
                        if pcm_pos + self.out_ctx.bytes_per_sample as usize <= self.pcm_data.len() {
                            let value = Self::my_round(sample * 8388607.0) as i32;
                            let clamped = self.clamp_value(-8388608, value, 8388607);
                            self.write_int(&mut pcm_pos, clamped, self.out_ctx.bytes_per_sample as usize);
                            pcm_offset = pcm_offset.max(pcm_pos + self.out_ctx.bytes_per_sample as usize);
                        }
                    }
                }
            }

            // Write the complete block only if we have valid data
            if pcm_offset > 0 && pcm_offset <= self.pcm_data.len() {
                self.write_block(pcm_offset)?;
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
        // Bounds check
        if *offset + bytes > self.pcm_data.len() {
            return;
        }
        
        // Write in little-endian format
        for i in 0..bytes {
            self.pcm_data[*offset + i] = ((value >> (8 * i)) & 0xFF) as u8;
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