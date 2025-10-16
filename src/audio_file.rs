//=======================================================================
/** @file AudioFile.h
 *  @author Adam Stark
 *  @copyright Copyright (C) 2017  Adam Stark
 *
 * This file is part of the 'AudioFile' library
 *
 * MIT License
 *
 * Copyright (c) 2017 Adam Stark
 *
 * Permission is hereby granted, free of charge, to any person obtaining a copy 
 * of this software and associated documentation files (the "Software"), to deal 
 * in the Software without restriction, including without limitation the rights 
 * to use, copy, modify, merge, publish, distribute, sublicense, and/or sell copies 
 * of the Software, and to permit persons to whom the Software is furnished to do so, 
 * subject to the following conditions:
 *
 * The above copyright notice and this permission notice shall be included in all 
 * copies or substantial portions of the Software.
 *
 * THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR IMPLIED, 
 * INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A 
 * PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE AUTHORS OR COPYRIGHT 
 * HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION 
 * OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR IN CONNECTION WITH THE 
 * SOFTWARE OR THE USE OR OTHER DEALINGS IN THE SOFTWARE.
 */
//=======================================================================

use std::fs::File;
use std::io::{self, BufWriter, Write}; // add BufWriter
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum AudioFileFormat {
    Error,
    NotLoaded,
    Wave,
    Aiff,
    Flac,
}

#[derive(Clone)]
pub struct AudioFile<T> {
    pub samples: Vec<Vec<T>>,
    sample_rate: u32,
    bit_depth: i32,
    num_channels: usize,
}

