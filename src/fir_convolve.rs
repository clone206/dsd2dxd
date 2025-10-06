// Optimized symmetric FIR convolver (scalar only – SIMD paths removed).

pub struct FirConvolve {
    n: usize,
    full_taps: Vec<f64>,   // symmetric full impulse (needed for event engine)
    imp_acc: Vec<f64>,     // output accumulator ring
    imp_pos: usize,
}

// Fast lookup converting i8 sample to f64.
static I8_TO_F64: [f64; 256] = {
    let mut tbl = [0.0f64; 256];
    let mut i = 0;
    while i < 256 {
        tbl[i] = (i as i8) as f64;
        i += 1;
    }
    tbl
};

impl FirConvolve {
    pub fn new(second_half_taps: &[f64]) -> Self {
        let half = second_half_taps.len();
        let n = half * 2;
        assert!(half > 0);

        let mut left_half = second_half_taps.to_vec();
        left_half.reverse();

        let mut left = second_half_taps.to_vec();
        left.reverse();
        let full_taps: Vec<f64> = left.iter().chain(second_half_taps.iter()).cloned().collect();

        Self {
            n,
            full_taps,
            imp_acc: vec![0.0; n],
            imp_pos: 0,
        }
    }

    #[inline]
    pub fn process_sample_i8(&mut self, x: i8) -> f64 {
        let xv = I8_TO_F64[x as u8 as usize];
        if xv != 0.0 {
            self.inject_impulse_scalar(xv);
        }
        let y = self.imp_acc[self.imp_pos];
        self.imp_acc[self.imp_pos] = 0.0;
        self.imp_pos += 1;
        if self.imp_pos == self.n {
            self.imp_pos = 0;
        }
        y
    }

    #[inline]
    fn inject_impulse_scalar(&mut self, scale: f64) {
        let n = self.n;
        let pos = self.imp_pos;
        let first = n - pos;
        let taps = &self.full_taps;
        let mut i = 0usize;
        while i < first {
            self.imp_acc[pos + i] += scale * taps[i];
            i += 1;
        }
        while i < n {
            self.imp_acc[i - first] += scale * taps[i];
            i += 1;
        }
    }
}
