use clap::{Parser, ValueEnum};
use std::path::PathBuf;
use std::error::Error;

mod audio_file;
mod dsdin_sys;
mod dsd2pcm;
mod dsd;
mod input;
mod output;
mod dither;
mod conversion_context;
mod dsd2pcm_sys;

pub use conversion_context::ConversionContext;
pub use input::InputContext;
pub use output::OutputContext;
pub use dither::Dither;
pub use dsd2pcm::Dxd;

static mut VERBOSE_MODE: bool = false;

#[derive(Parser)]
#[command(name = "dsd2dxd")]
#[command(about = "DSD to PCM filter. Reads from stdin or file and writes to stdout or file.")]
struct Cli {
    /// Number of channels
    #[arg(short = 'c', long = "channels", default_value = "2")]
    channels: i32,

    /// Format: Interleaved (I) or Planar (P)
    #[arg(short = 'f', long = "fmt", default_value = "I")]
    format: char,

    /// Bit depth: 16, 20, 24, or 32 (float)
    #[arg(short = 'b', long = "bitdepth", default_value = "24")]
    bit_depth: i32,

    /// Filter type: X (XLD), D (Original), E (Equiripple), C (Chebyshev)
    #[arg(short = 't', long = "filttype")]
    filter_type: Option<char>,

    /// Endianness: M (MSB) or L (LSB)
    #[arg(short = 'e', long = "endianness", default_value = "M")]
    endianness: char,

    /// Block size in bytes
    #[arg(short = 's', long = "bs", default_value = "4096")]
    block_size: i32,

    /// Dither type: T (TPDF), R (rectangular), N (NJAD), F (float), X (none)
    #[arg(short = 'd', long = "dither")]
    dither_type: Option<char>,

    /// Decimation ratio: 8, 16, 32, or 64
    #[arg(short = 'r', long = "ratio", default_value = "8")]
    decimation: i32,

    /// Input DSD rate: 1 (dsd64) or 2 (dsd128)
    #[arg(short = 'i', long = "inrate", default_value = "1")]
    input_rate: i32,

    /// Output type: S (stdout), A (aif), W (wave), F (flac)
    #[arg(short = 'o', long = "output", default_value = "S")]
    output: char,

    /// Volume level adjustment in dB
    #[arg(short = 'l', long = "level", default_value = "0.0")]
    level: f64,

    /// Print diagnostic messages
    #[arg(short = 'v', long = "verbose")]
    verbose: bool,

    /// Input files (use - for stdin)
    #[arg(name = "FILES")]
    files: Vec<String>,
}

fn verbose(message: &str, new_line: bool) {
    unsafe {
        if VERBOSE_MODE {
            if new_line {
                eprintln!("{}", message);
            } else {
                eprint!("{}", message);
            }
        }
    }
}

fn main() -> Result<(), Box<dyn Error>> {
    let cli = Cli::parse();
    unsafe { VERBOSE_MODE = cli.verbose; }

    let inputs = if cli.files.is_empty() {
        vec!["-".to_string()]
    } else {
        cli.files.clone()
    };

    let dither_type = cli.dither_type.unwrap_or(
        if cli.bit_depth == 32 { 'F' } else { 'T' }
    );

    let filter_type = cli.filter_type.unwrap_or(
        if cli.input_rate == 2 { 'C' } else { 'X' }
    );

    let mut out_ctx = OutputContext::new(
        cli.bit_depth,
        cli.output,
        cli.decimation,
        filter_type,
        cli.level,
    )?;

    let mut dither = Dither::new(dither_type)?;

    for input in inputs {
        // Check for unexpanded glob patterns
        if input.contains('*') {
            verbose(
                &format!("Warning: Unexpanded glob pattern detected in input: \"{}\". Skipping.", input),
                true
            );
            continue;
        }

        verbose(&format!("Input: {}", input), true);

        // Create input context
        let in_ctx = InputContext::new(
            input,
            cli.format,
            cli.endianness,
            cli.input_rate,
            cli.block_size,
            cli.channels,
            cli.verbose,
        )?;

        // Create dither and conversion context
        let mut conv_ctx = ConversionContext::new(
            in_ctx,
            out_ctx.clone(),  // Clone here instead
            dither.clone(),   // Clone here instead
            cli.verbose
        )?;
        conv_ctx.do_conversion()?;
    }

    Ok(())
}