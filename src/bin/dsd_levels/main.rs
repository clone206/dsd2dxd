use std::path::{Path, PathBuf};
use std::time::Instant;

use clap::Parser;
use rdsd2pcm::DsdReader;

const DEFAULT_FILTER_ORDER: usize = 12;
const FILTER_CUTOFF_HZ: f64 = 21_000.0;
const DSD64_FS_HZ: f64 = 2_822_400.0;

#[derive(Parser, Debug)]
#[command(
	name = "dsd_levels",
	about = "Compute peak level (dBFS) of DSF/DFF after Butterworth lowpass",
	version
)]
struct Args {
	/// One or more input DSD container files (.dsf or .dff)
	#[arg(required = true)]
	paths: Vec<PathBuf>,

	/// Print per-channel peaks in addition to overall peak
	#[arg(long, default_value_t = false)]
	per_channel: bool,

	/// Butterworth lowpass order (even only). Lower is faster but less selective.
	#[arg(long, default_value_t = DEFAULT_FILTER_ORDER)]
	order: usize,
}

fn is_dsd_container_path(path: &Path) -> bool {
	let Some(ext) = path.extension().and_then(|s| s.to_str()) else {
		return false;
	};
	matches!(ext.to_ascii_lowercase().as_str(), "dsf" | "dff")
}

fn dsd_sampling_frequency_hz(dsd_reader: &DsdReader) -> f64 {
	let multiplier = dsd_reader.dsd_rate() as f64;
	DSD64_FS_HZ * multiplier
}

#[derive(Clone, Copy, Debug)]
struct Complex {
	re: f64,
	im: f64,
}

impl Complex {
	fn conj(self) -> Self {
		Self {
			re: self.re,
			im: -self.im,
		}
	}
}

use std::ops::{Add, Div, Mul, Sub};

impl Add for Complex {
	type Output = Complex;
	fn add(self, rhs: Complex) -> Complex {
		Complex {
			re: self.re + rhs.re,
			im: self.im + rhs.im,
		}
	}
}

impl Sub for Complex {
	type Output = Complex;
	fn sub(self, rhs: Complex) -> Complex {
		Complex {
			re: self.re - rhs.re,
			im: self.im - rhs.im,
		}
	}
}

impl Mul for Complex {
	type Output = Complex;
	fn mul(self, rhs: Complex) -> Complex {
		Complex {
			re: self.re * rhs.re - self.im * rhs.im,
			im: self.re * rhs.im + self.im * rhs.re,
		}
	}
}

impl Div for Complex {
	type Output = Complex;
	fn div(self, rhs: Complex) -> Complex {
		let denom = rhs.re * rhs.re + rhs.im * rhs.im;
		Complex {
			re: (self.re * rhs.re + self.im * rhs.im) / denom,
			im: (self.im * rhs.re - self.re * rhs.im) / denom,
		}
	}
}

#[derive(Clone, Copy, Debug)]
struct BiquadCoeffs {
	b0: f32,
	b1: f32,
	b2: f32,
	a1: f32,
	a2: f32,
}

#[derive(Clone, Copy, Debug)]
struct BiquadState {
	coeffs: BiquadCoeffs,
	z1: f32,
	z2: f32,
}

impl BiquadState {
	fn new(coeffs: BiquadCoeffs) -> Self {
		Self { coeffs, z1: 0.0, z2: 0.0 }
	}

	#[inline(always)]
	fn process(&mut self, x: f32) -> f32 {
		// Transposed direct-form II
		let y = self.coeffs.b0 * x + self.z1;
		let z1 = self.coeffs.b1 * x - self.coeffs.a1 * y + self.z2;
		let z2 = self.coeffs.b2 * x - self.coeffs.a2 * y;
		self.z1 = z1;
		self.z2 = z2;
		y
	}
}

fn design_butterworth_lowpass_sos(
	order: usize,
	fs_hz: f64,
	cutoff_hz: f64,
) -> Result<Vec<BiquadCoeffs>, Box<dyn std::error::Error>> {
	if order == 0 || order % 2 != 0 {
		return Err("Only even Butterworth orders are supported".into());
	}
	if !(fs_hz.is_finite() && fs_hz > 0.0) {
		return Err("fs_hz must be finite and > 0".into());
	}
	if !(cutoff_hz.is_finite() && cutoff_hz > 0.0) {
		return Err("cutoff_hz must be finite and > 0".into());
	}
	if cutoff_hz >= fs_hz / 2.0 {
		return Err("cutoff_hz must be < Nyquist".into());
	}

	// Prewarp analog cutoff (rad/s) for bilinear transform.
	let omega_c = 2.0 * fs_hz * (std::f64::consts::PI * cutoff_hz / fs_hz).tan();
	let k = 2.0 * fs_hz;

	let mut sections = Vec::with_capacity(order / 2);
	for i in 0..(order / 2) {
		let theta = std::f64::consts::PI * ((2.0 * (i as f64) + 1.0) / (2.0 * (order as f64)) + 0.5);
		let pole_s = Complex {
			re: omega_c * theta.cos(),
			im: omega_c * theta.sin(),
		};
		// Bilinear transform: z = (k + s) / (k - s)
		let pole_z = (Complex { re: k, im: 0.0 } + pole_s) / (Complex { re: k, im: 0.0 } - pole_s);
		let pole_z_conj = pole_z.conj();

		let sum = pole_z + pole_z_conj; // 2*re
		let prod = pole_z * pole_z_conj; // |z|^2

		// Denominator: (1 - p1 z^-1)(1 - p2 z^-1) = 1 - (p1+p2) z^-1 + (p1 p2) z^-2
		let a1 = -sum.re;
		let a2 = prod.re;

		// Numerator zeros at z=-1 (double): (1 + z^-1)^2 = 1 + 2 z^-1 + z^-2
		// Choose gain so that DC gain is 1: H(1) = (b0+b1+b2) / (1+a1+a2) = 1
		let denom_dc = 1.0 + a1 + a2;
		let b0 = denom_dc / 4.0;
		let b1 = 2.0 * b0;
		let b2 = b0;

		sections.push(BiquadCoeffs {
			b0: b0 as f32,
			b1: b1 as f32,
			b2: b2 as f32,
			a1: a1 as f32,
			a2: a2 as f32,
		});
	}

	Ok(sections)
}

