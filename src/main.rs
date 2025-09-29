use clap::Parser;
use std::error::Error;

mod audio_file;
mod byte_precalc_decimator;
mod conversion_context;
mod dither;
mod dsd;
mod dsd2pcm;
mod dsdin_sys;
mod fir_convolve;
mod input;
mod output;
mod lm_resampler;

pub use conversion_context::ConversionContext;
pub use dither::Dither;
pub use dsd2pcm::Dxd;
pub use input::InputContext;
pub use output::OutputContext;

static mut VERBOSE_MODE: bool = false;

#[derive(Parser)]
#[command(name = "dsd2dxd")]
struct Cli {
    /// Number of channels
    #[arg(short = 'c', long = "channels", default_value = "2")]
    channels: Option<i32>,

    /// Format: Interleaved (I) or Planar (P)
    #[arg(short = 'f', long = "fmt", default_value = "I")]
    format: char,

    /// Bit depth: 16, 20, 24, or 32 (float)
    #[arg(short = 'b', long = "bitdepth", default_value = "24")]
    bit_depth: i32,

    /// Filter type: X (XLD), D (Original), 
    /// E (Equiripple. Only available with double rate DSD input, or 88.2K output from DSD64), C (Chebyshev. Only available with double rate DSD input) [default: X if single rate, E if double rate]
    #[arg(short = 't', long = "filttype")]
    filter_type: Option<char>,

    /// Endianness: M (MSB) or L (LSB)
    #[arg(short = 'e', long = "endianness", default_value = "M")]
    endianness: char,

    /// Block size in bytes
    #[arg(short = 's', long = "bs", default_value = "4096")]
    block_size: Option<i32>,

    /// Dither type: T (TPDF), R (rectangular), N (NJAD), F (float), X (none) [default: F for 32 bit, T otherwise]
    #[arg(short = 'd', long = "dither")]
    dither_type: Option<char>,

    /// Output sample rate in Hz
    #[arg(short = 'r', long = "rate", default_value = "352800")]
    output_rate: i32,

    /// Input DSD rate: 1 (dsd64) or 2 (dsd128)
    #[arg(short = 'i', long = "inrate", default_value = "1")]
    input_rate: i32,

    /// Output type: S (stdout), A (aif), W (wave), F (flac)
    /// Note that W, A, or F outputs to either 
    /// <basename>.[wav|aif|flac] in current directory,
    /// where <basename> is the input filename 
    /// without the extension, or output.[wav|aif|flac] if reading from stdin.)
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
    unsafe {
        VERBOSE_MODE = cli.verbose;
    }

    let inputs = if cli.files.is_empty() {
        vec!["-".to_string()]
    } else {
        cli.files.clone()
    };

    let dither_type = cli
        .dither_type
        .unwrap_or(if cli.bit_depth == 32 { 'F' } else { 'T' });

    let mut out_ctx = OutputContext::new(
        cli.bit_depth,
        cli.output,
        cli.level,
        cli.output_rate,
    )?;

    let dither = Dither::new(dither_type)?;

    for input in inputs {
        // Check for unexpanded glob patterns
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

        // Create input context
        let in_ctx = InputContext::new(
            input.clone(),
            cli.format,
            cli.endianness,
            cli.input_rate,
            cli.block_size.unwrap_or(4096),
            cli.channels.unwrap_or(2),
            cli.verbose,
        )?;

        let filter_type = if cli.filter_type.is_some() {
            cli.filter_type.unwrap().to_ascii_uppercase()
        } else {
            match in_ctx.dsd_rate {
                2 => 'E',
                _ => 'X',
            }
        };

        // Create conversion context
        let mut conv_ctx = ConversionContext::new(
            in_ctx,
            out_ctx.clone(),
            dither.clone(),
            filter_type,
            cli.verbose,
        )?;
        conv_ctx.do_conversion()?;
    }

    Ok(())
}
