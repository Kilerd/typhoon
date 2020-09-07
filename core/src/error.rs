use std::process::ExitStatus;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum TyphoonError {
    #[error("Error on opening file {} : {}", .0, .1)]
    FileError(String, std::io::Error),
    #[error("Parser error: {}", .0)]
    ParserError(String),
    #[error("Error on opening file {}", .0)]
    CompileError(String),
    #[error("Error on linking output file as binary ({}) \nSTDOUT: {}\nSTDERR: {}", .0, .1, .2)]
    LinkError(ExitStatus, String, String),
}
