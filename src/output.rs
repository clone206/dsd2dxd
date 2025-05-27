use crate::audio_file::{AudioFile, AudioFileFormat, AudioSample};
use std::error::Error;
use std::path::PathBuf;

#[derive(Clone)]
pub struct OutputContext {
    // Init'd via input params
    pub bits: i32,
    pub channels_num: i32,
    pub rate: i32,
    pub decim_ratio: i32,
    pub bytes_per_sample: i32,
    pub block_size: i32,
    pub pcm_block_size: i32,
    pub out_block_size: i32,
    pub output: char,
    pub filt_type: char,

    // Set freely
    pub peak_level: i32,
    pub scale_factor: f64,

    // Internal state
    float_file: Option<AudioFile<f32>>,
    int_file: Option<AudioFile<i32>>,
}

impl OutputContext {
    pub fn new(
        out_bits: i32,
        out_type: char,
        decimation: i32,
        filt_type_out: char,
        out_vol: f64,
    ) -> Result<Self, &'static str> {
        if ![16, 20, 24, 32].contains(&out_bits) {
            return Err("Unsupported bit depth");
        }

        let output = out_type.to_ascii_lowercase();
        if !['s', 'w', 'a', 'f'].contains(&output) {
            return Err("Unrecognized output type");
        }

        if output == 'f' && out_bits == 32 {
            return Err("32 bit float not allowed with flac output");
        }

        let bytes_per_sample = if out_bits == 20 { 3 } else { out_bits / 8 };

        let mut ctx = Self {
            bits: out_bits,
            output,
            decim_ratio: decimation,
            bytes_per_sample,
            filt_type: filt_type_out.to_ascii_lowercase(),
            channels_num: 0,
            rate: 0,
            block_size: 0,
            pcm_block_size: 0,
            out_block_size: 0,
            peak_level: 0,
            scale_factor: 1.0,
            float_file: None,
            int_file: None,
        };

        ctx.set_scaling(out_vol);
        Ok(ctx)
    }

    pub fn set_block_size(&mut self, block_size_out: i32, chan_num_out: i32) {
        self.channels_num = chan_num_out;
        self.block_size = block_size_out;
        self.pcm_block_size = block_size_out / (self.decim_ratio / 8);
        self.out_block_size = self.pcm_block_size * self.channels_num * self.bytes_per_sample;
    }

    pub fn init_file(&mut self) -> Result<(), Box<dyn Error>> {
        if self.output == 's' {
            return Ok(());
        }

        if self.bits == 32 {
            self.float_file = Some(AudioFile::new());
            self.set_file_params_float();
        } else {
            self.int_file = Some(AudioFile::new());
            self.set_file_params_int();
        }
        Ok(())
    }

    pub fn set_scaling(&mut self, volume: f64) {
        self.scale_factor = 1.0;
        let vol_scale = 10.0f64.powf(volume / 20.0);

        if self.bits != 32 {
            self.scale_factor = 2.0f64.powi(self.bits - 1);
        }

        self.peak_level = self.scale_factor.floor() as i32;
        self.scale_factor *= vol_scale;
    }

    fn set_file_params_float(&mut self) {
        if let Some(file) = &mut self.float_file {
            file.set_num_channels(self.channels_num as usize);
            file.set_bit_depth(self.bits);
            file.set_sample_rate(self.rate as u32);
        }
    }

    fn set_file_params_int(&mut self) {
        if let Some(file) = &mut self.int_file {
            file.set_num_channels(self.channels_num as usize);
            file.set_bit_depth(self.bits);
            file.set_sample_rate(self.rate as u32);
        }
    }

    pub fn save_and_print_file(&self, file_name: &str, fmt: AudioFileFormat) -> Result<(), String> {
        match (self.bits == 32, &self.float_file, &self.int_file) {
            (true, Some(file), _) => {
                file.save(file_name, fmt).map_err(|e| e.to_string())?;
                file.print_summary();
            }
            (false, _, Some(file)) => {
                file.save(file_name, fmt).map_err(|e| e.to_string())?;
                file.print_summary();
            }
            _ => return Err("No file initialized".to_string()),
        }
        
        eprintln!("Wrote to file: {}", file_name);
        Ok(())
    }

    pub fn push_samp<T: AudioSample>(&mut self, samp: T, channel: usize) {
        if self.bits == 32 {
            if let Some(file) = &mut self.float_file {
                file.samples[channel].push(samp.to_f32());
            }
        } else {
            if let Some(file) = &mut self.int_file {
                file.samples[channel].push(samp.to_i32());
            }
        }
    }

    pub fn set_rate(&mut self, new_rate: i32) {
        self.rate = new_rate;
    }
}