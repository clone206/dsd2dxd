use std::path::Path;

// Re-export only
pub use crate::dsdin_sys::DSD_64_RATE;
use crate::dsdin_sys::{DSD_FORMAT_DSDIFF, DSD_FORMAT_DSF};

pub struct Dsd {
    pub audio_length: i64,
    pub audio_pos: i64,
    pub channel_count: i32,
    pub dsd_rate: i32,
    pub interleaved: bool,
    pub is_lsb: bool,
    pub block_size: u32,
    pub sample_rate: u32,
    pub container_format: u32,
}

impl Dsd {
    // Replace ctor to allocate the opaque C struct using C-reported size, then use getters
    pub fn new(path: String) -> Result<Self, Box<dyn std::error::Error>> {
        let sr: u32;
        let cf: u32;
        let ch: i32;
        let bs: u32;
        let dl: u64;
        let ap: u64;
        let is_lsb: i32;

        if path.to_ascii_lowercase().ends_with(".dsf") {
            use dsf::DsfFile;
            let file_path = Path::new(&path);
            let mut dsf_file = DsfFile::open(file_path)?;
            sr = dsf_file.fmt_chunk().sampling_frequency();
            cf = DSD_FORMAT_DSF;
            ch = dsf_file.fmt_chunk().channel_num() as i32;
            is_lsb = if dsf_file.fmt_chunk().bits_per_sample() == 1 { 1 } else { 0 };
            bs = 4096; // DSF is always planar-per-frame with 4096 byte blocks
            dl = dsf_file.fmt_chunk().sample_count() / 8 * ch as u64; // bits to bytes
            ap = dsf_file.frames()?.offset(0)?;
            let dsd = Self {
                audio_length: dl as i64,
                audio_pos: ap as i64,
                channel_count: ch as i32,
                dsd_rate: match sr {
                    2_822_400 => 1,
                    5_644_800 => 2,
                    _ if sr > 0 && sr % 2_822_400 == 0 => (sr / 2_822_400) as i32,
                    _ => 0, // let InputContext fall back to CLI if needed
                },
                interleaved: match cf {
                    DSD_FORMAT_DSDIFF => true,
                    DSD_FORMAT_DSF => false, // DSF is block-interleaved -> treat as planar-per-frame
                    _ => false,
                },
                is_lsb: is_lsb != 0,
                block_size: bs,
                sample_rate: sr,
                container_format: cf,
            };

            return Ok(dsd);
        } else if path.to_ascii_lowercase().ends_with(".dff") {
            use dff::DffFile;
            let file_path = Path::new(&path);
            let dff_file = DffFile::open(file_path)?;
            sr = dff_file.get_sample_rate()?;
            cf = DSD_FORMAT_DSDIFF;
            ch = dff_file.get_num_channels()? as i32;
            is_lsb = 0; // DFF is always MSB
            bs = 1; // DFF is always interleaved with 1 byte blocks
            dl = dff_file.get_audio_length();
            ap = dff_file.get_dsd_data_offset();
            let dsd = Self {
                audio_length: dl as i64,
                audio_pos: ap as i64,
                channel_count: ch as i32,
                dsd_rate: match sr {
                    2_822_400 => 1,
                    5_644_800 => 2,
                    _ if sr > 0 && sr % 2_822_400 == 0 => (sr / 2_822_400) as i32,
                    _ => 0, // let InputContext fall back to CLI if needed
                },
                interleaved: match cf {
                    DSD_FORMAT_DSDIFF => true,
                    DSD_FORMAT_DSF => false, // DSF is block-interleaved -> treat as planar-per-frame
                    _ => false,
                },
                is_lsb: is_lsb != 0,
                block_size: bs,
                sample_rate: sr,
                container_format: cf,
            };

            return Ok(dsd);
        } else {
            return Err("Unsupported file extension; only .dsf and .dff are supported".into());
        }
    }
}