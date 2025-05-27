use rand::Rng;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Clone)]
pub struct Dither {
    noise_shaping_l: f64,  // Noise shape state L
    noise_shaping_r: f64,  // Noise shape state R
    byn_l: [f64; 13],      // Benford Real number weights L
    byn_r: [f64; 13],      // Benford Real number weights R
    fpd: u32,              // Floating-point dither
    dither_type: char,
}

impl Dither {
    pub fn new(dither_type: char) -> Result<Self, &'static str> {
        let dither_type = dither_type.to_ascii_lowercase();
        if !['n', 't', 'f', 'x', 'r'].contains(&dither_type) {
            return Err("Invalid dither type!");
        }

        Ok(Self {
            noise_shaping_l: 0.0,
            noise_shaping_r: 0.0,
            byn_l: [0.0; 13],
            byn_r: [0.0; 13],
            fpd: 1,
            dither_type,
        })
    }

    pub fn init(&mut self) {
        if self.dither_type != 'x' {
            let _ = rand::thread_rng();
        }

        match self.dither_type {
            'n' => self.init_outputs(),
            'f' => self.init_rand(),
            _ => {}
        }
    }

    fn init_outputs(&mut self) {
        // Weights based on Benford's law. Smaller leading digits more likely.
        let weights = [1000.0, 301.0, 176.0, 125.0, 97.0, 79.0, 67.0, 58.0, 51.0, 46.0, 1000.0];
        self.byn_l[..11].copy_from_slice(&weights);
        self.byn_r[..11].copy_from_slice(&weights);
    }

    fn init_rand(&mut self) {
        let mut rng = rand::thread_rng();
        while self.fpd < 16386 {
            self.fpd = rng.gen::<u32>();
        }
    }

    pub fn process_samp(&mut self, sample: &mut f64, chan_num: i32) {
        match self.dither_type {
            'n' => self.njad(sample, chan_num),
            't' => self.tpdf(sample),
            'f' => self.fpdither(sample),
            'r' => self.rect(sample),
            _ => {}
        }
    }

    fn fpdither(&mut self, sample: &mut f64) {
        let exponent = sample.abs().log2().floor() as i32;
        self.fpd ^= self.fpd << 13;
        self.fpd ^= self.fpd >> 17;
        self.fpd ^= self.fpd << 5;
        *sample += (self.fpd as f64) * 3.4e-36 * (2.0f64).powi(exponent + 62);
        *sample = (*sample as f32) as f64;
    }

    fn njad(&mut self, sample: &mut f64, chan_num: i32) {
        let (noise_shaping, byn) = match chan_num {
            0 => (&mut self.noise_shaping_l, &mut self.byn_l),
            1 => (&mut self.noise_shaping_r, &mut self.byn_r),
            _ => panic!("njad only supports a maximum of 2 channels!"),
        };

        let mut cut_bins = false;
        let dry_sample = *sample;

        // Subtract error from previous iteration
        *sample -= *noise_shaping;

        // Isolate leading digit of number
        let mut benfordize = sample.floor();
        while benfordize >= 1.0 {
            benfordize /= 10.0;
        }
        while benfordize < 1.0 && benfordize > 0.0000001 {
            benfordize *= 10.0;
        }

        // Hotbin A becomes the Benford bin value for this number floored
        let hot_bin_a = benfordize.floor() as usize;

        let mut total_a = 0.0;
        // produce total number- smaller of total_a & total_b is closer to Benford real
        if hot_bin_a > 0 && hot_bin_a < 10 {
            // Temp add weight to this leading digit
            byn[hot_bin_a] += 1.0;

            // Coeffs get permanently incremented later in the loop
            if byn[hot_bin_a] > 982.0 {
                cut_bins = true;
            }

            total_a += 301.0 - byn[1];
            total_a += 176.0 - byn[2];
            total_a += 125.0 - byn[3];
            total_a += 97.0 - byn[4];
            total_a += 79.0 - byn[5];
            total_a += 67.0 - byn[6];
            total_a += 58.0 - byn[7];
            total_a += 51.0 - byn[8];
            total_a += 46.0 - byn[9];

            // Remove temp weight from this leading digit
            byn[hot_bin_a] -= 1.0;
        }

        // Isolate leading digit of number
        benfordize = sample.ceil();
        while benfordize >= 1.0 {
            benfordize /= 10.0;
        }
        while benfordize < 1.0 && benfordize > 0.0000001 {
            benfordize *= 10.0;
        }

        // Hotbin B becomes the Benford bin value for this number ceiled
        let hot_bin_b = benfordize.floor() as usize;

        let mut total_b = 0.0;
        if hot_bin_b > 0 && hot_bin_b < 10 {
            // Temp add weight to this leading digit
            byn[hot_bin_b] += 1.0;

            if byn[hot_bin_b] > 982.0 {
                cut_bins = true;
            }

            total_b += 301.0 - byn[1];
            total_b += 176.0 - byn[2];
            total_b += 125.0 - byn[3];
            total_b += 97.0 - byn[4];
            total_b += 79.0 - byn[5];
            total_b += 67.0 - byn[6];
            total_b += 58.0 - byn[7];
            total_b += 51.0 - byn[8];
            total_b += 46.0 - byn[9];

            // Remove temp weight from this leading digit
            byn[hot_bin_b] -= 1.0;
        }

        // Assign the relevant one to the delay line
        let output_sample = if total_a < total_b {
            byn[hot_bin_a] += 1.0;
            sample.floor()
        } else {
            byn[hot_bin_b] += 1.0;
            sample.floor() + 1.0
        };

        if cut_bins {
            // Scale down coeffs (weights based on Benford's Law)
            for i in 1..=10 {
                byn[i] *= 0.99;
            }
        }

        // Store the error
        *noise_shaping += output_sample - dry_sample;

        // Error shouldn't be greater than input sample value
        let abs_sample = sample.abs();
        *noise_shaping = noise_shaping.clamp(-abs_sample, abs_sample);
    }

    fn tpdf(&self, sample: &mut f64) {
        let mut rng = rand::thread_rng();
        let rand1: f64 = rng.gen::<f64>() / 2.0;  // rand value between 0 and 0.5
        let rand2: f64 = rng.gen::<f64>() / 2.0;  // rand value between 0 and 0.5
        *sample += rand1 - rand2;  // Range from -0.5 to +0.5
    }

    fn rect(&self, sample: &mut f64) {
        let mut rng = rand::thread_rng();
        let rand1: f64 = rng.gen::<f64>();  // rand value between 0 and 1
        *sample += rand1 - 0.5;  // Range from -0.5 to +0.5
    }
}