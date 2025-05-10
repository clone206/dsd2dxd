use std::fs::File;
use std::ptr::NonNull;
use std::os::unix::io::AsRawFd;
use libc::FILE;
use crate::dsdin_sys::{dsd_reader_t, dsd_reader_open, dsd_reader_close, 
                       dsd_reader_clone, dsd_reader_read, dsd_reader_next_chunk,
                       DSD_FORMAT_DSDIFF, DSD_FORMAT_DSF}; // Add these to imports

pub struct Dsd {
    reader: NonNull<dsd_reader_t>,
    _file: File, // Keep file alive while reader exists
    pub audio_length: i64,
    pub audio_pos: i64,
    pub channel_count: i32,
    pub dsd_rate: i32,
    pub interleaved: bool,
    pub is_lsb: bool,
    pub block_size: u32,
}

impl Dsd {
    pub fn new(file: File) -> Result<Self, Box<dyn std::error::Error>> {
        unsafe {
            let mut reader = Box::new(std::mem::zeroed::<dsd_reader_t>());
            let file_ptr = libc::fdopen(file.as_raw_fd(), b"rb\0".as_ptr() as *const _);
            
            if dsd_reader_open(file_ptr, &mut *reader) != 1 {
                return Err("Couldn't init reader. Check inputs.".into());
            }

            let reader = NonNull::new(Box::into_raw(reader))
                .ok_or("Failed to create reader")?;

            let dsd = Self {
                reader,
                _file: file,
                audio_length: reader.as_ref().data_length,
                audio_pos: reader.as_ref().audio_pos,
                channel_count: reader.as_ref().channel_count,
                dsd_rate: (reader.as_ref().sample_rate / 44100 / 64) as i32,
                interleaved: match reader.as_ref().container_format {
                    DSD_FORMAT_DSDIFF => true,
                    DSD_FORMAT_DSF => false,
                    _ => false,
                },
                is_lsb: reader.as_ref().is_lsb == 1,
                block_size: reader.as_ref().block_size,
            };

            Ok(dsd)
        }
    }

    pub fn reader_read(&mut self, buf: &mut [u8]) -> usize {
        unsafe {
            dsd_reader_read(
                buf.as_mut_ptr() as *mut i8,
                buf.len(),
                self.reader.as_ptr(),
            )
        }
    }

    pub fn reader_next_chunk(&mut self) -> u32 {
        unsafe {
            dsd_reader_next_chunk(self.reader.as_ptr())
        }
    }
}

impl Clone for Dsd {
    fn clone(&self) -> Self {
        unsafe {
            let reader = NonNull::new(dsd_reader_clone(self.reader.as_ptr()))
                .expect("Couldn't clone reader");
            
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
            }
        }
    }
}

impl Drop for Dsd {
    fn drop(&mut self) {
        unsafe {
            dsd_reader_close(self.reader.as_ptr());
            let _ = Box::from_raw(self.reader.as_ptr());
        }
    }
}