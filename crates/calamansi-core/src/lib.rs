//! Calamansi's conversion engine.
//!
//! Everything here is platform-neutral and runs locally: no network access,
//! no telemetry. The Windows app and the `calamansi` CLI are thin shells
//! around [`batch::run`].

pub mod batch;
pub mod checksum;
pub mod error;
pub mod format;
pub mod image_ops;
pub mod media;
pub mod naming;
pub mod pdf_ops;
pub mod routes;
pub mod settings;
pub mod subtitles;
pub mod wheel;

pub use error::{CoreError, Result};
pub use format::Format;
