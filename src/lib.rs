use core::fmt;
use std::process::{ExitCode, Termination};
use std::io::{self, Write};
use colored::Colorize;
use log::{Level, LevelFilter, Metadata, Record, error};

#[derive(Debug)]
pub enum MyError {
    Message(String),
}

impl std::fmt::Display for MyError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MyError::Message(msg) => write!(f, "{}", msg),
        }
    }
}

impl std::error::Error for MyError {}

pub type MyResult<T> = Result<T, MyError>;

pub struct TermResult(pub MyResult<()>);

impl Termination for TermResult {
    fn report(self) -> ExitCode {
        match self.0 {
            Ok(_) => ExitCode::SUCCESS,
            Err(err) => {
                error!("{}", err);
                ExitCode::FAILURE
            }
        }
    }
}

// Convert boxed dynamic errors into MyError
impl From<Box<dyn std::error::Error>> for MyError {
    fn from(err: Box<dyn std::error::Error>) -> Self {
        MyError::Message(err.to_string())
    }
}

pub struct ColorLogger {
    max_level: LevelFilter,
}

impl ColorLogger {
    pub fn new(quiet: bool, verbose: bool) -> Self {
        let max_level = if quiet {
            LevelFilter::Off
        } else if verbose {
            LevelFilter::Trace
        } else {
            LevelFilter::Info
        };
        Self {
            max_level,
        }
    }

    #[allow(dead_code)]
    pub fn init(&self) {
        log::set_boxed_logger(Box::new(self.clone()))
        .expect("Failed to initialize logger");
    }
}

impl Clone for ColorLogger {
    fn clone(&self) -> Self {
        Self {
            max_level: self.max_level,
        }
    }
}

impl log::Log for ColorLogger {
    fn enabled(&self, metadata: &Metadata) -> bool {
         metadata.level() <= self.max_level
    }

    fn log(&self, record: &Record) {
        if self.enabled(record.metadata()) {
            match record.level() {
                Level::Error => eprintln!(
                    "{} {}",
                    "[ERROR]".red().bold(),
                    format!("{}", record.args()).red().bold()
                ),
                Level::Warn => eprintln!(
                    "{} {}",
                    "[WARN]".yellow().bold(),
                    format!("{}", record.args()).yellow().bold()
                ),
                _ => eprintln!(
                    "[{}] {}",
                    record.level().to_string().blue(),
                    record.args()
                ),
            }
        }
        self.flush();
    }

    fn flush(&self) {
        io::stderr().flush().unwrap();
    }
}
