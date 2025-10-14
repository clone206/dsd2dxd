use std::{fs::File, path::Path};

use id3::Tag;

// Strongly typed container format
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContainerFormat {
    Dsdiff,
    Dsf,
}

pub const DFF_BLOCK_SIZE: u32 = 1;
pub const DSF_BLOCK_SIZE: u32 = 4096;

pub struct Dsd {
    pub audio_length: u64,
    pub audio_pos: u64,
    pub channel_count: u32,
    pub is_lsb: bool,
    pub block_size: u32,
    pub sample_rate: u32,
    pub container_format: ContainerFormat,
    pub file: File,
    pub tag: Option<Tag>,
}

impl Dsd {
    pub fn new(path: String) -> Result<Self, Box<dyn std::error::Error>> {
        let lower = path.to_ascii_lowercase();

        if lower.ends_with(".dsf") {
            use dsf::DsfFile;
            let file_path = Path::new(&path);
            let mut dsf_file = DsfFile::open(file_path)?;
            let file = dsf_file.file().try_clone()?;
            Ok(Self {
                sample_rate: dsf_file.fmt_chunk().sampling_frequency(),
                container_format: ContainerFormat::Dsf,
                channel_count: dsf_file.fmt_chunk().channel_num() as u32,
                is_lsb: dsf_file.fmt_chunk().bits_per_sample() == 1,
                block_size: DSF_BLOCK_SIZE, // Should always be this value for DSF
                audio_length: dsf_file.fmt_chunk().sample_count() / 8
                    * dsf_file.fmt_chunk().channel_num() as u64,
                audio_pos: dsf_file.frames()?.offset(0)?,
                file,
                tag: dsf_file.id3_tag().clone(),
            })
        } else if lower.ends_with(".dff") {
            use dff::DffFile;
            let file_path = Path::new(&path);
            let dff_file = DffFile::open(file_path)?;
            let file = dff_file.file().try_clone()?;
            Ok(Self {
                sample_rate: dff_file.get_sample_rate()?,
                container_format: ContainerFormat::Dsdiff,
                channel_count: dff_file.get_num_channels()? as u32,
                is_lsb: false,
                block_size: DFF_BLOCK_SIZE, // Should always be 1 for DFF
                audio_length: dff_file.get_audio_length(),
                audio_pos: dff_file.get_dsd_data_offset(),
                file,
                tag: dff_file.id3_tag().clone(),
            })
        } else {
            Err("Unsupported file extension; only .dsf and .dff are supported".into())
        }
    }
}
