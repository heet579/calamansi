use crate::format::Format;
use std::io;

/// Every failure the engine can report. Messages are written for end users:
/// they are shown as-is in the progress pill and the CLI.
#[derive(Debug, thiserror::Error)]
pub enum CoreError {
    #[error("{0}")]
    Io(#[from] io::Error),

    #[error("{from} to {to} isn't supported")]
    Unsupported { from: Format, to: Format },

    #[error("couldn't recognise the file type")]
    UnknownFormat,

    #[error("{0}")]
    TooLarge(String),

    #[error("image error: {0}")]
    Image(#[from] image::ImageError),

    #[error("PDF error: {0}")]
    Pdf(String),

    #[error("this needs FFmpeg, which wasn't found (install it with `winget install ffmpeg`)")]
    FfmpegMissing,

    #[error("FFmpeg failed: {0}")]
    Ffmpeg(String),

    #[error("{0}")]
    InvalidInput(String),

    #[error("cancelled")]
    Cancelled,
}

impl From<lopdf::Error> for CoreError {
    fn from(e: lopdf::Error) -> Self {
        CoreError::Pdf(e.to_string())
    }
}

pub type Result<T, E = CoreError> = std::result::Result<T, E>;
