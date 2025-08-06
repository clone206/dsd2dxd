use std::ptr::NonNull;
use std::sync::atomic::{AtomicBool, Ordering};
use std::marker::PhantomData;
use libc::{c_char, c_int, ptrdiff_t, size_t};

#[repr(C)]
pub struct Dsd2PcmCtx {
    fifo: [u8; 65536],  // FIFOSIZE = 1<<16
    fifopos: u32,
    num_tables: u32,
    lsbfirst: i32,
    decimation: i32,
    delay: i32,
    delay2: i32,
    ctables: *mut *mut f64,
}

#[link(name = "dsd2pcm")]
extern "C" {
    fn dsd2pcm_init(filt_type: c_char, lsbf: c_int, decimation: c_int, dsd_rate: c_int) -> *mut Dsd2PcmCtx;
    fn dsd2pcm_destroy(ctx: *mut Dsd2PcmCtx);
    fn dsd2pcm_clone(ctx: *mut Dsd2PcmCtx) -> *mut Dsd2PcmCtx;
    fn dsd2pcm_reset(ctx: *mut Dsd2PcmCtx);
    fn dsd2pcm_translate(
        ctx: *mut Dsd2PcmCtx,
        block_size: size_t,
        dsd_data: *const u8,
        dsd_stride: ptrdiff_t,
        float_data: *mut f64,
        float_stride: ptrdiff_t,
        decimation: c_int
    );
}

pub struct Dxd {
    ctx: NonNull<Dsd2PcmCtx>,
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
        dsd_samples: usize,
        dsd_data: &[u8],
        dsd_stride: isize,
        pcm_data: &mut [f64],
        pcm_stride: isize,
        decimation: i32,
    ) -> Result<(), &'static str> {
        unsafe {
            dsd2pcm_translate(
                self.ctx.as_ptr(),
                dsd_samples,
                dsd_data.as_ptr(),
                dsd_stride,
                pcm_data.as_mut_ptr(),
                pcm_stride,
                decimation,
            );
        }
        Ok(())
    }
}