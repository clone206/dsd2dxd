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

use crate::dsd::{ContainerFormat, Dsd, DFF_BLOCK_SIZE};
use std::error::Error;
use std::fs::File;
use std::path::{Path, PathBuf};

pub struct InputContext {
    pub verbose_mode: bool,
    pub lsbit_first: i32,
    pub interleaved: bool,
    pub std_in: bool,
    pub dsd_rate: i32,
    pub input: String,
    pub file_path: Option<PathBuf>,
    pub parent_path: Option<PathBuf>,

    pub dsd_stride: u32,
    pub dsd_chan_offset: i32,
    pub channels_num: u32,
    pub block_size: u32,
    pub audio_length: u64,
    pub audio_pos: u64,
    pub file: Option<File>,
    pub tag: Option<id3::Tag>,
}

impl InputContext {
    pub fn new(
        input_file: String,
        format: char,
        endian: char,
        dsd_rate: i32,
        block_size: u32,
        channels: u32,
        verbose: bool,
    ) -> Result<Self, Box<dyn Error>> {
        let lsbit_first = match endian.to_ascii_lowercase() {
            'l' => 1,
            'm' => 0,
            _ => return Err("No endianness detected!".into()),
        };

        let interleaved = match format.to_ascii_lowercase() {
            'p' => false,
            'i' => true,
            _ => return Err("No fmt detected!".into()),
        };

        // Determine input kind early
        let lower_name = input_file.to_ascii_lowercase();
        let is_container = lower_name.ends_with(".dsf") || lower_name.ends_with(".dff");
        let is_stdin = input_file == "-";

        // Only enforce CLI dsd_rate for stdin or raw inputs
        if (is_stdin || !is_container) && ![1, 2, 4].contains(&dsd_rate) {
            return Err("Unsupported DSD input rate.".into());
        }

        let mut ctx = Self {
            verbose_mode: verbose,
            lsbit_first,
            interleaved,
            std_in: is_stdin,
            dsd_rate,
            input: input_file.clone(),
            file_path: None,
            parent_path: None,
            dsd_stride: 0,
            dsd_chan_offset: 0,
            channels_num: channels,
            block_size: block_size,
            audio_length: 0,
            audio_pos: 0,
            file: None,
            tag: None,
        };

        ctx.set_block_size(block_size);

        if !ctx.std_in {
            let path = PathBuf::from(&input_file);
            ctx.file_path = Some(path.clone());
            ctx.parent_path = Some(path.parent().unwrap_or(Path::new("")).to_path_buf());

            ctx.verbose(
                &format!(
                    "Input file basename: {}",
                    path.file_stem().unwrap_or_default().to_string_lossy()
                ),
                true,
            );
            ctx.verbose(
                &format!(
                    "Parent path: {}",
                    ctx.parent_path.as_ref().unwrap().display()
                ),
                true,
            );

            ctx.verbose(&format!("Opening input file: {}", input_file), true);

            let lower_name = input_file.to_ascii_lowercase();
            let use_container = lower_name.ends_with(".dsf") || lower_name.ends_with(".dff");

            if use_container {
                match Dsd::new(input_file.clone()) {
                    Ok(my_dsd) => {
                        // Pull raw fields
                        let file_len = my_dsd.file.metadata()?.len();
                        ctx.verbose(&format!("File size: {} bytes", file_len), true);

                        ctx.file = Some(my_dsd.file);
                        ctx.tag = my_dsd.tag;

                        ctx.audio_pos = my_dsd.audio_pos;
                        // Clamp audio_length to what the file can actually contain
                        let max_len: u64 = (file_len - ctx.audio_pos).max(0);
                        ctx.audio_length =
                            if my_dsd.audio_length > 0 && my_dsd.audio_length <= max_len {
                                my_dsd.audio_length
                            } else {
                                max_len
                            };

                        // Channels from container (fallback to CLI on nonsense)
                        ctx.channels_num = if my_dsd.channel_count > 0 {
                            my_dsd.channel_count
                        } else {
                            ctx.channels_num
                        };

                        // Bit order from container
                        ctx.lsbit_first = if my_dsd.is_lsb { 1 } else { 0 };

                        // Interleaving from container (DSF = block-interleaved → treat as planar per frame)
                        match my_dsd.container_format {
                            ContainerFormat::Dsdiff => ctx.interleaved = true,
                            ContainerFormat::Dsf => ctx.interleaved = false,
                        }

                        // Block size from container. Recompute stride/offset.
                        // For dff, which always has a block size per channel of 1, 
                        // we accept the user-supplied or default block size and calculate
                        // the stride accordingly. For DSF, we treat the block size as
                        // representing the block size per channel and override any user
                        // supplied or default values for block size.
                        if my_dsd.block_size > DFF_BLOCK_SIZE {
                            ctx.block_size = my_dsd.block_size;
                        }
                        ctx.set_block_size(ctx.block_size);

                        // DSD rate from container sample_rate if valid (2.8224MHz → 1, 5.6448MHz → 2)
                        if my_dsd.sample_rate == 2_822_400 {
                            ctx.dsd_rate = 1;
                        } else if my_dsd.sample_rate == 5_644_800 {
                            ctx.dsd_rate = 2;
                        } else if my_dsd.sample_rate > 0 && my_dsd.sample_rate % 2_822_400 == 0 {
                            ctx.dsd_rate = (my_dsd.sample_rate / 2_822_400) as i32;
                        } else {
                            // Fallback: keep CLI value (avoid triggering “Invalid DSD rate”)
                            eprintln!(
                                "Container sample_rate {} not standard; keeping CLI dsd_rate={}",
                                my_dsd.sample_rate, ctx.dsd_rate
                            );
                        }

                        ctx.verbose(
                            &format!("Audio length in bytes: {}", ctx.audio_length),
                            true,
                        );
                        eprintln!(
                            "Container: sr={}Hz channels={} interleaved={} block_size/ch={}",
                            my_dsd.sample_rate,
                            ctx.channels_num,
                            ctx.interleaved,
                            ctx.block_size,
                        );
                    }
                    Err(e) => {
                        eprintln!("Container open failed ({})", e);
                        if let Ok(meta) = std::fs::metadata(&input_file) {
                            ctx.audio_pos = 0;
                            ctx.audio_length = meta.len();
                        } else {
                            return Err("Failed to open input file metadata".into());
                        }
                    }
                }
            } else {
                // Raw DSD
                if let Ok(meta) = std::fs::metadata(&input_file) {
                    ctx.audio_pos = 0;
                    ctx.audio_length = meta.len();
                    ctx.file = Some(File::open(&input_file)?);
                    eprintln!("Treating input as raw DSD (no container)");
                }
            }
        } else {
            // Handle stdin case
            eprintln!("Reading from stdin");
            ctx.audio_length = u64::MAX;
            ctx.audio_pos = 0;
            eprintln!(
                "Using CLI parameters: {} channels, LSB first: {}, Interleaved: {}",
                ctx.channels_num, if ctx.lsbit_first == 1 { "true" } else { "false" }, ctx.interleaved
            );
        }

        Ok(ctx)
    }

    pub fn set_block_size(&mut self, block_size_in: u32) {
        self.block_size = block_size_in;
        self.dsd_chan_offset = if self.interleaved { 1 } else { block_size_in as i32 };
        self.dsd_stride = if self.interleaved {
            self.channels_num
        } else {
            1
        };
    }

    fn verbose(&self, say: &str, new_line: bool) {
        if self.verbose_mode {
            if new_line {
                eprintln!("{}", say);
            } else {
                eprint!("{}", say);
            }
        }
    }

    // removed unused lower_cmp helper
}