#[derive(Debug)]
struct ChannelFilter {
	sections: Vec<BiquadState>,
}

impl ChannelFilter {
	fn new(section_coeffs: &[BiquadCoeffs]) -> Self {
		Self {
			sections: section_coeffs.iter().copied().map(BiquadState::new).collect(),
		}
	}

	#[inline(always)]
	fn process(&mut self, x: f32) -> f32 {
		let mut y = x;
		for s in &mut self.sections {
			y = s.process(y);
		}
		y
	}
}

fn process_file(
	path: &Path,
	per_channel: bool,
	filter_order: usize,
) -> Result<(), Box<dyn std::error::Error>> {
	if !is_dsd_container_path(path) {
		return Err(format!(
			"Unsupported input (expected .dsf/.dff): {}",
			path.display()
		)
		.into());
	}

	let dsd_reader = DsdReader::from_container(path.to_path_buf())?;
	let fs_hz_dsd = dsd_sampling_frequency_hz(&dsd_reader);
	// We expand packed bytes into a per-bit bipolar stream, so the effective sample rate equals
	// the DSD bit-rate.
	let fs_hz = fs_hz_dsd;
	if FILTER_CUTOFF_HZ <= 0.0 {
		return Err("FILTER_CUTOFF_HZ must be > 0".into());
	}
	if filter_order == 0 {
		return Err("filter order must be > 0".into());
	}
	if filter_order % 2 != 0 {
		return Err("filter order must be even".into());
	}
	if FILTER_CUTOFF_HZ >= fs_hz / 2.0 {
		return Err(format!(
			"FILTER_CUTOFF_HZ must be < Nyquist ({})",
			fs_hz / 2.0
		)
		.into());
	}

	if FILTER_CUTOFF_HZ >= fs_hz / 2.0 {
		return Err(format!(
			"FILTER_CUTOFF_HZ must be < Nyquist ({})",
			fs_hz / 2.0
		)
		.into());
	}

	let sos = design_butterworth_lowpass_sos(filter_order, fs_hz, FILTER_CUTOFF_HZ)?;
	let channels = dsd_reader.channels_num();
	let mut chan_filters: Vec<ChannelFilter> = (0..channels).map(|_| ChannelFilter::new(&sos)).collect();
	let mut chan_peaks: Vec<f64> = vec![0.0; channels];
	let mut dsd_iter = dsd_reader.dsd_iter()?;
	let wall_start = Instant::now();
	let mut bits_processed_per_chan: u64 = 0;

	for (read_size, chan_bufs) in &mut dsd_iter {
		if channels == 0 {
			break;
		}
		let bytes_per_chan = read_size / channels;
		let min_chan_buf_len = chan_bufs
			.iter()
			.take(channels)
			.map(|b| b.len())
			.min()
			.unwrap_or(0);
		let valid_bytes = bytes_per_chan.min(min_chan_buf_len);
		bits_processed_per_chan = bits_processed_per_chan.saturating_add((valid_bytes as u64) * 8);
		for chan in 0..channels {
			let chan_buf = &chan_bufs[chan];
			for &b in &chan_buf[..valid_bytes] {
				// dsd-reader yields bytes in LSB-first time order.
				let mut mask = 1u8;
				for _ in 0..8 {
					let x: f32 = if (b & mask) != 0 { 1.0 } else { -1.0 };
					let y = chan_filters[chan].process(x);
					if !y.is_finite() {
						return Err("Non-finite filtered samples (overflow/instability)".into());
					}
					let ay = (y.abs()) as f64;
					if ay > chan_peaks[chan] {
						chan_peaks[chan] = ay;
					}
					mask <<= 1;
				}
			}
		}
	}

	let elapsed_s = wall_start.elapsed().as_secs_f64();
	let audio_s = if fs_hz > 0.0 {
		(bits_processed_per_chan as f64) / fs_hz
	} else {
		0.0
	};
	let x_realtime = if elapsed_s > 0.0 {
		audio_s / elapsed_s
	} else {
		f64::INFINITY
	};

	let mut per_chan_peaks_dbfs = Vec::with_capacity(channels);
	let mut overall_peak_dbfs = f64::NEG_INFINITY;
	for &p in &chan_peaks {
		let db = if p == 0.0 { f64::NEG_INFINITY } else { 20.0 * p.log10() };
		per_chan_peaks_dbfs.push(db);
		if db > overall_peak_dbfs {
			overall_peak_dbfs = db;
		}
	}

	println!(
		"{}: peak={:.2} dBFS (order={}, fc={} Hz, fs={} Hz, elapsed={:.3}s, xRT={:.2})",
		path.display(),
		overall_peak_dbfs,
		filter_order,
		FILTER_CUTOFF_HZ,
		fs_hz,
		elapsed_s,
		x_realtime
	);
	if per_channel {
		for (i, db) in per_chan_peaks_dbfs.iter().enumerate() {
			println!("  ch{}: {:.2} dBFS", i + 1, db);
		}
	}

	Ok(())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
	let args = Args::parse();
	for path in &args.paths {
		if let Err(e) = process_file(path, args.per_channel, args.order) {
			eprintln!("{}: error: {}", path.display(), e);
		}
	}
	Ok(())
}