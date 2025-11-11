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

use id3::Tag;
use std::{
    fs::{self, File},
    io,
    path::{Path, PathBuf},
};
use log::warn;

// Strongly typed container format
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DsdFileFormat {
    Dsdiff,
    Dsf,
    Raw,
}

pub const DSD_64_RATE: u32 = 2822400;
pub const DFF_BLOCK_SIZE: u32 = 1;
pub const DSF_BLOCK_SIZE: u32 = 4096;

pub const DSD_EXTENSIONS: [&str; 3] = ["dsf", "dff", "dsd"];

pub struct DsdFile {
    pub audio_length: u64,
    pub audio_pos: u64,
    pub channel_count: Option<u32>,
    pub is_lsb: Option<bool>,
    pub block_size: Option<u32>,
    pub sample_rate: Option<u32>,
    pub container_format: DsdFileFormat,
    pub file: File,
    pub tag: Option<Tag>,
}

impl DsdFile {
    pub fn new(
        path: &PathBuf,
        file_format: DsdFileFormat,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        if file_format == DsdFileFormat::Dsf {
            use dsf::DsfFile;
            let file_path = Path::new(&path);
            let mut dsf_file = DsfFile::open(file_path)?;
            if let Some(e) = dsf_file.tag_read_err() {
                warn!(
                    "Attempted read of ID3 tag failed. Partial read attempted: {}",
                    e
                );
            }
            let file = dsf_file.file().try_clone()?;
            Ok(Self {
                sample_rate: Some(
                    dsf_file.fmt_chunk().sampling_frequency(),
                ),
                container_format: DsdFileFormat::Dsf,
                channel_count: Some(
                    dsf_file.fmt_chunk().channel_num() as u32
                ),
                is_lsb: Some(dsf_file.fmt_chunk().bits_per_sample() == 1),
                block_size: Some(DSF_BLOCK_SIZE), // Should always be this value for DSF
                audio_length: dsf_file.fmt_chunk().sample_count() / 8
                    * dsf_file.fmt_chunk().channel_num() as u64,
                audio_pos: dsf_file.frames()?.offset(0)?,
                file,
                tag: dsf_file.id3_tag().clone(),
            })
        } else if file_format == DsdFileFormat::Dsdiff {
            use dff_meta::DffFile;
            use dff_meta::model::*;
            let file_path = Path::new(&path);
            let dff_file = match DffFile::open(file_path) {
                Ok(dff) => dff,
                Err(Error::Id3Error(e, dff_file)) => {
                    warn!(
                        "Attempted read of ID3 tag failed. Partial read attempted: {}",
                        e
                    );
                    dff_file
                }
                Err(e) => {
                    return Err(e.into());
                }
            };
            let file = dff_file.file().try_clone()?;
            Ok(Self {
                sample_rate: Some(dff_file.get_sample_rate()?),
                container_format: DsdFileFormat::Dsdiff,
                channel_count: Some(dff_file.get_num_channels()? as u32),
                is_lsb: Some(false),
                block_size: Some(DFF_BLOCK_SIZE), // Should always be 1 for DFF
                audio_length: dff_file.get_audio_length(),
                audio_pos: dff_file.get_dsd_data_offset(),
                file,
                tag: dff_file.id3_tag().clone(),
            })
        } else if file_format == DsdFileFormat::Raw {
            let Ok(meta) = std::fs::metadata(path) else {
                return Err("Failed to read input file metadata".into());
            };
            Ok(Self {
                sample_rate: None,
                container_format: DsdFileFormat::Raw,
                channel_count: None,
                is_lsb: None,
                block_size: None,
                audio_length: meta.len(),
                audio_pos: 0,
                file: File::open(path)?,
                tag: None,
            })
        } else {
            Err("Unsupported file extension; only .dsf, .dff, and .dsd are supported"
                .into())
        }
    }
}

/// Find all DSD files in the provided paths, optionally recursing into directories
pub fn find_dsd_files(
    paths: &[PathBuf],
    recurse: bool,
) -> io::Result<Vec<PathBuf>> {
    let mut file_paths = Vec::new();
    for path in paths {
        if path.is_dir() {
            if recurse {
                // Recurse into all directory entries
                let entries: Vec<PathBuf> = fs::read_dir(path)?
                    .filter_map(|e| e.ok().map(|d| d.path()))
                    .collect();
                file_paths.extend(find_dsd_files(&entries, recurse)?);
            } else {
                // Non-recursive: include only top-level files that are DSD
                for entry in fs::read_dir(path)? {
                    let entry_path = entry?.path();
                    if entry_path.is_file() && is_dsd_file(&entry_path) {
                        file_paths
                            .push(entry_path.canonicalize()?.clone());
                    }
                }
            }
        } else if path.is_file() && is_dsd_file(path) {
            // Single push site for matching files
            file_paths.push(path.canonicalize()?.clone());
        }
    }
    file_paths.sort();
    file_paths.dedup();
    Ok(file_paths)
}

/// Check if the provided path is a DSD file based on its extension
pub fn is_dsd_file(path: &PathBuf) -> bool {
    if path.is_file()
        && let Some(ext) = path.extension()
        && let ext_lower = ext.to_ascii_lowercase().to_string_lossy()
        && DSD_EXTENSIONS.contains(&ext_lower.as_ref())
    {
        return true;
    }
    false
}
