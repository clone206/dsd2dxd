use core::fmt;
use std::process::{ExitCode, Termination};

use log::error;

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

// Removed custom FromResidual impl: using a separate run() -> Result<(), MyError>
// function keeps error handling ergonomic on stable Rust without nightly features.

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
