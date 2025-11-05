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

use flac_codec::metadata::{Picture, VorbisComment};

use crate::audio_file::{AudioFile, AudioFileFormat, AudioSample};
use std::error::Error;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::{io, vec};

pub struct OutputContext {
    // Init'd via input params
    pub bits: i32,
    pub channels_num: u32,
    pub rate: i32,
    pub bytes_per_sample: i32,
    pub output: char,
    pub path: Option<PathBuf>,

    // Set freely
    pub peak_level: i32,
    pub scale_factor: f64,

    // Internal state
    float_file: Option<AudioFile<f32>>,
    int_file: Option<AudioFile<i32>>,
    stdout_buf: Vec<u8>,
    vorbis: Option<VorbisComment>,
    pictures: Vec<Picture>,
}

impl OutputContext {
    pub fn new(
        out_bits: i32,
        out_type: char,
        out_vol: f64,
        out_rate: i32,
        out_path: Option<String>,
    ) -> Result<Self, Box<dyn Error>> {
        if ![16, 20, 24, 32].contains(&out_bits) {
            return Err("Unsupported bit depth".into());
        }

        let output = out_type.to_ascii_lowercase();
        if !['s', 'w', 'a', 'f'].contains(&output) {
            return Err("Unrecognized output type".into());
        }

        if output == 's' && out_path.is_some() {
            return Err(
                "Cannot specify output path when outputting to stdout"
                    .into(),
            );
        }

        if out_bits == 32 && output != 's' && output != 'w' {
            return Err(
                "32 bit float only allowed with wav or stdout".into()
            );
        }

        let bytes_per_sample =
            if out_bits == 20 { 3 } else { out_bits / 8 };

        let mut pathbuf_opt = None;
        if let Some(p) = out_path {
            let pb = PathBuf::from(&p);
            if !pb.exists() {
                return Err(format!(
                    "Specified output path does not exist: {}",
                    pb.display()
                )
                .into());
            }
            pathbuf_opt = Some(pb);
        }

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
            stdout_buf: Vec::new(),
            vorbis: None,
            pictures: Vec::new(),
            path: pathbuf_opt,
        };

        ctx.set_scaling(out_vol);
        Ok(ctx)
    }

    pub fn init(
        &mut self,
        out_frames_capacity: usize,
        channels_num: u32,
    ) -> Result<(), Box<dyn Error>> {
        self.channels_num = channels_num;
        if self.output == 's' {
            self.stdout_buf = vec![
                0u8;
                out_frames_capacity
                    * self.channels_num as usize
                    * self.bytes_per_sample as usize
            ];
            return Ok(());
        }
        // Clear for each new output
        self.vorbis = None;
        self.pictures.clear();

        if self.bits == 32 {
            self.float_file = Some(AudioFile::new());
            self.set_file_params_float();
        } else {
            self.int_file = Some(AudioFile::new());
            self.set_file_params_int();
        }
        Ok(())
    }

    pub fn add_picture(&mut self, pic: Picture) {
        self.pictures.push(pic);
    }

    pub fn set_vorbis(&mut self, vorbis: VorbisComment) {
        self.vorbis = Some(vorbis);
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

    pub fn save_file(&self, out_path: &PathBuf) -> Result<(), String> {
        match self.output.to_ascii_lowercase() {
            'w' => {
                self.save_and_print_file(out_path, AudioFileFormat::Wave)?;
            }
            'a' => {
                self.save_and_print_file(out_path, AudioFileFormat::Aiff)?;
            }
            'f' => {
                self.save_and_print_file(out_path, AudioFileFormat::Flac)?;
            }
            _ => {}
        }
        Ok(())
    }

    /// Save audio file, forcibly overwriting any existing file at the target path
    pub fn save_and_print_file(
        &self,
        out_path: &PathBuf,
        fmt: AudioFileFormat,
    ) -> Result<(), String> {
        let path = out_path.as_path();
        if path.exists() {
            // Best effort remove; propagate error if it fails (e.g. permission issues)
            std::fs::remove_file(path).map_err(|e| {
                format!(
                    "Failed to remove existing file '{}': {}",
                    out_path.to_string_lossy(),
                    e
                )
            })?;
        }

        match (self.bits == 32, &self.float_file, &self.int_file) {
            (true, Some(file), _) => {
                file.save(
                    out_path,
                    fmt,
                    self.vorbis.clone(),
                    self.pictures.clone(),
                )
                .map_err(|e| e.to_string())?;
                file.print_summary();
            }
            (false, _, Some(file)) => {
                file.save(
                    out_path,
                    fmt,
                    self.vorbis.clone(),
                    self.pictures.clone(),
                )
                .map_err(|e| e.to_string())?;
                file.print_summary();
            }
            _ => return Err("No file initialized".to_string()),
        }

        eprintln!("Wrote to file: {}", out_path.to_string_lossy());
        Ok(())
    }

    pub fn pack_float(&mut self, offset: &mut usize, sample: f64) {
        // Convert to f32 and write in little-endian
        let bytes = (sample as f32).to_le_bytes();
        self.stdout_buf[*offset..*offset + 4].copy_from_slice(&bytes);
        *offset += 4;
    }

    pub fn pack_int(&mut self, offset: &mut usize, value: i32) {
        if *offset + self.bytes_per_sample as usize > self.stdout_buf.len()
        {
            return;
        }

        match self.bytes_per_sample {
            3 => {
                // 24-bit container (also used for 20-bit). For 20-bit we left-align by shifting 4.
                let mut v = value;
                if self.bits == 20 {
                    v <<= 4; // align 20 significant bits into the top of 24-bit word (LS 4 bits zero)
                }
                self.stdout_buf[*offset] = (v & 0xFF) as u8;
                self.stdout_buf[*offset + 1] = ((v >> 8) & 0xFF) as u8;
                self.stdout_buf[*offset + 2] = ((v >> 16) & 0xFF) as u8;
            }
            2 => {
                let v = value as i16;
                let b = v.to_le_bytes();
                self.stdout_buf[*offset..*offset + 2].copy_from_slice(&b);
            }
            _ => return,
        }
        *offset += self.bytes_per_sample as usize;
    }

    pub fn write_stdout(
        &mut self,
        pcm_bytes: usize,
    ) -> Result<(), Box<dyn Error>> {
        if pcm_bytes == 0 || pcm_bytes > self.stdout_buf.len() {
            return Ok(());
        }

        io::stdout().write_all(&self.stdout_buf[..pcm_bytes])?;
        io::stdout().flush()?;
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
            stdout_buf: self.stdout_buf.clone(),
            vorbis: self.vorbis.clone(),
            pictures: self.pictures.clone(),
            path: self.path.clone(),
        }
    }
}
