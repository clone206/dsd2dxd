use std::path::Path;
use std::io::{self, Write};
use std::fs::File;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum AudioFileFormat {
    Error,
    NotLoaded,
    Wave,
    Aiff,
}

#[derive(Clone)]
pub struct AudioFile<T> {
    pub samples: Vec<Vec<T>>,
    sample_rate: u32,
    bit_depth: i32,
    num_channels: usize,
}

impl<T> AudioFile<T> 
where T: AudioSample
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
            _ => Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Unsupported format"
            )),
        }
    }

    fn save_wave_file<P: AsRef<Path>>(&self, path: P) -> io::Result<()> {
        let mut file = File::create(path)?;
        let channels = self.num_channels as u16;
        let bytes_per_sample = (self.bit_depth / 8) as u16;
        let _block_align = channels * bytes_per_sample;
        let data_size = (self.get_num_samples_per_channel() * 
            self.num_channels * bytes_per_sample as usize) as u32;
        let file_size = data_size + 36; // 36 = size of WAV header

        // RIFF header
        file.write_all(b"RIFF")?;
        file.write_all(&file_size.to_le_bytes())?;
        file.write_all(b"WAVE")?;

        // fmt chunk
        file.write_all(b"fmt ")?;
        file.write_all(&(16u32.to_le_bytes()))?; // chunk size
        let format_tag = if T::is_float() { 3u16 } else { 1u16 }; // 3 = float, 1 = PCM
        file.write_all(&format_tag.to_le_bytes())?;
        file.write_all(&channels.to_le_bytes())?;
        file.write_all(&self.sample_rate.to_le_bytes())?;
        let byte_rate = self.sample_rate as u32 * _block_align as u32;
        file.write_all(&byte_rate.to_le_bytes())?;
        file.write_all(&_block_align.to_le_bytes())?;
        file.write_all(&(self.bit_depth as u16).to_le_bytes())?;

        // data chunk
        file.write_all(b"data")?;
        file.write_all(&data_size.to_le_bytes())?;

        // Write samples
        for i in 0..self.get_num_samples_per_channel() {
            for channel in 0..self.num_channels {
                let sample = self.samples[channel][i];
                match self.bit_depth {
                    16 => file.write_all(&sample.to_i16().to_le_bytes())?,
                    24 => {
                        let value = sample.to_i24();
                        file.write_all(&[
                            (value & 0xFF) as u8,
                            ((value >> 8) & 0xFF) as u8,
                            ((value >> 16) & 0xFF) as u8,
                        ])?
                    },
                    32 if T::is_float() => file.write_all(&sample.to_f32().to_le_bytes())?,
                    32 => file.write_all(&sample.to_i32().to_le_bytes())?,
                    _ => return Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "Unsupported bit depth"
                    )),
                }
            }
        }
        Ok(())
    }

    fn save_aiff_file<P: AsRef<Path>>(&self, path: P) -> io::Result<()> {
        let mut file = File::create(path)?;
        let channels = self.num_channels as u16;
        let bytes_per_sample = (self.bit_depth / 8) as u16;
        let block_align = channels * bytes_per_sample;
        let data_size = (self.get_num_samples_per_channel() * 
            self.num_channels * bytes_per_sample as usize) as u32;

        // FORM chunk
        file.write_all(b"FORM")?;
        file.write_all(&(data_size + 46).to_be_bytes())?; // +46 for header size
        file.write_all(b"AIFF")?;

        // Common chunk
        file.write_all(b"COMM")?;
        file.write_all(&18u32.to_be_bytes())?; // Common chunk size
        file.write_all(&channels.to_be_bytes())?;
        file.write_all(&(self.get_num_samples_per_channel() as u32).to_be_bytes())?;
        file.write_all(&(self.bit_depth as u16).to_be_bytes())?;
        
        // Sample rate as 80-bit extended
        let sample_rate = self.sample_rate as f64;
        let mut extended = [0u8; 10];
        self.encode_extended(sample_rate, &mut extended);
        file.write_all(&extended)?;

        // Sound data chunk
        file.write_all(b"SSND")?;
        file.write_all(&data_size.to_be_bytes())?;
        file.write_all(&0u32.to_be_bytes())?; // offset
        file.write_all(&0u32.to_be_bytes())?; // block size

        // Write samples in big-endian
        for i in 0..self.get_num_samples_per_channel() {
            for channel in 0..self.num_channels {
                let sample = self.samples[channel][i];
                match self.bit_depth {
                    16 => file.write_all(&sample.to_i16().to_be_bytes())?,
                    24 => {
                        let value = sample.to_i24();
                        file.write_all(&[
                            ((value >> 16) & 0xFF) as u8,
                            ((value >> 8) & 0xFF) as u8,
                            (value & 0xFF) as u8,
                        ])?
                    },
                    32 => file.write_all(&sample.to_i32().to_be_bytes())?,
                    _ => return Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "Unsupported bit depth"
                    )),
                }
            }
        }
        Ok(())
    }

    // Helper for AIFF extended float conversion
    fn encode_extended(&self, mut value: f64, buffer: &mut [u8; 10]) {
        if value == 0.0 {
            return;
        }

        let mut exp = 0;
        while value < 1.0 {
            value *= 2.0;
            exp -= 1;
        }
        while value >= 2.0 {
            value /= 2.0;
            exp += 1;
        }

        exp += 16383; // bias
        buffer[0] = ((exp >> 8) & 0xFF) as u8;
        buffer[1] = (exp & 0xFF) as u8;

        value -= 1.0; // Remove hidden bit
        for i in 2..10 {
            value *= 256.0;
            buffer[i] = value as u8;
            value -= buffer[i] as f64;
        }
    }

    pub fn print_summary(&self) {
        println!("Audio File Summary:");
        println!("Bit Depth: {}", self.bit_depth);
        println!("Sample Rate: {}", self.sample_rate);
        println!("Num Channels: {}", self.num_channels);
        println!("Num Samples Per Channel: {}", self.get_num_samples_per_channel());
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
    fn from_float(value: f32) -> Self { value }
    fn to_float(self) -> f32 { self }
    fn to_i16(self) -> i16 { (self.clamp(-1.0, 1.0) * 32767.0) as i16 }
    fn to_i24(self) -> i32 { (self.clamp(-1.0, 1.0) * 8388607.0) as i32 }
    fn to_i32(self) -> i32 { (self.clamp(-1.0, 1.0) * 2147483647.0) as i32 }
    fn to_f32(self) -> f32 { self }
    fn is_float() -> bool { true }
}

impl AudioSample for i32 {
    fn from_float(value: f32) -> Self { (value * 2147483647.0) as i32 }
    fn to_float(self) -> f32 { self as f32 / 2147483647.0 }
    fn to_i16(self) -> i16 { (self >> 16) as i16 }
    fn to_i24(self) -> i32 { self >> 8 }
    fn to_i32(self) -> i32 { self }
    fn to_f32(self) -> f32 { self as f32 / 2147483647.0 }
    fn is_float() -> bool { false }
}