impl<T> AudioFile<T>
where
    T: AudioSample,
{
    pub fn new() -> Self {
        Self {
            samples: vec![],
            sample_rate: 44100,
            bit_depth: 16,
            num_channels: 0,
        }
    }

    pub fn set_num_channels(&mut self, channels: usize) {
        self.num_channels = channels;
        self.samples.resize(channels, Vec::new());
    }

    pub fn set_bit_depth(&mut self, depth: i32) {
        self.bit_depth = depth;
    }

    pub fn set_sample_rate(&mut self, rate: u32) {
        self.sample_rate = rate;
    }

    pub fn get_num_samples_per_channel(&self) -> usize {
        self.samples.first().map_or(0, |channel| channel.len())
    }

    pub fn save<P: AsRef<Path>>(&self, path: P, format: AudioFileFormat) -> io::Result<()> {
        match format {
            AudioFileFormat::Wave => self.save_wave_file(path),
            AudioFileFormat::Aiff => self.save_aiff_file(path),
            AudioFileFormat::Flac => self.save_flac_file(path),
            _ => Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Unsupported format",
            )),
        }
    }

    fn save_wave_file<P: AsRef<Path>>(&self, path: P) -> io::Result<()> {
        let file = File::create(path)?;
        // Use a large buffered writer (1 MiB); adjust if desired
        let mut w = BufWriter::with_capacity(1 << 20, file);

        let channels = self.num_channels as u16;
        let bytes_per_sample = (self.bit_depth / 8) as u16;
        let block_align = channels * bytes_per_sample;
        let frames = self.get_num_samples_per_channel();
        let data_size = (frames * self.num_channels * bytes_per_sample as usize) as u32;
        let file_size = data_size + 36;

        // RIFF header
        w.write_all(b"RIFF")?;
        w.write_all(&file_size.to_le_bytes())?;
        w.write_all(b"WAVE")?;

        // fmt chunk
        w.write_all(b"fmt ")?;
        w.write_all(&16u32.to_le_bytes())?;
        let format_tag = if T::is_float() { 3u16 } else { 1u16 };
        w.write_all(&format_tag.to_le_bytes())?;
        w.write_all(&channels.to_le_bytes())?;
        w.write_all(&self.sample_rate.to_le_bytes())?;
        let byte_rate = self.sample_rate as u32 * block_align as u32;
        w.write_all(&byte_rate.to_le_bytes())?;
        w.write_all(&block_align.to_le_bytes())?;
        w.write_all(&(self.bit_depth as u16).to_le_bytes())?;

        // data chunk
        w.write_all(b"data")?;
        w.write_all(&data_size.to_le_bytes())?;

        // Stream samples in blocks to reduce temporary allocation
        // Choose a frame block size that stays cache friendly
        const FRAME_BLOCK: usize = 16_384;
        let mut buf: Vec<u8> = Vec::with_capacity(FRAME_BLOCK * block_align as usize);

        match self.bit_depth {
            16 => {
                for base in (0..frames).step_by(FRAME_BLOCK) {
                    buf.clear();
                    let end = (base + FRAME_BLOCK).min(frames);
                    for i in base..end {
                        for ch in 0..self.num_channels {
                            let s = self.samples[ch][i].to_i16().to_le_bytes();
                            buf.extend_from_slice(&s);
                        }
                    }
                    w.write_all(&buf)?;
                }
            }
            24 => {
                for base in (0..frames).step_by(FRAME_BLOCK) {
                    buf.clear();
                    let end = (base + FRAME_BLOCK).min(frames);
                    for i in base..end {
                        for ch in 0..self.num_channels {
                            let v = self.samples[ch][i].to_i24();
                            buf.extend_from_slice(&[
                                (v & 0xFF) as u8,
                                ((v >> 8) & 0xFF) as u8,
                                ((v >> 16) & 0xFF) as u8,
                            ]);
                        }
                    }
                    w.write_all(&buf)?;
                }
            }
            32 if T::is_float() => {
                for base in (0..frames).step_by(FRAME_BLOCK) {
                    buf.clear();
                    let end = (base + FRAME_BLOCK).min(frames);
                    for i in base..end {
                        for ch in 0..self.num_channels {
                            let b = self.samples[ch][i].to_f32().to_le_bytes();
                            buf.extend_from_slice(&b);
                        }
                    }
                    w.write_all(&buf)?;
                }
            }
            32 => {
                for base in (0..frames).step_by(FRAME_BLOCK) {
                    buf.clear();
                    let end = (base + FRAME_BLOCK).min(frames);
                    for i in base..end {
                        for ch in 0..self.num_channels {
                            let b = self.samples[ch][i].to_i32().to_le_bytes();
                            buf.extend_from_slice(&b);
                        }
                    }
                    w.write_all(&buf)?;
                }
            }
            _ => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "Unsupported bit depth",
                ))
            }
        }

        w.flush()?;
        Ok(())
    }

    fn save_aiff_file<P: AsRef<Path>>(&self, path: P) -> io::Result<()> {
        let file = File::create(path)?;
        let mut w = BufWriter::with_capacity(1 << 20, file);

        let channels = self.num_channels as u16;
        let bytes_per_sample = (self.bit_depth / 8) as u16;
        let block_align = channels * bytes_per_sample;
        let frames = self.get_num_samples_per_channel();
        let data_size = (frames * self.num_channels * bytes_per_sample as usize) as u32;

        // FORM chunk
        w.write_all(b"FORM")?;
        w.write_all(&(data_size + 46).to_be_bytes())?;
        w.write_all(b"AIFF")?;

        // COMM chunk
        w.write_all(b"COMM")?;
        w.write_all(&18u32.to_be_bytes())?;
        w.write_all(&channels.to_be_bytes())?;
        w.write_all(&(frames as u32).to_be_bytes())?;
        w.write_all(&(self.bit_depth as u16).to_be_bytes())?;

        let sample_rate = self.sample_rate as f64;
        let mut extended = [0u8; 10];
        self.encode_extended(sample_rate, &mut extended);
        w.write_all(&extended)?;

        // SSND chunk
        w.write_all(b"SSND")?;
        w.write_all(&data_size.to_be_bytes())?;
        w.write_all(&0u32.to_be_bytes())?; // offset
        w.write_all(&0u32.to_be_bytes())?; // block size

        const FRAME_BLOCK: usize = 16_384;
        let mut buf: Vec<u8> = Vec::with_capacity(FRAME_BLOCK * block_align as usize);

        match self.bit_depth {
            16 => {
                for base in (0..frames).step_by(FRAME_BLOCK) {
                    buf.clear();
                    let end = (base + FRAME_BLOCK).min(frames);
                    for i in base..end {
                        for ch in 0..self.num_channels {
                            let b = self.samples[ch][i].to_i16().to_be_bytes();
                            buf.extend_from_slice(&b);
                        }
                    }
                    w.write_all(&buf)?;
                }
            }
            24 => {
                for base in (0..frames).step_by(FRAME_BLOCK) {
                    buf.clear();
                    let end = (base + FRAME_BLOCK).min(frames);
                    for i in base..end {
                        for ch in 0..self.num_channels {
                            let v = self.samples[ch][i].to_i24();
                            buf.extend_from_slice(&[
                                ((v >> 16) & 0xFF) as u8,
                                ((v >> 8) & 0xFF) as u8,
                                (v & 0xFF) as u8,
                            ]);
                        }
                    }
                    w.write_all(&buf)?;
                }
            }
            32 => {
                // AIFF branch only used for integer 32-bit (no float path provided here)
                for base in (0..frames).step_by(FRAME_BLOCK) {
                    buf.clear();
                    let end = (base + FRAME_BLOCK).min(frames);
                    for i in base..end {
                        for ch in 0..self.num_channels {
                            let b = self.samples[ch][i].to_i32().to_be_bytes();
                            buf.extend_from_slice(&b);
                        }
                    }
                    w.write_all(&buf)?;
                }
            }
            _ => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "Unsupported bit depth",
                ))
            }
        }

        w.flush()?;
        Ok(())
    }

    fn save_flac_file<P: AsRef<Path>>(&self, path: P) -> io::Result<()> {
        use flac_codec::byteorder::LittleEndian;
        use flac_codec::encode::{FlacByteWriter, Options};

        if self.num_channels == 0 {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "No channels"));
        }
        let frames = self.get_num_samples_per_channel();
        if frames == 0 {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "No samples"));
        }

        // Support 16 or 24 bit (truncate >24).
        let mut bits_per_sample = self.bit_depth;
        if bits_per_sample > 24 {
            bits_per_sample = 24;
        }
        if bits_per_sample != 16 && bits_per_sample != 24 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "FLAC: only 16 or 24-bit supported",
            ));
        }

        let channels = self.num_channels as u32;
        let bps = bits_per_sample as u32;
        let bytes_per_sample = (bits_per_sample / 8) as usize;
        let total_pcm_bytes = frames as u64 * channels as u64 * bytes_per_sample as u64;

        // Create FLAC writer (LittleEndian because we feed little-endian sample bytes)
        let mut flac: FlacByteWriter<_, LittleEndian> = FlacByteWriter::create(
            path.as_ref(),
            Options::default(),
            self.sample_rate,
            bps,
            channels.try_into().unwrap(),
            Some(total_pcm_bytes),
        )
        .map_err(|e| io::Error::new(io::ErrorKind::Other, format!("FLAC create: {e}")))?;

        const FRAME_BLOCK: usize = 16_384;
        let mut buf: Vec<u8> =
            Vec::with_capacity(FRAME_BLOCK * channels as usize * bytes_per_sample);

        if bits_per_sample == 16 {
            for base in (0..frames).step_by(FRAME_BLOCK) {
                buf.clear();
                let end = (base + FRAME_BLOCK).min(frames);
                for i in base..end {
                    for ch in 0..self.num_channels {
                        buf.extend_from_slice(&self.samples[ch][i].to_i16().to_le_bytes());
                    }
                }
                flac.write_all(&buf)?;
            }
        } else {
            // 24-bit
            for base in (0..frames).step_by(FRAME_BLOCK) {
                buf.clear();
                let end = (base + FRAME_BLOCK).min(frames);
                for i in base..end {
                    for ch in 0..self.num_channels {
                        let v = self.samples[ch][i].to_i24();
                        // little-endian 24-bit (LSB first)
                        buf.extend_from_slice(&[
                            (v & 0xFF) as u8,
                            ((v >> 8) & 0xFF) as u8,
                            ((v >> 16) & 0xFF) as u8,
                        ]);
                    }
                }
                flac.write_all(&buf)?;
            }
        }

        flac.finalize()
            .map_err(|e| io::Error::new(io::ErrorKind::Other, format!("FLAC finalize: {e}")))?;
        Ok(())
    }

    // Helper for AIFF extended float conversion
    fn encode_extended(&self, mut value: f64, buffer: &mut [u8; 10]) {
        // Encode AIFF 80-bit extended with the integer bit included in the mantissa.
        // Layout: 1 sign bit, 15-bit exponent (bias 16383), 1-bit integer + 63-bit fraction.
        for b in buffer.iter_mut() {
            *b = 0;
        }
        if value <= 0.0 {
            return;
        }

        // Normalize into [1.0, 2.0)
        let mut exp: i32 = 0;
        while value >= 2.0 {
            value *= 0.5;
            exp += 1;
        }
        while value < 1.0 {
            value *= 2.0;
            exp -= 1;
        }

        // Biased exponent
        let mut exp_field: u16 = (exp + 16383) as u16;

        // Mantissa includes the leading 1-bit (integer bit) in the top of the 64-bit field.
        // Compute with u128 to detect rounding overflow to 2^64, then downcast to u64.
        let mut mant128: u128 = (value * ((1u128 << 63) as f64)).round() as u128;
        if mant128 == (1u128 << 64) {
            // value rounded to 2.0; renormalize
            mant128 = 1u128 << 63;
            exp_field = exp_field.wrapping_add(1);
        }
        let mant: u64 = mant128 as u64;
        // Store big-endian
        buffer[0] = (exp_field >> 8) as u8;
        buffer[1] = (exp_field & 0xFF) as u8;
        buffer[2] = ((mant >> 56) & 0xFF) as u8;
        buffer[3] = ((mant >> 48) & 0xFF) as u8;
        buffer[4] = ((mant >> 40) & 0xFF) as u8;
        buffer[5] = ((mant >> 32) & 0xFF) as u8;
        buffer[6] = ((mant >> 24) & 0xFF) as u8;
        buffer[7] = ((mant >> 16) & 0xFF) as u8;
        buffer[8] = ((mant >> 8) & 0xFF) as u8;
        buffer[9] = (mant & 0xFF) as u8;
    }

    pub fn print_summary(&self) {
        println!("Audio File Summary:");
        println!("Bit Depth: {}", self.bit_depth);
        println!("Sample Rate: {}", self.sample_rate);
        println!("Num Channels: {}", self.num_channels);
        println!(
            "Num Samples Per Channel: {}",
            self.get_num_samples_per_channel()
        );
    }
}

