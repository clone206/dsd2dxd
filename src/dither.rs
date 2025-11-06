/*
 Copyright (c) 2023 clone206

 This file is part of dsd2dxd

 dsd2dxd is free software: you can redistribute it and/or modify it
 under the terms of the GNU General Public License as published by the
 Free Software Foundation, either version 3 of the License, or
 (at your option) any later version.

 dsd2dxd is distributed in the hope that it will be useful, but
 WITHOUT ANY WARRANTY; without even the implied warranty of
 MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE. See the
 GNU General Public License for more details.
 You should have received a copy of the GNU General Public License
 along with dsd2dxd. If not, see <https://www.gnu.org/licenses/>.
*/

use rand::Rng;
use std::env;

#[derive(Clone)]
pub struct Dither {
    fpd: u32, // Floating-point dither
    dither_type: char,
    neg_scale: f64, // Pre-dither scale
    pos_scale: f64, // Post-dither scale
}

impl Dither {
    pub fn new(dither_type: char) -> Result<Self, &'static str> {
        let dither_type = dither_type.to_ascii_lowercase();
        if !['t', 'f', 'x', 'r'].contains(&dither_type) {
            return Err("Invalid dither type!");
        }

        // Parse env var once at construction
        let (neg_scale, pos_scale) = match env::var("DSD2DXD_DITHERSCALE")
        {
            Ok(val) => val
                .parse::<f64>()
                .ok()
                .map(|db| {
                    (10.0f64.powf(-db / 20.0), 10.0f64.powf(db / 20.0))
                })
                .unwrap_or((1.0, 1.0)),
            Err(_) => (1.0, 1.0),
        };

        Ok(Self {
            fpd: 1,
            dither_type,
            neg_scale,
            pos_scale,
        })
    }

    pub fn init(&mut self) {
        if self.dither_type != 'x' {
            let _ = rand::thread_rng();
        }
        if self.dither_type == 'f' {
            self.init_rand();
        }
    }

    fn init_rand(&mut self) {
        let mut rng = rand::thread_rng();
        while self.fpd < 16386 {
            self.fpd = rng.r#gen::<u32>();
        }
    }

    fn process_tpdf(&mut self) -> f64 {
        // Triangular PDF dither with 1 LSB peak-to-peak amplitude (input already scaled so 1.0 = 1 LSB)
        let mut rng = rand::thread_rng();
        let r1: f64 = rng.gen_range(0.0..=0.5);
        let r2: f64 = rng.gen_range(0.0..=0.5);
        r1 - r2 // range [-0.5, 0.5], triangular distribution
    }

    pub fn process_samp(&mut self, sample: &mut f64) {
        *sample *= self.neg_scale;
        match self.dither_type {
            't' => *sample += self.process_tpdf(),
            'r' => *sample += self.process_rpdf(),
            'f' => self.fpdither(sample),
            _ => (),
        }
        *sample *= self.pos_scale;
    }

    fn process_rpdf(&mut self) -> f64 {
        let mut rng = rand::thread_rng();
        rng.r#gen::<f64>() - 0.5
    }

    fn fpdither(&mut self, sample: &mut f64) {
        let exponent = sample.abs().log2().floor() as i32;
        self.fpd ^= self.fpd << 13;
        self.fpd ^= self.fpd >> 17;
        self.fpd ^= self.fpd << 5;
        *sample +=
            (self.fpd as f64) * 3.4e-36 * (2.0f64).powi(exponent + 62);
        *sample = (*sample as f32) as f64;
    }

    // Add getter for dither type
    pub fn dither_type(&self) -> char {
        self.dither_type
    }
}
