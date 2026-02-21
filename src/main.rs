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
use colored::Colorize;
use common_path::common_path_all;
use dsd2dxd::ColorLogger;
use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use indicatif_log_bridge::LogWrapper;
use log::{info, trace, warn};
use rayon::prelude::*;
use rdsd2pcm::{
    DitherType, DsdFileFormat, Endianness, FilterType, FmtType,
    FormatExtensions, ONE_HUNDRED_PERCENT, OutputType, ProgressUpdate,
    Rdsd2Pcm, find_dsd_files,
};
use std::path::Path;
use std::sync::atomic::AtomicBool;
use std::thread::available_parallelism;
use std::{error::Error, io, path::PathBuf, sync::mpsc, time::Instant};

use dsd2dxd::TermResult;
static CANCEL_FLAG: AtomicBool = AtomicBool::new(false);

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
    channels: Option<usize>,

    /// DSD data format: Interleaved (I) or Planar (P)
    #[arg(short = 'f', long = "fmt", default_value = "I")]
    format: char,

    /// Output bit depth: 16, 20, 24 (fixed integer), or 32 (float)
    #[arg(short = 'b', long = "bitdepth", default_value = "24")]
    bit_depth: usize,

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
    /// 176400, 192000, 352800, 384000 for all input DSD rates.
    /// For DSD128 inputs, 705600 is also available. For DSD256 inputs,
    /// both 705600 and 1411200 are available. Note that conversion
    /// to multiples of 44100 are faster than 48000 multiples.
    /// For DSD512 inputs, only 352800 is available.
    #[arg(short = 'r', long = "rate", default_value = "352800")]
    output_rate: u32,

    /// Input DSD rate: 1 (DSD64), 2 (DSD128), 4 (DSD256), or 8 (DSD512)
    #[arg(short = 'i', long = "inrate", default_value = "1")]
    input_rate: u32,

    /// Output type: S (stdout), A (aif), C (aifc), W (wave), F (flac)
    /// Note that W, A, C, or F outputs to either
    /// <basename>.[wav|aif|aifc|flac] in current directory,
    /// where <basename> is the input filename
    /// without the extension, or output.[wav|aif|aifc|flac]
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

fn main() -> TermResult {
    match run() {
        Ok(()) => TermResult(Ok(())),
        Err(e) => TermResult(Err(e.into())),
    }
}

fn run() -> Result<(), Box<dyn Error>> {
    let cli = Cli::parse();
    let logger = ColorLogger::new(cli.quiet, cli.verbose);
    let multi = MultiProgress::new();
    LogWrapper::new(multi.clone(), logger).try_init()?;

    let avail_par = available_parallelism().map(|n| n.get()).unwrap_or(1);
    let thread_count = (avail_par / 2).max(1);

    // Configure Rayon pool size to our computed thread_count.
    // build_global can only be called once; ignore error if already set.
    if let Err(e) = rayon::ThreadPoolBuilder::new()
        .num_threads(thread_count)
        .build_global()
    {
        warn!(
            "Rayon pool initialization error ({} threads). Details: {:?}",
            thread_count, e
        );
    } else {
        trace!("Configured Rayon pool with {} threads", thread_count);
    }

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
        'c' => OutputType::Aifc,
        'w' => OutputType::Wav,
        'f' => OutputType::Flac,
        _ => OutputType::Stdout,
    };

    let mut inputs = if cli.files.is_empty() {
        vec![PathBuf::from("-")]
    } else {
        cli.files.clone()
    };

    inputs.sort();
    inputs.dedup();

    let mut total_inputs = 0;
    let wall_start = Instant::now();

    // Handle stdin conversion once, then remove it so we don't treat it as a file path.
    if inputs.contains(&PathBuf::from("-")) {
        convert_stdin(
            &cli,
            output,
            dither_type,
            format,
            endian,
            filt_type,
        )?;
        total_inputs += 1;
        inputs.retain(|p| p != &PathBuf::from("-"));
    }

    // Filter to remove any glob patterns, yielding all inputted paths, canonicalized
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
        .map(|p| {
            let full_path = p.canonicalize()?;
            Ok(full_path)
        })
        .collect::<Result<Vec<_>, std::io::Error>>()?;

    // Determine base directory against which output paths should be constructed.
    // Should only come into play when an output folder path is specified.
    let base_dir = if paths.len() == 1 {
        // Just one file/folder; use its parent directory.
        paths[0].parent().unwrap_or(Path::new("/")).to_path_buf()
    } else {
        // For multiple files, find lowest common ancestor directory.
        let common = common_path_all(paths.iter()
            .map(|p| p.as_path()))
            .unwrap_or(PathBuf::from("/"));
        common.parent().unwrap_or(Path::new("/")).to_path_buf()
    };

    let expanded_paths = find_dsd_files(&paths, cli.recurse)?;
    let num_paths = expanded_paths.len();
    total_inputs += num_paths;

    // Parallelize per input using Rayon; short-circuit on first error.
    expanded_paths
        .into_par_iter()
        .try_for_each(|path| {
            // Parse CLI in-thread to avoid sharing non-Send/Sync fields.
            let cli_local = Cli::parse();
            convert_file(
                path,
                &cli_local,
                output,
                dither_type,
                format,
                endian,
                filt_type,
                base_dir.clone(),
                &multi,
                output != OutputType::Stdout,
            )
        })
        .map_err(|e| -> Box<dyn Error> {
            Box::new(io::Error::new(io::ErrorKind::Other, e))
        })?;

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

