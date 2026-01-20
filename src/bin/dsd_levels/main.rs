use std::error::Error;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::mpsc;
use std::thread::available_parallelism;
use std::time::Instant;
use std::io;

use clap::Parser;
use colored::Colorize;
use dsd2dxd::ColorLogger;
use dsd2dxd::TermResult;
use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use indicatif_log_bridge::LogWrapper;
use log::{info, warn};
use rayon::iter::{IntoParallelIterator, ParallelIterator};
use rdsd2pcm::{
    DitherType, DsdFileFormat, Endianness, FilterType, FmtType,
    FormatExtensions, ONE_HUNDRED_PERCENT, OutputType, ProgressUpdate,
    Rdsd2Pcm, find_dsd_files,
};

static CANCEL_FLAG: AtomicBool = AtomicBool::new(false);

#[derive(Parser, Debug)]
#[command(
    name = "dsd_levels",
    about = "Compute peak level (dBFS) of DSF/DFF after Butterworth lowpass",
    version
)]
struct Cli {
    /// One or more input DSD container files (.dsf or .dff)
    #[arg(required = true)]
    files: Vec<PathBuf>,

    /// Recurse into directories when the supplied input paths include folders
    #[arg(short = 'R', long = "recurse")]
    recurse: bool,

    /// Number of channels
    #[arg(short = 'c', long = "channels", default_value = "2")]
    channels: Option<usize>,

    /// DSD data format: Interleaved (I) or Planar (P)
    #[arg(short = 'f', long = "fmt", default_value = "I")]
    format: char,

    /// DSD data endianness: M (most significant bit first),
    /// or L (least significant bit first)
    #[arg(short = 'e', long = "endianness", default_value = "M")]
    endianness: char,

    /// DSD block size in bytes. Only set this if you know
    /// what you're doing.
    #[arg(short = 's', long = "bs", default_value = "4096")]
    block_size: Option<u32>,

    /// Output sample rate in Hz. Can be 88200, 96000,
    /// 176400, 192000, 352800, 384000. Note that conversion
    /// to multiples of 44100 are faster than 48000 multiples
    #[arg(short = 'r', long = "rate", default_value = "352800")]
    output_rate: u32,

    /// Input DSD rate: 1 (DSD64), 2 (DSD128), or 4 (DSD256)
    #[arg(short = 'i', long = "inrate", default_value = "1")]
    input_rate: u32,
}

fn main() -> TermResult {
    match run() {
        Ok(()) => TermResult(Ok(())),
        Err(e) => TermResult(Err(e.into())),
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();
    let logger = ColorLogger::new(false, false);
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
    }

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
    let wall_start = Instant::now();

    // Handle stdin conversion once, then remove it so we don't treat it as a file path.
    if inputs.contains(&PathBuf::from("-")) {
        check_stdin(&cli, format, endian, cwd.clone())?;
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

    let expanded_paths = find_dsd_files(&paths, cli.recurse)?;
    let num_paths = expanded_paths.len();
    total_inputs += num_paths;

    // Parallelize per input using Rayon; short-circuit on first error.
    expanded_paths
        .into_par_iter()
        .try_for_each(|path| {
            // Parse CLI in-thread to avoid sharing non-Send/Sync fields.
            let cli_local = Cli::parse();
            check_file(
                path,
                &cli_local,
                format,
                endian,
                cwd.clone(),
                &multi,
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
        "Analyzed {} inputs in {:02}:{:02}:{:02}",
        total_inputs, h, m, s
    );

    Ok(())
}

fn check_file(
    path: PathBuf,
    cli: &Cli,
    format: FmtType,
    endian: Endianness,
    cwd: PathBuf,
    multi: &MultiProgress,
) -> Result<(), String> {
    let file_name = if let Some(name) = path.file_name() {
        name.to_string_lossy().into_owned()
    } else {
        return Err(format!("Invalid file path: {}", path.display()));
    };
    let mut lib = if DsdFileFormat::from(&path).is_container() {
        Rdsd2Pcm::from_container(
            32,
            OutputType::Stdout,
            0.0,
            cli.output_rate,
            None,
            DitherType::None,
            FilterType::Equiripple,
            false,
            cwd,
            path,
        )
        .map_err(|e| e.to_string())?
    } else {
        Rdsd2Pcm::new(
            32,
            OutputType::Stdout,
            0.0,
            cli.output_rate,
            None,
            DitherType::None,
            format,
            endian,
            cli.input_rate.try_into()?,
            cli.block_size.unwrap_or(4096),
            cli.channels.unwrap_or(2),
            FilterType::Equiripple,
            false,
            cwd,
            Some(path),
        )
        .map_err(|e| e.to_string())?
    };

    let (sender, receiver) = mpsc::channel::<ProgressUpdate>();

    let style = ProgressStyle::with_template(
        "{prefix} {bar:20.cyan/blue} {percent}{msg}",
    )
    .map_err(|e| e.to_string())?;

    let pg = multi
        .add(ProgressBar::new(100))
        .with_style(style)
        .with_prefix(format!(
            "{} {}",
            "[Analyzing]".bold(),
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

    let peak_res = lib.check_level(&CANCEL_FLAG, Some(sender));

    if let Err(e) = progress_handle.join() {
        // Ensure progress thread exits and propagate errors.
        return Err(format!("Progress thread panicked: {:?}", e));
    }

    match peak_res {
        Ok(peak) => {
            info!("{}: peak level = {:.2} dBFS", file_name, peak);
            Ok(())
        }
        Err(e) => {
            return Err(format!("Error processing {}: {}", file_name, e));
        }
    }
}

fn check_stdin(
    cli: &Cli,
    format: FmtType,
    endian: Endianness,
    cwd: PathBuf,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut lib = Rdsd2Pcm::new(
        32,
        OutputType::Stdout,
        0.0,
        cli.output_rate,
        None,
        DitherType::None,
        format,
        endian,
        cli.input_rate.try_into()?,
        cli.block_size.unwrap_or(4096),
        cli.channels.unwrap_or(2),
        FilterType::Equiripple,
        false,
        cwd,
        None,
    )
    .map_err(|e| e.to_string())?;

    let peak = lib.check_level(&CANCEL_FLAG, None)?;

    info!("Stdin peak level = {:.2} dBFS", peak);

    Ok(())
}
