use std::ptr::NonNull;
use crate::dsd2pcm_sys::*;

pub struct Dxd {
    handle: NonNull<dsd2pcm_ctx>,
}

impl Dxd {
    pub fn new(filt_type: char, lsb_first: bool, decimation: i32, dsd_rate: i32) 
        -> Result<Self, &'static str> 
    {
        let handle = unsafe {
            NonNull::new(dsd2pcm_init(
                filt_type as i8,
                lsb_first as i32,
                decimation,
                dsd_rate
            )).ok_or("Couldn't init. Check inputs.")?
        };
        
        Ok(Self { handle })
    }

    pub fn translate(&mut self,
        block_size: usize,
        dsd_data: &[u8],
        dsd_stride: isize,
        float_data: &mut [f64],
        float_stride: isize,
        decimation: i32,
    ) {
        unsafe {
            dsd2pcm_translate(
                self.handle.as_ptr(),
                block_size,
                dsd_data.as_ptr(),
                dsd_stride,
                float_data.as_mut_ptr(),
                float_stride,
                decimation,
            );
        }
    }
}

impl Clone for Dxd {
    fn clone(&self) -> Self {
        unsafe {
            let handle = NonNull::new(dsd2pcm_clone(self.handle.as_ptr()))
                .expect("Couldn't clone. Check inputs.");
            Self { handle }
        }
    }
}

impl Drop for Dxd {
    fn drop(&mut self) {
        unsafe {
            dsd2pcm_destroy(self.handle.as_ptr());
        }
    }
}