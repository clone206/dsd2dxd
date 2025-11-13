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

use crate::dsd::{DFF_BLOCK_SIZE, DSD_64_RATE, DsdFile, DsdFileFormat};
use log::{debug, info};
use std::error::Error;
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

pub struct InputContext {
    pub lsbit_first: bool,
    pub std_in: bool,
    pub dsd_rate: i32,
    pub in_path: Option<PathBuf>,
    pub parent_path: Option<PathBuf>,
    pub dsd_stride: u32,
    pub dsd_chan_offset: u32,
    pub channels_num: u32,
    pub block_size: u32,
    pub audio_length: u64,
    pub tag: Option<id3::Tag>,

    interleaved: bool,
    audio_pos: u64,
    reader: Box<dyn Read + Send>,
    file: Option<File>,
}

impl InputContext {
    pub fn new(
        in_path: Option<PathBuf>,
        format: char,
        endian: char,
        dsd_rate: i32,
        block_size: u32,
        channels: u32,
        std_in: bool,
    ) -> Result<Self, Box<dyn Error>> {
        let lsbit_first = match endian.to_ascii_lowercase() {
            'l' => true,
            'm' => false,
            _ => return Err("No endianness detected!".into()),
        };

        let interleaved = match format.to_ascii_lowercase() {
            'p' => false,
            'i' => true,
            _ => return Err("No fmt detected!".into()),
        };

        let parent_path = if let Some(path) = &in_path {
            if path.is_dir() {
                return Err("Input path cannot be a directory".into());
            }
            Some(path.parent().unwrap_or(Path::new("")).to_path_buf())
        } else {
            None
        };

        let dsd_file_format = if std_in {
            None
        } else if let Some(path) = &in_path
            && let Some(ext_str) = path.extension()
        {
            match ext_str.to_ascii_lowercase().to_string_lossy().as_ref() {
                "dsf" => Some(DsdFileFormat::Dsf),
                "dff" => Some(DsdFileFormat::Dsdiff),
                _ => Some(DsdFileFormat::Raw),
            }
        } else {
            Some(DsdFileFormat::Raw)
        };

        // Only enforce CLI dsd_rate for stdin or raw inputs
        if (std_in || !dsd_file_format.is_some())
            && ![1, 2, 4].contains(&dsd_rate)
        {
            return Err("Unsupported DSD input rate.".into());
        }

        let mut ctx = Self {
            lsbit_first,
            interleaved,
            std_in,
            dsd_rate,
            in_path,
            parent_path,
            dsd_stride: 0,
            dsd_chan_offset: 0,
            channels_num: channels,
            block_size: block_size,
            audio_length: 0,
            audio_pos: 0,
            file: None,
            tag: None,
            reader: Box::new(io::empty()),
        };

        ctx.set_block_size(block_size);

        if ctx.std_in {
            ctx.set_stdin();
        } else if let Some(format) = dsd_file_format {
            ctx.update_from_file(format)?;
        } else {
            return Err("No valid input specified".into());
        }

        ctx.set_reader()?;

        Ok(ctx)
    }

    #[inline(always)]
    pub fn read_frame(
        &mut self,
        bytes_remaining: u64,
        frame_size: usize,
        buff: &mut Vec<u8>,
    ) -> Result<usize, Box<dyn Error>> {
        // stdin always reads frame_size
        let to_read: usize = if !self.std_in {
            if bytes_remaining >= frame_size as u64 {
                frame_size
            } else {
                bytes_remaining as usize
            }
        } else {
            frame_size
        };

        // Read one frame identically for stdin and file
        let read_size: usize =
            match self.reader.read_exact(&mut buff[..to_read]) {
                Ok(()) => to_read,
                Err(e) => return Err(Box::new(e)),
            };

        Ok(read_size)
    }

    fn set_stdin(&mut self) {
        debug!("Reading from stdin");
        self.audio_length = u64::MAX;
        self.audio_pos = 0;
        debug!(
            "Using CLI parameters: {} channels, LSB first: {}, Interleaved: {}",
            self.channels_num,
            if self.lsbit_first == true {
                "true"
            } else {
                "false"
            },
            self.interleaved
        );
    }

