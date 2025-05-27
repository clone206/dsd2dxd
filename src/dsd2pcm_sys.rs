#![allow(non_upper_case_globals)]
#![allow(non_camel_case_types)]
#![allow(non_snake_case)]

use std::os::raw::{c_char, c_int};

#[repr(C)]
pub struct dsd2pcm_ctx;

extern "C" {
    pub fn dsd2pcm_init(
        filt_type: c_char,
        lsb_first: c_int,
        decimation: c_int,
        dsd_rate: c_int
    ) -> *mut dsd2pcm_ctx;
    
    pub fn dsd2pcm_clone(ctx: *const dsd2pcm_ctx) -> *mut dsd2pcm_ctx;
    
    pub fn dsd2pcm_destroy(ctx: *mut dsd2pcm_ctx);
    
    pub fn dsd2pcm_translate(
        ctx: *mut dsd2pcm_ctx,
        block_size: usize,
        dsd_data: *const c_char,  // Change this to c_char to match C interface
        dsd_stride: isize,
        float_data: *mut f64,
        float_stride: isize,
        decimation: c_int,
    );
}