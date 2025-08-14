use std::ffi::CString;
use std::fs::File;
use std::ptr::NonNull;
use std::os::raw::c_char;
use crate::dsdin_sys::{
    dsd_reader_t,
    dsd_reader_close,
    dsd_reader_clone,
    dsd_reader_read,
    dsd_reader_next_chunk,
    dsd_reader_open_str,
    dsd_reader_sizeof,                    // ADD
    dsd_reader_get_data_length,           // ADD
    dsd_reader_get_audio_pos,             // ADD
    dsd_reader_get_channel_count,         // ADD
    dsd_reader_get_sample_rate,           // ADD
    dsd_reader_get_container_format,      // ADD
    dsd_reader_get_is_lsb,                // ADD
    dsd_reader_get_block_size,            // ADD
    DSD_FORMAT_DSDIFF,
    DSD_FORMAT_DSF,
};

// Re-export only
pub use crate::dsdin_sys::DSD_64_RATE;

pub struct Dsd {
    reader: NonNull<dsd_reader_t>,
    _file: File,
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
        unsafe {
            // NUL-terminate path without CString validation
            let mut c_path = path.as_bytes().to_vec();
            c_path.push(0);

            // Allocate the opaque reader with the exact size reported by C
            let sz = dsd_reader_sizeof();
            if sz == 0 {
                return Err("dsd_reader_sizeof() returned 0".into());
            }
            let ptr = libc::malloc(sz) as *mut dsd_reader_t;
            if ptr.is_null() {
                return Err("malloc dsd_reader_t failed".into());
            }

            // Initialize via C helper (C does fopen + dsd_reader_open)
            if dsd_reader_open_str(c_path.as_ptr() as *const c_char, ptr) != 1 {
                libc::free(ptr as *mut libc::c_void);
                return Err("Couldn't init reader via dsd_reader_open_str".into());
            }

            let reader = NonNull::new(ptr).ok_or("Null reader pointer")?;

            // Fetch fields via getters (don’t read struct layout from Rust)
            let sr = dsd_reader_get_sample_rate(reader.as_ptr());
            let cf = dsd_reader_get_container_format(reader.as_ptr());
            let ch = dsd_reader_get_channel_count(reader.as_ptr());
            let is_lsb = dsd_reader_get_is_lsb(reader.as_ptr());
            let bs = dsd_reader_get_block_size(reader.as_ptr());
            let dl = dsd_reader_get_data_length(reader.as_ptr());
            let ap = dsd_reader_get_audio_pos(reader.as_ptr());

            // Independent Rust File handle; avoids fd ownership issues
            let file = File::open(&path)?;

            let dsd = Self {
                reader,
                _file: file,
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

            Ok(dsd)
        }
    }

    pub fn reader_read(&mut self, buf: &mut [u8]) -> usize {
        unsafe { dsd_reader_read(buf.as_mut_ptr() as *mut i8, buf.len(), self.reader.as_ptr()) }
    }

    pub fn reader_next_chunk(&mut self) -> u32 {
        unsafe { dsd_reader_next_chunk(self.reader.as_ptr()) }
    }
}

impl Clone for Dsd {
    fn clone(&self) -> Self {
        unsafe {
            let cloned = dsd_reader_clone(self.reader.as_ptr());
            let reader = NonNull::new(cloned).expect("Couldn't clone reader");
            Self {
                reader,
                _file: self._file.try_clone().expect("Failed to clone file"),
                audio_length: self.audio_length,
                audio_pos: self.audio_pos,
                channel_count: self.channel_count,
                dsd_rate: self.dsd_rate,
                interleaved: self.interleaved,
                is_lsb: self.is_lsb,
                block_size: self.block_size,
                sample_rate: self.sample_rate,
                container_format: self.container_format,
            }
        }
    }
}

impl Drop for Dsd {
    fn drop(&mut self) {
        unsafe {
            dsd_reader_close(self.reader.as_ptr());
            libc::free(self.reader.as_ptr() as *mut libc::c_void);
        }
    }
}