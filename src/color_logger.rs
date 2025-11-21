use std::io::{self, Write};

use colored::Colorize;
use log::{Level, LevelFilter, Metadata, Record};

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
