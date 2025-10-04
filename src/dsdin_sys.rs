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