    fn update_from_file(
        &mut self,
        dsd_file_format: DsdFileFormat,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let Some(path) = &self.in_path else {
            return Err("No readable input file".into());
        };

        debug!(
            "Opening input file: {}",
            self.in_path.clone().unwrap().to_string_lossy()
        );
        debug!(
            "Parent path: {}",
            self.parent_path.as_ref().unwrap().display()
        );
        match DsdFile::new(path, dsd_file_format) {
            Ok(my_dsd) => {
                // Pull raw fields
                let file_len = my_dsd.file.metadata()?.len();
                debug!("File size: {} bytes", file_len);

                self.file = Some(my_dsd.file);
                self.tag = my_dsd.tag;

                self.audio_pos = my_dsd.audio_pos;
                // Clamp audio_length to what the file can actually contain
                let max_len: u64 = (file_len - self.audio_pos).max(0);
                self.audio_length = if my_dsd.audio_length > 0
                    && my_dsd.audio_length <= max_len
                {
                    my_dsd.audio_length
                } else {
                    max_len
                };

                // Channels from container (fallback to CLI on nonsense)
                if let Some(chans_num) = my_dsd.channel_count {
                    self.channels_num = chans_num;
                }

                // Bit order from container
                if let Some(lsb) = my_dsd.is_lsb {
                    self.lsbit_first = lsb;
                }

                // Interleaving from container (DSF = block-interleaved → treat as planar per frame)
                match my_dsd.container_format {
                    DsdFileFormat::Dsdiff => self.interleaved = true,
                    DsdFileFormat::Dsf => self.interleaved = false,
                    DsdFileFormat::Raw => {}
                }

                // Block size from container. Recompute stride/offset.
                // For dff, which always has a block size per channel of 1,
                // we accept the user-supplied or default block size and calculate
                // the stride accordingly. For DSF, we treat the block size as
                // representing the block size per channel and override any user
                // supplied or default values for block size.
                if let Some(block_size) = my_dsd.block_size
                    && block_size > DFF_BLOCK_SIZE
                {
                    self.block_size = block_size;
                }
                self.set_block_size(self.block_size);

                // DSD rate from container sample_rate if valid (2.8224MHz → 1, 5.6448MHz → 2)
                if let Some(sample_rate) = my_dsd.sample_rate {
                    if sample_rate % DSD_64_RATE == 0 {
                        self.dsd_rate = (sample_rate / DSD_64_RATE) as i32;
                    } else {
                        // Fallback: keep CLI value (avoid triggering “Invalid DSD rate”)
                        info!(
                            "Container sample_rate {} not standard; keeping CLI dsd_rate={}",
                            sample_rate, self.dsd_rate
                        );
                    }
                }

                debug!("Audio length in bytes: {}", self.audio_length);
                debug!(
                    "Container: sr={}Hz channels={} interleaved={}",
                    self.dsd_rate * DSD_64_RATE as i32,
                    self.channels_num,
                    self.interleaved,
                );
            }
            Err(e) if dsd_file_format != DsdFileFormat::Raw => {
                info!("Container open failed with error: {}", e);
                info!("Treating input as raw DSD (no container)");
                self.update_from_file(DsdFileFormat::Raw)?;
            }
            Err(e) => {
                return Err(e);
            }
        }
        Ok(())
    }

    fn set_block_size(&mut self, block_size: u32) {
        self.block_size = block_size;
        self.dsd_chan_offset =
            if self.interleaved { 1 } else { block_size };
        self.dsd_stride = if self.interleaved {
            self.channels_num
        } else {
            1
        };
        debug!(
            "Set block_size={} dsd_chan_offset={} dsd_stride={}",
            self.block_size, self.dsd_chan_offset, self.dsd_stride
        );
    }

    fn set_reader(&mut self) -> Result<(), Box<dyn Error>> {
        if self.std_in {
            // Use Stdin (not StdinLock) so the reader is 'static + Send for threaded use
            self.reader = Box::new(io::stdin());
            return Ok(());
        }
        // Obtain an owned File by cloning the handle from InputContext, then seek if needed.
        let mut file = self
            .file
            .as_ref()
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::Other,
                    "Missing input file handle",
                )
            })?
            .try_clone()?;

        if self.audio_pos > 0 {
            file.seek(SeekFrom::Start(self.audio_pos as u64))?;
            debug!(
                "Seeked to audio start position: {}",
                file.stream_position()?
            );
        }
        self.reader = Box::new(file);
        Ok(())
    }
}