pub trait AudioSample: Copy + Send + Sync {
    fn from_float(value: f32) -> Self;
    fn to_float(self) -> f32;
    fn to_i16(self) -> i16;
    fn to_i24(self) -> i32;
    fn to_i32(self) -> i32;
    fn to_f32(self) -> f32;
    fn is_float() -> bool;
}

impl AudioSample for f32 {
    fn from_float(value: f32) -> Self {
        value
    }
    fn to_float(self) -> f32 {
        self
    }
    fn to_i16(self) -> i16 {
        (self.clamp(-1.0, 1.0) * 32767.0) as i16
    }
    fn to_i24(self) -> i32 {
        (self.clamp(-1.0, 1.0) * 8388607.0) as i32
    }
    fn to_i32(self) -> i32 {
        (self.clamp(-1.0, 1.0) * 2147483647.0) as i32
    }
    fn to_f32(self) -> f32 {
        self
    }
    fn is_float() -> bool {
        true
    }
}

impl AudioSample for i32 {
    fn from_float(value: f32) -> Self {
        (value * 2147483647.0) as i32
    }
    fn to_float(self) -> f32 {
        self as f32 / 2147483647.0
    }
    // For 16-bit output, pipeline already scales to ±32767; clamp and pass through.
    fn to_i16(self) -> i16 {
        let v = self.max(-32768).min(32767);
        v as i16
    }
    fn to_i24(self) -> i32 {
        self
    }
    fn to_i32(self) -> i32 {
        self
    }
    fn to_f32(self) -> f32 {
        self as f32 / 2147483647.0
    }
    fn is_float() -> bool {
        false
    }
}
