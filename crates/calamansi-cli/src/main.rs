//! `calamansi-cli` — the conversion engine from the command line.
//!
//! Useful for scripting, for testing the engine without the Windows shell,
//! and for platforms that don't have a GUI yet.

use anyhow::{bail, Context as _, Result};
use calamansi_core::batch::{self, Action, Context, Outcome, Tool};
use calamansi_core::format::{self, Format};
use calamansi_core::media::Ffmpeg;
use calamansi_core::routes;
use calamansi_core::settings::{OutputLocation, Settings};
use clap::{Parser, Subcommand};
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::atomic::AtomicBool;

#[derive(Parser)]
#[command(name = "calamansi-cli", version, about = "Local file converter and toolbox", long_about = None)]
struct Cli {
    /// Write outputs to this folder instead of next to each input.
    #[arg(long, short, global = true)]
    out: Option<PathBuf>,
    /// Quality for JPEG / AVIF / lossy WebP (1-100).
    #[arg(long, short, global = true)]
    quality: Option<u8>,
    /// Use this FFmpeg executable.
    #[arg(long, global = true)]
    ffmpeg: Option<PathBuf>,
    #[command(subcommand)]
    command: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Convert files to another format, e.g. `calamansi-cli convert *.png --to jpg`.
    Convert {
        #[arg(long, short)]
        to: String,
        #[arg(required = true)]
        files: Vec<PathBuf>,
    },
    /// List the formats the given files can be converted to.
    Targets {
        #[arg(required = true)]
        files: Vec<PathBuf>,
    },
    /// Combine images into one PDF, one page per image.
    ImagesToPdf {
        #[arg(required = true)]
        files: Vec<PathBuf>,
    },
    /// Merge PDFs in the order given.
    MergePdf {
        #[arg(required = true, num_args = 2..)]
        files: Vec<PathBuf>,
    },
    /// Split each PDF into one file per page.
    SplitPdf {
        #[arg(required = true)]
        files: Vec<PathBuf>,
    },
    /// Shrink images so the longest side is at most --max (never enlarges).
    Resize {
        #[arg(long, default_value_t = 1920)]
        max: u32,
        #[arg(required = true)]
        files: Vec<PathBuf>,
    },
    /// Rotate images 90° clockwise.
    Rotate {
        #[arg(required = true)]
        files: Vec<PathBuf>,
    },
    /// Print SHA-256 checksums.
    Sha256 {
        #[arg(required = true)]
        files: Vec<PathBuf>,
    },
    /// Show whether FFmpeg was found and which settings file is used.
    Doctor,
}

fn main() -> ExitCode {
    match run() {
        Ok(code) => code,
        Err(e) => {
            eprintln!("error: {e:#}");
            ExitCode::from(2)
        }
    }
}

fn run() -> Result<ExitCode> {
    let cli = Cli::parse();
    let mut settings = Settings::default_path()
        .map(|p| Settings::load(&p))
        .unwrap_or_default();
    if let Some(dir) = cli.out {
        settings.output = OutputLocation::Folder(dir);
    }
    if let Some(q) = cli.quality {
        settings.quality = q.clamp(1, 100);
    }
    if cli.ffmpeg.is_some() {
        settings.ffmpeg_path = cli.ffmpeg;
    }
    let ffmpeg = Ffmpeg::locate(settings.ffmpeg_path.as_deref());

    let (action, files) = match cli.command {
        Cmd::Doctor => {
            println!("calamansi-cli {}", env!("CARGO_PKG_VERSION"));
            match Settings::default_path() {
                Some(p) => println!(
                    "settings: {}{}",
                    p.display(),
                    if p.exists() { "" } else { " (not created yet)" }
                ),
                None => println!("settings: no config directory"),
            }
            match &ffmpeg {
                Some(f) => println!("ffmpeg:   {} ({})", f.path.display(), f.version),
                None => println!("ffmpeg:   not found. Audio/video need it: winget install ffmpeg"),
            }
            return Ok(ExitCode::SUCCESS);
        }
        Cmd::Targets { files } => {
            let formats = detect_all(&files)?;
            for (f, engine) in routes::targets_for(&formats, ffmpeg.is_some()) {
                let note = if engine == routes::Engine::Ffmpeg {
                    "  (FFmpeg)"
                } else {
                    ""
                };
                println!("{:<5} .{}{note}", f.label(), f.extension());
            }
            return Ok(ExitCode::SUCCESS);
        }
        Cmd::Convert { to, files } => {
            let f = Format::parse(&to).with_context(|| format!("unknown format `{to}`"))?;
            (Action::Convert(f), files)
        }
        Cmd::ImagesToPdf { files } => (Action::Tool(Tool::ImagesToPdf), files),
        Cmd::MergePdf { files } => (Action::Tool(Tool::MergePdf), files),
        Cmd::SplitPdf { files } => (Action::Tool(Tool::SplitPdf), files),
        Cmd::Resize { max, files } => {
            settings.resize_max_side = max;
            (Action::Tool(Tool::Resize), files)
        }
        Cmd::Rotate { files } => (Action::Tool(Tool::RotateRight), files),
        Cmd::Sha256 { files } => (Action::Tool(Tool::Sha256), files),
    };

    for f in &files {
        if !f.is_file() {
            bail!("{} isn't a file", f.display());
        }
    }
    let cancel = AtomicBool::new(false);
    let cx = Context {
        settings: &settings,
        ffmpeg: ffmpeg.as_ref(),
        cancel: &cancel,
    };
    let report = batch::run(action, &files, &cx, &|_| {});

    let mut failed = false;
    for r in &report.results {
        let name = r.input.display();
        match &r.outcome {
            Outcome::Done(outs) => {
                for o in outs {
                    println!("{name} -> {}", o.display());
                }
            }
            Outcome::Value(v) => println!("{v}  {name}"),
            Outcome::Skipped(why) => println!("{name}: skipped ({why})"),
            Outcome::Failed(why) => {
                failed = true;
                eprintln!("{name}: {why}");
            }
            Outcome::Cancelled => eprintln!("{name}: cancelled"),
        }
    }
    Ok(if failed {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    })
}

fn detect_all(files: &[PathBuf]) -> Result<Vec<Format>> {
    files
        .iter()
        .map(|p| {
            format::detect(p).with_context(|| format!("{}: unrecognised file type", p.display()))
        })
        .collect()
}