/// Run conversion for stdin. Single threaded.
fn convert_stdin(
    cli: &Cli,
    output: OutputType,
    dither_type: DitherType,
    format: FmtType,
    endian: Endianness,
    filt_type: FilterType,
) -> Result<(), Box<dyn Error>> {
    // Construct a fresh conversion context per input to avoid moving a shared `lib`.
    let mut lib = Rdsd2Pcm::new(
        cli.bit_depth,
        output,
        cli.level,
        cli.output_rate,
        cli.path.clone(),
        dither_type,
        format,
        endian,
        cli.input_rate.try_into()?,
        cli.block_size.unwrap_or(4096),
        cli.channels.unwrap_or(2),
        filt_type,
        cli.append_rate,
        std::env::current_dir()
            .unwrap_or_else(|_| std::path::PathBuf::from(".")),
        None,
    )
    .map_err(|e| e.to_string())?;

    lib.do_conversion(&CANCEL_FLAG, None)
}

/// Run conversion for single input and report progress to stderr if requested.
fn convert_file(
    path: PathBuf,
    cli: &Cli,
    output: OutputType,
    dither_type: DitherType,
    format: FmtType,
    endian: Endianness,
    filt_type: FilterType,
    cwd: PathBuf,
    multi: &MultiProgress,
    show_progress: bool,
) -> Result<(), String> {
    let mut lib = if DsdFileFormat::from(&path).is_container() {
        Rdsd2Pcm::from_container(
            cli.bit_depth,
            output,
            cli.level,
            cli.output_rate,
            cli.path.clone(),
            dither_type,
            filt_type,
            cli.append_rate,
            cwd,
            path,
        )
        .map_err(|e| e.to_string())?
    } else {
        Rdsd2Pcm::new(
            cli.bit_depth,
            output,
            cli.level,
            cli.output_rate,
            cli.path.clone(),
            dither_type,
            format,
            endian,
            cli.input_rate.try_into()?,
            cli.block_size.unwrap_or(4096),
            cli.channels.unwrap_or(2),
            filt_type,
            cli.append_rate,
            cwd,
            Some(path),
        )
        .map_err(|e| e.to_string())?
    };

    let (progress_handle, sender) = if show_progress {
        let (sender, receiver) = mpsc::channel::<ProgressUpdate>();
        let file_name = lib.file_name();
        let style = ProgressStyle::with_template(
            "{prefix} {bar:20.cyan/blue} {percent}{msg}",
        )
        .map_err(|e| e.to_string())?;

        let pg = multi
            .add(ProgressBar::new(100))
            .with_style(style)
            .with_prefix(format!(
                "{} {}",
                "[Converting]".bold(),
                file_name.bold()
            ))
            .with_message("%");

        // Run conversion on this Rayon worker; drive progress on a lightweight OS thread.
        let progress_handle = std::thread::spawn(move || {
            while let Ok(progress) = receiver.recv() {
                pg.set_position(progress.percent.floor() as u64);
                if progress.percent == ONE_HUNDRED_PERCENT {
                    break;
                }
            }
        });
        (Some(progress_handle), Some(sender))
    } else {
        (None, None)
    };

    // Perform the blocking conversion here (inside Rayon worker).
    let conv_res = lib.do_conversion(&CANCEL_FLAG, sender);

    if let Some(progress_handle) = progress_handle
        && let Err(e) = progress_handle.join()
    {
        // Ensure progress thread exits and propagate errors.
        return Err(format!("Progress thread panicked: {:?}", e));
    }

    conv_res.map_err(|e| format!("Conversion error: {}", e))
}
