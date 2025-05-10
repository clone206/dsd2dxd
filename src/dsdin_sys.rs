#![allow(non_upper_case_globals)]
#![allow(non_camel_case_types)]
#![allow(non_snake_case)]

use std::os::raw::{c_char, c_int, c_uint};

#[repr(C)]
pub struct dsd_reader_t {
    pub data_length: i64,
    pub audio_pos: i64,
    pub channel_count: c_int,
    pub is_lsb: c_int,
    pub block_size: c_uint,
    pub sample_rate: c_uint,
    pub container_format: c_uint,
}

extern "C" {
    pub fn dsd_reader_open(file: *mut libc::FILE, reader: *mut dsd_reader_t) -> c_int;
    pub fn dsd_reader_close(reader: *mut dsd_reader_t);
    pub fn dsd_reader_clone(reader: *const dsd_reader_t) -> *mut dsd_reader_t;
    pub fn dsd_reader_read(buf: *mut c_char, len: usize, reader: *mut dsd_reader_t) -> usize;
    pub fn dsd_reader_next_chunk(reader: *mut dsd_reader_t) -> c_uint;
}

#[cfg(target_endian = "big")]
const fn make_marker(a: u8, b: u8, c: u8, d: u8) -> u32 {
    ((a as u32) << 24) | ((b as u32) << 16) | ((c as u32) << 8) | (d as u32)
}

#[cfg(target_endian = "little")]
const fn make_marker(a: u8, b: u8, c: u8, d: u8) -> u32 {
    (a as u32) | ((b as u32) << 8) | ((c as u32) << 16) | ((d as u32) << 24)
}

pub const DSD_FORMAT_DSDIFF: u32 = make_marker(b'F', b'R', b'M', b'8');
pub const DSD_FORMAT_DSF: u32 = make_marker(b'D', b'S', b'D', b' ');