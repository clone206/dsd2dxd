#![allow(non_upper_case_globals)]
#![allow(non_camel_case_types)]
#![allow(non_snake_case)]

use std::os::raw::{c_char, c_int, c_uint};

#[repr(C)]
pub struct dsd_reader_t {
    // existing opaque layout; do not rely on field order from Rust
    _private: [u8; 0], // keep opaque if you like; we only use getters
}

extern "C" {
    pub fn dsd_reader_open(file: *mut libc::FILE, reader: *mut dsd_reader_t) -> c_int;
    pub fn dsd_reader_close(reader: *mut dsd_reader_t);
    pub fn dsd_reader_clone(reader: *const dsd_reader_t) -> *mut dsd_reader_t;
    pub fn dsd_reader_read(buf: *mut c_char, len: usize, reader: *mut dsd_reader_t) -> usize;
    pub fn dsd_reader_next_chunk(reader: *mut dsd_reader_t) -> c_uint;

    // New open-by-filename helper
    pub fn dsd_reader_open_str(filename: *const c_char, reader: *mut dsd_reader_t) -> c_int;

    // ADD: size reporter (new, no removals)
    pub fn dsd_reader_sizeof() -> usize;

    // Getters (keep existing; declarations are additive)
    pub fn dsd_reader_get_data_length(reader: *const dsd_reader_t) -> u64;
    pub fn dsd_reader_get_audio_pos(reader: *const dsd_reader_t) -> u64;
    pub fn dsd_reader_get_channel_count(reader: *const dsd_reader_t) -> c_int;
    pub fn dsd_reader_get_sample_rate(reader: *const dsd_reader_t) -> c_uint;
    pub fn dsd_reader_get_container_format(reader: *const dsd_reader_t) -> c_uint;
    pub fn dsd_reader_get_is_lsb(reader: *const dsd_reader_t) -> c_int;
    pub fn dsd_reader_get_block_size(reader: *const dsd_reader_t) -> c_uint;
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
pub const DSD_64_RATE: u32 = 2822400;