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

mod color_logger;
mod model;

use clap::Parser;
use colored::Colorize;
use color_logger::ColorLogger;
use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use indicatif_log_bridge::LogWrapper;
use log::{info, warn};
use rdsd2pcm::{
    DitherType, Endianness, FilterType, FmtType, ONE_HUNDRED_PERCENT,
    OutputType,
};
use std::{error::Error, io, path::PathBuf, sync::mpsc, time::Instant};

use crate::model::TermResult;

#[derive(Parser)]
#[command(name = "dsd2dxd", version)]
struct Cli {
    /// Output directory path for converted PCM files. Directory
    /// must already exist but any subdirectories will be created as needed.
    /// Artwork files will be copied to the output directories.
    /// [default: same as input file]
    #[arg(short = 'p', long = "path", default_value = None)]
    path: Option<PathBuf>,

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

    /// Quiet mode: suppress all log output
    #[arg(short = 'q', long = "quiet")]
    quiet: bool,

    /// Append abbreviated output rate to filename
    /// (e.g., _96K, _88_2K). Also appends " [<OUTPUT_RATE>]" to the
    /// album tag of the output file if present.
    #[arg(short = 'a', long = "append")]
    append_rate: bool,

    /// Recurse into directories when the supplied input paths include folders
    #[arg(short = 'R', long = "recurse")]
    recurse: bool,

    /// Input files/folders (use - for stdin)
    #[arg(name = "FILES")]
    files: Vec<PathBuf>,
}

#[tokio::main]
async fn main() -> TermResult {
    match run().await {
        Ok(()) => TermResult(Ok(())),
        Err(e) => TermResult(Err(e.into())),
    }
}

async fn run() -> Result<(), Box<dyn Error>> {
    let cli = Cli::parse();
    let logger = ColorLogger::new(cli.quiet, cli.verbose);
    let multi = MultiProgress::new();
    LogWrapper::new(multi.clone(), logger).try_init()?;

    let dither = cli.dither_type.unwrap_or(if cli.bit_depth == 32 {
        'F'
    } else {
        'T'
    });

    let dither_type = match dither.to_ascii_lowercase() {
        't' => DitherType::TPDF,
        'r' => DitherType::Rectangular,
        'f' => DitherType::FPD,
        'x' => DitherType::None,
        _ => {
            return Err(
                "Invalid dither type; must be T, R, F, or X".into()
            );
        }
    };

    let format =
        match cli.format.to_ascii_lowercase() {
            'i' => FmtType::Interleaved,
            'p' => FmtType::Planar,
            _ => return Err(
                "Invalid format; must be I (interleaved) or P (planar)"
                    .into(),
            ),
        };

    let endian = match cli.endianness.to_ascii_lowercase() {
        'l' => Endianness::LsbFirst,
        'm' => Endianness::MsbFirst,
        _ => Endianness::MsbFirst,
    };

    let filt_type = match cli.filter_type.unwrap().to_ascii_uppercase() {
        'E' => FilterType::Equiripple,
        'X' => FilterType::XLD,
        'D' => FilterType::Dsd2Pcm,
        'C' => FilterType::Chebyshev,
        _ => FilterType::Equiripple,
    };

    let output = match cli.output.to_ascii_lowercase() {
        's' => OutputType::Stdout,
        'a' => OutputType::Aiff,
        'w' => OutputType::Wav,
        'f' => OutputType::Flac,
        _ => OutputType::Stdout,
    };

    let cwd = std::env::current_dir()
        .unwrap_or_else(|_| std::path::PathBuf::from("."));

    let mut inputs = if cli.files.is_empty() {
        vec![PathBuf::from("-")]
    } else {
        cli.files.clone()
    };

    inputs.sort();
    inputs.dedup();

    let mut total_inputs = 0;
    // Handle stdin conversion once, then remove it so we don't treat it as a file path.
    if inputs.contains(&PathBuf::from("-")) {
        do_conversion(
            None,
            &cli,
            output,
            dither_type,
            format,
            endian,
            filt_type,
            cwd.clone(),
            &multi,
        )
        .await?;
        total_inputs += 1;
        inputs.retain(|p| p != &PathBuf::from("-"));
    }

    // Filter to remove any glob patterns, yielding all inputted paths
    let paths = inputs
        .iter()
        .filter_map(|input| {
            if input.to_string_lossy().contains('*') {
                warn!(
                    "Unexpanded glob pattern detected in input: \"{}\". Skipping.",
                    input.display()
                );
                None
            } else {
                Some(input)
            }
        })
        .cloned()
        .collect::<Vec<_>>();

    let expanded_paths = rdsd2pcm::find_dsd_files(&paths, cli.recurse)?;
    total_inputs += expanded_paths.len();

    let wall_start = Instant::now();

    for path in expanded_paths {
        do_conversion(
            Some(path),
            &cli,
            output,
            dither_type,
            format,
            endian,
            filt_type,
            cwd.clone(),
            &multi,
        )
        .await?;
    }

    let total_elapsed = wall_start.elapsed();
    let total_secs = total_elapsed.as_secs();
    let h = total_secs / 3600;
    let m = (total_secs % 3600) / 60;
    let s = total_secs % 60;
    info!(
        "Processed {} inputs in {:02}:{:02}:{:02}",
        total_inputs, h, m, s
    );

    Ok(())
}

/// Run conversion for single input and report progress to stderr
async fn do_conversion(
    path: Option<PathBuf>,
    cli: &Cli,
    output: rdsd2pcm::OutputType,
    dither_type: rdsd2pcm::DitherType,
    format: rdsd2pcm::FmtType,
    endian: rdsd2pcm::Endianness,
    filt_type: rdsd2pcm::FilterType,
    cwd: PathBuf,
    multi: &MultiProgress,
) -> Result<(), Box<dyn Error>> {
    // Construct a fresh conversion context per input to avoid moving a shared `lib`.
    let mut lib = rdsd2pcm::Rdsd2Pcm::new(
        cli.bit_depth,
        output,
        cli.level,
        cli.output_rate,
        cli.path.clone(),
        dither_type,
        format,
        endian,
        cli.input_rate,
        cli.block_size.unwrap_or(4096),
        cli.channels.unwrap_or(2),
        filt_type,
        cli.append_rate,
        cwd,
        path,
    )?;
    let (sender, receiver) = mpsc::channel::<f32>();
    let file_name = lib.file_name();

    let pg = multi
        .add(ProgressBar::new(100))
        .with_style(ProgressStyle::with_template(
            "{prefix} {bar:20.cyan/blue} {percent}{msg}",
        )?)
        .with_prefix(format!("{} {}", "[Converting]".bold(), file_name.bold()))
        .with_message("%");

    // Spawn task for conversion; join after progress loop.
    let handle = tokio::spawn(async move {
        // Map Box<dyn Error> into String so JoinHandle carries a Send payload
        lib.do_conversion(Some(sender)).map_err(|e| e.to_string())
    });
    for progress in &receiver {
        pg.set_position(progress.floor() as u64);
        if progress == ONE_HUNDRED_PERCENT {
            break;
        }
    }
    drop(receiver); // Close the receiver

    // Propagate conversion errors
    if let Err(e) = handle.await? {
        return Err(Box::new(io::Error::new(
            io::ErrorKind::Other,
            format!("Conversion error: {}", e),
        )));
    }
    Ok(())
}
