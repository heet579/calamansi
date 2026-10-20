//! `calamansi-cli` — the conversion engine from the command line.
//!
//! Useful for scripting, for testing the engine without the Windows shell,
//! and for platforms that don't have a GUI yet.

use anyhow::{bail, Context as _, Result};
use calamansi_core::batch::{self, Action, Context, Outcome, Tool};
use calamansi_core::format::{self, Format};
use calamansi_core::media::{Ffmpeg, MediaTool};
use calamansi_core::office::Office;
use calamansi_core::routes::{self, Engine};
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
    /// Use this LibreOffice `soffice` executable.
    #[arg(long, global = true)]
    office: Option<PathBuf>,
    #[command(subcommand)]
    command: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Convert files to another format, e.g. `calamansi-cli convert *.png --to pdf`.
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
    /// Run a tool, e.g. `calamansi-cli tool merge-pdf a.pdf b.pdf`. `tool --list` shows them all.
    Tool {
        /// Tool name (see --list).
        name: Option<String>,
        files: Vec<PathBuf>,
        /// List the tools that apply to FILES (or all tools).
        #[arg(long)]
        list: bool,
        /// Longest side for `resize`.
        #[arg(long, default_value_t = 1920)]
        max: u32,
    },
    /// Show which helper programs (FFmpeg, Office) were found.
    Doctor {
        /// Also run sample conversions in DIR (default: a temp folder).
        #[arg(long, value_name = "DIR", num_args = 0..=1, default_missing_value = "")]
        self_test: Option<PathBuf>,
    },
}

/// Command-line names for tools.
const TOOL_NAMES: &[(&str, Tool)] = &[
    ("images-to-pdf", Tool::ImagesToPdf),
    ("collage", Tool::Collage),
    ("merge-pdf", Tool::MergePdf),
    ("resize", Tool::Resize),
    ("compress-image", Tool::CompressImage),
    ("rotate-right", Tool::RotateRight),
    ("rotate-left", Tool::RotateLeft),
    ("flip", Tool::Flip),
    ("grayscale", Tool::Grayscale),
    ("strip-metadata", Tool::StripMetadata),
    ("split-pdf", Tool::SplitPdf),
    ("rotate-pdf", Tool::RotatePdf),
    ("reverse-pdf", Tool::ReversePdf),
    ("compress-pdf", Tool::CompressPdf),
    ("compress-video", Tool::Media(MediaTool::CompressVideo)),
    ("mute", Tool::Media(MediaTool::Mute)),
    ("snapshot", Tool::Media(MediaTool::Snapshot)),
    ("rotate-video", Tool::Media(MediaTool::RotateVideo)),
    ("normalize", Tool::Media(MediaTool::Normalize)),
    ("mono", Tool::Media(MediaTool::Mono)),
    ("duplicate", Tool::Duplicate),
    ("copy-path", Tool::CopyPath),
    ("sha256", Tool::Sha256),
];

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
    if cli.office.is_some() {
        settings.office_path = cli.office;
    }
    let ffmpeg = Ffmpeg::locate(settings.ffmpeg_path.as_deref());
    let office = Office::locate(settings.office_path.as_deref());
    let available = routes::Available {
        ffmpeg: ffmpeg.is_some(),
        office: office.is_some(),
    };

    let (action, files) = match cli.command {
        Cmd::Doctor { self_test } => {
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
            match &office {
                Some(o) => println!("office:   {}", o.name()),
                None => println!(
                    "office:   not found. Word/Excel/PowerPoint conversions need Microsoft Office or LibreOffice"
                ),
            }
            if let Some(dir) = self_test {
                let dir = if dir.as_os_str().is_empty() {
                    std::env::temp_dir().join("calamansi-self-test")
                } else {
                    dir
                };
                settings.output = OutputLocation::BesideSource;
                let cancel = AtomicBool::new(false);
                let cx = Context {
                    settings: &settings,
                    ffmpeg: ffmpeg.as_ref(),
                    office: office.as_ref(),
                    cancel: &cancel,
                };
                let checks = calamansi_core::selftest::run(&dir, &cx);
                let failed = checks.iter().filter(|c| !c.passed()).count();
                for c in &checks {
                    match &c.result {
                        Ok(_) => println!("  ok    {}", c.name),
                        Err(e) => println!("  FAIL  {}: {e}", c.name),
                    }
                }
                println!(
                    "{} of {} checks passed; files in {}",
                    checks.len() - failed,
                    checks.len(),
                    dir.display()
                );
                return Ok(if failed == 0 {
                    ExitCode::SUCCESS
                } else {
                    ExitCode::FAILURE
                });
            }
            return Ok(ExitCode::SUCCESS);
        }
        Cmd::Targets { files } => {
            let formats = detect_all(&files)?;
            for (f, engine) in routes::targets_for(&formats, available) {
                let note = match engine {
                    Engine::Native => "",
                    Engine::Ffmpeg => "  (FFmpeg)",
                    Engine::Office => "  (Office)",
                };
                println!("{:<10} .{}{note}", f.label(), f.extension());
            }
            return Ok(ExitCode::SUCCESS);
        }
        Cmd::Convert { to, files } => {
            let f = Format::parse(&to).with_context(|| format!("unknown format `{to}`"))?;
            (Action::Convert(f), files)
        }
        Cmd::Tool {
            name,
            files,
            list,
            max,
        } => {
            if list || name.is_none() {
                // With --list, a lone positional is a file, not a tool name.
                let files: Vec<PathBuf> =
                    name.map(PathBuf::from).into_iter().chain(files).collect();
                let formats = if files.is_empty() {
                    None
                } else {
                    Some(detect_all(&files)?)
                };
                for (n, t) in TOOL_NAMES {
                    if formats.as_ref().is_none_or(|f| t.applies_to(f, available)) {
                        println!("{n:<16} {}", t.label());
                    }
                }
                return Ok(ExitCode::SUCCESS);
            }
            let name = name.unwrap_or_default();
            let tool = TOOL_NAMES
                .iter()
                .find(|(n, _)| *n == name)
                .map(|(_, t)| *t)
                .with_context(|| format!("unknown tool `{name}` (see `tool --list`)"))?;
            if files.is_empty() {
                bail!("give at least one file");
            }
            settings.resize_max_side = max;
            (Action::Tool(tool), files)
        }
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
        office: office.as_ref(),
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
            Outcome::Value(v) if action == Action::Tool(Tool::CopyPath) => println!("{v}"),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_tool_has_a_cli_name() {
        for t in Tool::ALL {
            assert!(TOOL_NAMES.iter().any(|(_, x)| x == t), "{t:?}");
        }
    }
}
