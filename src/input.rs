use std::path::{Path, PathBuf};
use std::fs::File;
use crate::dsd::{Dsd, DSD_64_RATE};  // Changed from dsdin to dsd

pub struct InputContext {
    pub verbose_mode: bool,
    pub lsbit_first: i32,
    pub interleaved: bool,
    pub std_in: bool,
    pub dsd_rate: i32,
    pub input: String,
    pub file_path: Option<PathBuf>,
    pub parent_path: Option<PathBuf>,

    pub dsd_stride: i32,
    pub dsd_chan_offset: i32,
    pub channels_num: i32,
    pub block_size: i32,
    pub audio_length: i64,
    pub audio_pos: i64,
}

impl InputContext {
    pub fn new(
        in_file: String,
        fmt: char,
        endianness: char,
        in_rate: i32,
        block_size_in: i32,
        channels: i32,
        verbose_param: bool,
    ) -> Result<Self, &'static str> {
        let lsbit_first = match endianness.to_ascii_lowercase() {
            'l' => 1,
            'm' => 0,
            _ => return Err("No endianness detected!"),
        };

        let interleaved = match fmt.to_ascii_lowercase() {
            'p' => false,
            'i' => true,
            _ => return Err("No fmt detected!"),
        };

        if ![1, 2].contains(&in_rate) {
            return Err("Unsupported DSD input rate.");
        }

        let mut ctx = Self {
            verbose_mode: verbose_param,
            lsbit_first,
            interleaved,
            std_in: in_file == "-",
            dsd_rate: in_rate,
            input: in_file.clone(),
            file_path: None,
            parent_path: None,
            dsd_stride: 0,
            dsd_chan_offset: 0,
            channels_num: channels,
            block_size: block_size_in,
            audio_length: 0,
            audio_pos: 0,
        };

        ctx.set_block_size(block_size_in);

        if !ctx.std_in {
            let path = PathBuf::from(&in_file);
            ctx.file_path = Some(path.clone());
            ctx.parent_path = Some(path.parent().unwrap_or(Path::new("")).to_path_buf());

            ctx.verbose(&format!("Input file basename: {}", 
                path.file_stem().unwrap_or_default().to_string_lossy()), false);
            ctx.verbose(&format!("Parent path: {}", 
                ctx.parent_path.as_ref().unwrap().display()), true);

            if let Ok(file) = File::open(&in_file) {
                if let Ok(my_dsd) = Dsd::new(file) {
                    ctx.audio_pos = my_dsd.audio_pos;
                    ctx.audio_length = my_dsd.audio_length;
                    ctx.channels_num = my_dsd.channel_count;
                    ctx.dsd_rate = my_dsd.dsd_rate;
                    ctx.interleaved = my_dsd.interleaved;
                    ctx.lsbit_first = my_dsd.is_lsb as i32;

                    if my_dsd.block_size > 0 {
                        ctx.verbose("Setting block size from file", true);
                        ctx.set_block_size(my_dsd.block_size as i32);
                    }
                    ctx.verbose(&format!("Audio length in bytes: {}", ctx.audio_length), false);
                }
            }
        }

        Ok(ctx)
    }

    pub fn set_block_size(&mut self, block_size_in: i32) {
        self.block_size = block_size_in;
        self.dsd_chan_offset = if self.interleaved { 1 } else { block_size_in };
        self.dsd_stride = if self.interleaved { self.channels_num } else { 1 };
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

    fn lower_cmp(a: char, b: char) -> bool {
        a.to_ascii_lowercase() == b
    }
}