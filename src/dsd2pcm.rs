use std::ptr::NonNull;
use std::sync::atomic::{AtomicBool, Ordering};
use std::marker::PhantomData;
use crate::dsd2pcm_sys::*;

pub struct Dxd {
    ctx: NonNull<dsd2pcm_ctx>,
    dropped: AtomicBool,
    _not_send_sync: PhantomData<*mut ()>,  // Add PhantomData field
}

impl Drop for Dxd {
    fn drop(&mut self) {
        // Only destroy if not already dropped
        if !self.dropped.load(Ordering::SeqCst) {
            unsafe {
                dsd2pcm_destroy(self.ctx.as_ptr());
            }
            self.dropped.store(true, Ordering::SeqCst);
        }
    }
}

impl Dxd {
    pub fn new(
        filt_type: char,
        lsb_first: bool,
        decimation: i32,
        dsd_rate: i32,
    ) -> Result<Self, &'static str> {
        // Validate parameters
        if !['X', 'D', 'E', 'C'].contains(&filt_type.to_ascii_uppercase()) {
            return Err("Invalid filter type. Must be X, D, E, or C");
        }

        if ![8, 16, 32, 64].contains(&decimation) {
            return Err("Invalid decimation ratio. Must be 8, 16, 32, or 64");
        }

        if ![1, 2].contains(&dsd_rate) {
            return Err("Invalid DSD rate. Must be 1 (DSD64) or 2 (DSD128)");
        }

        unsafe {
            let ptr = dsd2pcm_init(
                filt_type.to_ascii_uppercase() as i8,
                if lsb_first { 1 } else { 0 },
                decimation,
                dsd_rate
            );
            
            if ptr.is_null() {
                return Err("Failed to initialize DSD to PCM converter - null pointer returned");
            }

            Ok(Self { 
                ctx: NonNull::new(ptr).ok_or("Failed to create NonNull pointer")?,
                dropped: AtomicBool::new(false),
                _not_send_sync: PhantomData,
            })
        }
    }

    pub fn translate(
        &mut self,
        block_size: usize,
        dsd_data: &[u8],
        dsd_stride: isize,
        float_data: &mut [f64],
        float_stride: isize,
        decimation: i32,
    ) {
        // Don't translate if already dropped
        if self.dropped.load(Ordering::SeqCst) {
            return;
        }

        // Validate buffer sizes
        let min_dsd_len = block_size + (dsd_stride as usize * (block_size - 1));
        let min_float_len = (block_size / decimation as usize) + 
            (float_stride as usize * ((block_size / decimation as usize) - 1));

        if dsd_data.len() < min_dsd_len || float_data.len() < min_float_len {
            return;
        }

        unsafe {
            dsd2pcm_translate(
                self.ctx.as_ptr(),
                block_size,
                dsd_data.as_ptr() as *const i8,
                dsd_stride,
                float_data.as_mut_ptr(),
                float_stride,
                decimation,
            );
        }
    }
}