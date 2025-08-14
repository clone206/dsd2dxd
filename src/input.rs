use std::path::{Path, PathBuf};
use std::fs::File;
use crate::dsd::{Dsd};
use std::error::Error;
use std::mem; // add
use std::io; // add this
use std::io::{Read, Seek, SeekFrom}; // existing

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
        input_file: String,
        format: char,
        endian: char,
        dsd_rate: i32,
        block_size: i32,
        channels: i32,
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

        if ![1, 2].contains(&dsd_rate) {
            return Err("Unsupported DSD input rate.".into());
        }

        let mut ctx = Self {
            verbose_mode: verbose,
            lsbit_first,
            interleaved,
            std_in: input_file == "-",
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
        };

        ctx.set_block_size(block_size);

        if !ctx.std_in {
            let path = PathBuf::from(&input_file);
            ctx.file_path = Some(path.clone());
            ctx.parent_path = Some(path.parent().unwrap_or(Path::new("")).to_path_buf());

            ctx.verbose(&format!("Input file basename: {}", 
                path.file_stem().unwrap_or_default().to_string_lossy()), true);
            ctx.verbose(&format!("Parent path: {}", 
                ctx.parent_path.as_ref().unwrap().display()), true);

            ctx.verbose(&format!("Opening input file: {}", input_file), true);

            if let Ok(file) = File::open(&input_file) {
                if let Ok(metadata) = file.metadata() {
                    ctx.verbose(&format!("File size: {} bytes", metadata.len()), true);
                }

                let lower_name = input_file.to_ascii_lowercase();
                let use_container = lower_name.ends_with(".dsf") || lower_name.ends_with(".dff");

                if use_container && lower_name.ends_with(".dsf") {
                    // Parse DSF container header in Rust (avoid FFI double-close/segfault)
                    match parse_dsf_header(Path::new(&input_file)) {
                        Ok(h) => {
                            ctx.audio_pos    = h.audio_pos as i64;
                            ctx.audio_length = h.audio_len as i64;
                            ctx.channels_num = h.channels as i32;

                            // DSF stores actual sampling frequency (e.g., 2_822_400 for DSD64)
                            ctx.dsd_rate = match h.sampling_freq {
                                2_822_400 => 1,
                                5_644_800 => 2,
                                other => (other / 2_822_400) as i32,
                            };

                            // DSF is LSBit-first, block-interleaved. Treat as planar-per-frame.
                            ctx.lsbit_first = 1;
                            ctx.interleaved = false; // was true; block-interleaved -> stride=1, offset=block_size

                            // Use the container’s block size per channel
                            ctx.set_block_size(h.block_size_per_channel as i32);

                            ctx.verbose(&format!("Audio length in bytes: {}", ctx.audio_length), false);
                            ctx.verbose(
                                &format!(
                                    "DSF: channels={} fs={}Hz block_size/ch={} audio_pos={} audio_len={}",
                                    h.channels, h.sampling_freq, h.block_size_per_channel, h.audio_pos, h.audio_len
                                ),
                                true,
                            );
                        }
                        Err(e) => {
                            ctx.verbose(&format!("DSF parse failed ({}); treating as raw DSD", e), true);
                            if let Ok(meta) = std::fs::metadata(&input_file) {
                                ctx.audio_pos = 0;
                                ctx.audio_length = meta.len() as i64;
                            }
                        }
                    }
                } else if use_container && lower_name.ends_with(".dff") {
                    // Minimal fallback: treat DFF like raw (or add a small DFF parser similarly)
                    ctx.verbose("DFF container not parsed; treating as raw DSD", true);
                    if let Ok(meta) = std::fs::metadata(&input_file) {
                        ctx.audio_pos = 0;
                        ctx.audio_length = meta.len() as i64;
                    }
                } else {
                    // Raw DSD
                    if let Ok(meta) = std::fs::metadata(&input_file) {
                        ctx.audio_pos = 0;
                        ctx.audio_length = meta.len() as i64;
                        ctx.verbose("Treating input as raw DSD (no container)", true);
                    }
                }
            }
        } else {
            // Handle stdin case
            ctx.verbose("Reading from stdin", true);
            ctx.audio_length = i64::MAX;
            ctx.audio_pos = 0;
            ctx.verbose(&format!("Using CLI parameters: {} channels, LSB first: {}, Interleaved: {}", 
                ctx.channels_num, 
                ctx.lsbit_first, 
                ctx.interleaved
            ), true);
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

// Minimal DSF header parser (enough to locate audio data and basic format)
struct DsfHeader {
    audio_pos: u64,
    audio_len: u64,
    channels: u32,
    sampling_freq: u32,
    block_size_per_channel: u32,
}

fn read_u32le<R: Read>(r: &mut R) -> io::Result<u32> {
    let mut b = [0u8; 4];
    r.read_exact(&mut b)?;
    Ok(u32::from_le_bytes(b))
}

fn read_u64le<R: Read>(r: &mut R) -> io::Result<u64> {
    let mut b = [0u8; 8];
    r.read_exact(&mut b)?;
    Ok(u64::from_le_bytes(b))
}

fn parse_dsf_header(path: &Path) -> Result<DsfHeader, Box<dyn Error>> {
    let mut f = File::open(path)?;
    let file_size = f.metadata()?.len();

    // 'DSD ' chunk
    let mut id = [0u8; 4];
    f.read_exact(&mut id)?;
    if &id != b"DSD " {
        return Err("Not a DSF file (missing 'DSD ' chunk)".into());
    }
    let _dsd_size = read_u64le(&mut f)?;     // typically 28
    let _file_size_field = read_u64le(&mut f)?; // total file size (can ignore)
    let _meta_ptr = read_u64le(&mut f)?;     // metadata offset (unused here)

    // 'fmt ' chunk
    f.read_exact(&mut id)?;
    if &id != b"fmt " {
        return Err("DSF missing 'fmt ' chunk".into());
    }
    let fmt_size = read_u64le(&mut f)?;
    let _fmt_version = read_u32le(&mut f)?;
    let _fmt_id      = read_u32le(&mut f)?;
    let _chan_type   = read_u32le(&mut f)?;
    let channels     = read_u32le(&mut f)?;
    let sampling_freq = read_u32le(&mut f)?;
    let _bits_per_sample = read_u32le(&mut f)?; // should be 1
    let _sample_count = read_u64le(&mut f)?;
    let block_size_per_channel = read_u32le(&mut f)?;
    let _reserved = read_u32le(&mut f)?;
    // Skip any extra fmt payload beyond standard 40 bytes
    let fmt_payload_read: i64 = 40; // bytes read after fmt_size
    let fmt_payload_to_skip = (fmt_size as i64) - 12 - fmt_payload_read;
    if fmt_payload_to_skip > 0 {
        f.seek(SeekFrom::Current(fmt_payload_to_skip))?;
    }

    // 'data' chunk
    f.read_exact(&mut id)?;
    if &id != b"data" {
        return Err("DSF missing 'data' chunk".into());
    }
    let data_size = read_u64le(&mut f)?; // chunk size (metadata ptr + audio data)
    let _metadata_ptr2 = read_u64le(&mut f)?;
    let audio_pos = f.seek(SeekFrom::Current(0))?;

    // Two independent ways to compute audio length:
    // 1) from chunk size: data_size = 8 (metadata ptr) + audio_len
    let from_chunk = data_size.saturating_sub(8);
    // 2) from file size and current position
    let from_file = file_size
        .checked_sub(audio_pos)
        .ok_or("Invalid positions for DSF data")?;

    // Use the minimum (robust against writer quirks) and log if they differ (via your verbose prints outside)
    let audio_len = from_chunk.min(from_file);

    Ok(DsfHeader {
        audio_pos,
        audio_len,
        channels,
        sampling_freq,
        block_size_per_channel,
    })
}