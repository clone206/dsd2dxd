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

use crate::audio_file::{AudioFile, AudioFileFormat, AudioSample};
use std::error::Error;
use std::fs::File;
use std::path::Path;

pub struct OutputContext {
    // Init'd via input params
    pub bits: i32,
    pub channels_num: u32,
    pub rate: i32,
    pub bytes_per_sample: i32,
    pub output: char,

    // Set freely
    pub peak_level: i32,
    pub scale_factor: f64,

    // Internal state
    float_file: Option<AudioFile<f32>>,
    int_file: Option<AudioFile<i32>>,
    pub file: Option<File>,
}

impl OutputContext {
    pub fn new(
        out_bits: i32,
        out_type: char,
        out_vol: f64,
        out_rate: i32,
    ) -> Result<Self, Box<dyn Error>> {
        if ![16, 20, 24, 32].contains(&out_bits) {
            return Err("Unsupported bit depth".into());
        }

        let output = out_type.to_ascii_lowercase();
        if !['s', 'w', 'a', 'f'].contains(&output) {
            return Err("Unrecognized output type".into());
        }

        if output == 'f' && out_bits == 32 {
            return Err("32 bit float not allowed with flac output".into());
        }

        let bytes_per_sample = if out_bits == 20 { 3 } else { out_bits / 8 };

        let mut ctx = Self {
            bits: out_bits,
            output,
            bytes_per_sample,
            channels_num: 0,
            rate: out_rate,
            peak_level: 0,
            scale_factor: 1.0,
            float_file: None,
            int_file: None,
            file: None,
        };

        ctx.set_scaling(out_vol);
        Ok(ctx)
    }

    pub fn set_channels_num(&mut self, chan_num_out: u32) {
        self.channels_num = chan_num_out;
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

    /// Save audio file, forcibly overwriting any existing file at the target path
    /// without relying on backend error strings. Safe for all formats.
    pub fn save_and_print_file(&self, file_name: &str, fmt: AudioFileFormat) -> Result<(), String> {
        let path = Path::new(file_name);
        if path.exists() {
            // Best effort remove; propagate error if it fails (e.g. permission issues)
            std::fs::remove_file(path)
                .map_err(|e| format!("Failed to remove existing file '{}': {}", file_name, e))?;
        }

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
}

impl Clone for OutputContext {
    fn clone(&self) -> Self {
        Self {
            bits: self.bits,
            channels_num: self.channels_num,
            rate: self.rate,
            bytes_per_sample: self.bytes_per_sample,
            output: self.output,
            peak_level: self.peak_level,
            scale_factor: self.scale_factor,
            float_file: self.float_file.clone(),
            int_file: self.int_file.clone(),
            file: None, // File cannot be cloned, so we create a new None
        }
    }
}
