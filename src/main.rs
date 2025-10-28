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

use clap::Parser;
use std::error::Error;
mod audio_file;
mod byte_precalc_decimator;
mod conversion_context;
mod dither;
mod dsd;
mod filters;
mod filters_lm;
mod input;
mod lm_resampler;
mod output;

pub use conversion_context::ConversionContext;
pub use dither::Dither;
pub use input::InputContext;
pub use output::OutputContext;

#[derive(Parser)]
#[command(name = "dsd2dxd")]
struct Cli {
    /// Output directory path for converted PCM files. Directory
    /// must already exist but any subdirectories will be created as needed.
    /// [default: same as input file]
    #[arg(short = 'p', long = "path", default_value = None)]
    path: Option<String>,

    /// Number of channels
    #[arg(short = 'c', long = "channels", default_value = "2")]
    channels: Option<u32>,

    /// DSD data format: Interleaved (I) or Planar (P)
    #[arg(short = 'f', long = "fmt", default_value = "I")]
    format: char,

    /// Output bit depth: 16, 20, 24 (fixed integer), or 32 (float)
    #[arg(short = 'b', long = "bitdepth", default_value = "24")]
    bit_depth: i32,

    /// Filter type: E (Equiripple), X (XLD. Only available with
    /// DSD64 input and 88200, 176400, and 352800 outputs),
    /// D (Original dsd2pcm. Only available with DSD64 input and
    /// 352800 output), C (Chebyshev. Only available with
    /// DSD128 input and 88200, 176400, and 352800 outputs)
    #[arg(short = 't', long = "filttype", default_value = "E")]
    filter_type: Option<char>,

    /// DSD data endianness: M (most significant bit first),
    /// or L (least significant bit first)
    #[arg(short = 'e', long = "endianness", default_value = "M")]
    endianness: char,

    /// DSD block size in bytes. Only set this if you know
    /// what you're doing.
    #[arg(short = 's', long = "bs", default_value = "4096")]
    block_size: Option<u32>,

    /// Dither type: T (TPDF), R (rectangular),
    /// F (float), X (none) [default: F for 32 bit, T otherwise]
    #[arg(short = 'd', long = "dither")]
    dither_type: Option<char>,

    /// Output sample rate in Hz. Can be 88200, 96000,
    /// 176400, 192000, 352800, 384000. Note that conversion
    /// to multiples of 44100 are faster than 48000 multiples
    #[arg(short = 'r', long = "rate", default_value = "352800")]
    output_rate: i32,

    /// Input DSD rate: 1 (DSD64), 2 (DSD128), or 4 (DSD256)
    #[arg(short = 'i', long = "inrate", default_value = "1")]
    input_rate: i32,

    /// Output type: S (stdout), A (aif), W (wave), F (flac)
    /// Note that W, A, or F outputs to either
    /// <basename>.[wav|aif|flac] in current directory,
    /// where <basename> is the input filename
    /// without the extension, or output.[wav|aif|flac]
    /// (if reading from stdin.)
    #[arg(short = 'o', long = "output", default_value = "S")]
    output: char,

    /// Volume level adjustment in dB. Can be negative with the
    /// long form, e.g. --level=-3
    #[arg(short = 'l', long = "level", default_value = "0.0")]
    level: f64,

    /// Print diagnostic messages
    #[arg(short = 'v', long = "verbose")]
    verbose: bool,

    /// Append abbreviated output rate to filename
    /// (e.g., _96K, _88_2K). Also appends " [<OUTPUT_RATE>]" to the
    /// album tag of the output file if present.
    #[arg(short = 'a', long = "append")]
    append_rate: bool,

    /// Input files (use - for stdin)
    #[arg(name = "FILES")]
    files: Vec<String>,
}

fn main() -> Result<(), Box<dyn Error>> {
    let cli = Cli::parse();

    let verbose = |message: &str, new_line: bool| {
        if cli.verbose {
            if new_line {
                eprintln!("{}", message);
            } else {
                eprint!("{}", message);
            }
        }
    };

    let inputs = if cli.files.is_empty() {
        vec!["-".to_string()]
    } else {
        cli.files.clone()
    };

    let dither_type = cli
        .dither_type
        .unwrap_or(if cli.bit_depth == 32 { 'F' } else { 'T' });

    let out_ctx = OutputContext::new(cli.bit_depth, cli.output, cli.level, cli.output_rate, cli.path)?;

    let dither = Dither::new(dither_type)?;

    for input in inputs {
        if input.contains('*') {
            verbose(
                &format!(
                    "Warning: Unexpanded glob pattern detected in input: \"{}\". Skipping.",
                    input
                ),
                true,
            );
            continue;
        }

        verbose(&format!("Input: {}", input), true);

        let in_ctx = InputContext::new(
            input.clone(),
            cli.format,
            cli.endianness,
            cli.input_rate,
            cli.block_size.unwrap_or(4096),
            cli.channels.unwrap_or(2),
            cli.verbose,
        )?;

        let mut conv_ctx = ConversionContext::new(
            in_ctx,
            out_ctx.clone(),
            dither.clone(),
            cli.filter_type.unwrap().to_ascii_uppercase(),
            cli.verbose,
            cli.append_rate,
        )?;
        conv_ctx.do_conversion()?;
    }

    Ok(())
}
