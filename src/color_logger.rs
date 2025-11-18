use std::io::{self, Write};

use log::{Level, Metadata, Record};
use colored::Colorize;

pub struct ColorLogger {
    quiet: bool,
}

impl ColorLogger {
    pub fn init(verbose: bool, quiet: bool) {
        let level = if verbose { Level::Trace } else { Level::Info };
        log::set_boxed_logger(Box::new(ColorLogger { quiet }))
            .map(|()| log::set_max_level(level.to_level_filter()))
            .expect("Failed to initialize logger");
    }
}

impl log::Log for ColorLogger {
    fn enabled(&self, metadata: &Metadata) -> bool {
        !self.quiet && metadata.level() <= log::max_level()
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
