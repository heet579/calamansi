//! Running an action over a selection of files.
//!
//! Each input gets its own result: one failure never stops the others.

use crate::checksum;
use crate::error::{CoreError, Result};
use crate::format::{self, Format, Kind};
use crate::image_ops;
use crate::media::Ffmpeg;
use crate::naming;
use crate::pdf_ops;
use crate::routes::{self, Engine};
use crate::settings::Settings;
use crate::subtitles;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::Duration;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tool {
    ImagesToPdf,
    MergePdf,
    SplitPdf,
    Resize,
    RotateRight,
    Sha256,
}

impl Tool {
    pub fn label(self) -> &'static str {
        match self {
            Tool::ImagesToPdf => "Make PDF",
            Tool::MergePdf => "Merge",
            Tool::SplitPdf => "Split",
            Tool::Resize => "Resize",
            Tool::RotateRight => "Rotate",
            Tool::Sha256 => "SHA-256",
        }
    }

    /// Whether this tool makes sense for the selection.
    pub fn applies_to(self, inputs: &[Format]) -> bool {
        let all = |pred: fn(Format) -> bool| !inputs.is_empty() && inputs.iter().all(|&f| pred(f));
        let native_image = |f: Format| routes::route(f, Format::Pdf) == Some(Engine::Native);
        match self {
            Tool::ImagesToPdf => all(native_image),
            Tool::MergePdf => inputs.len() >= 2 && all(|f| f == Format::Pdf),
            Tool::SplitPdf => all(|f| f == Format::Pdf),
            Tool::Resize | Tool::RotateRight => all(native_image),
            Tool::Sha256 => !inputs.is_empty(),
        }
    }

    /// Tools that produce one output from the whole selection.
    pub fn is_aggregate(self) -> bool {
        matches!(self, Tool::ImagesToPdf | Tool::MergePdf)
    }

    pub const ALL: &'static [Tool] = &[
        Tool::ImagesToPdf,
        Tool::MergePdf,
        Tool::SplitPdf,
        Tool::Resize,
        Tool::RotateRight,
        Tool::Sha256,
    ];
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Convert(Format),
    Tool(Tool),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Files written.
    Done(Vec<PathBuf>),
    /// A computed value (e.g. a checksum), nothing written.
    Value(String),
    Skipped(String),
    Failed(String),
    Cancelled,
}

#[derive(Clone, Debug)]
pub struct FileResult {
    pub input: PathBuf,
    pub outcome: Outcome,
}

#[derive(Clone, Debug, Default)]
pub struct Report {
    pub results: Vec<FileResult>,
}

impl Report {
    pub fn outputs(&self) -> Vec<&Path> {
        self.results
            .iter()
            .flat_map(|r| match &r.outcome {
                Outcome::Done(v) => v.iter().map(PathBuf::as_path).collect(),
                _ => Vec::new(),
            })
            .collect()
    }
    pub fn count(&self, pred: impl Fn(&Outcome) -> bool) -> usize {
        self.results.iter().filter(|r| pred(&r.outcome)).count()
    }
    pub fn failures(&self) -> Vec<(&Path, &str)> {
        self.results
            .iter()
            .filter_map(|r| match &r.outcome {
                Outcome::Failed(m) => Some((r.input.as_path(), m.as_str())),
                _ => None,
            })
            .collect()
    }
    pub fn values(&self) -> Vec<(&Path, &str)> {
        self.results
            .iter()
            .filter_map(|r| match &r.outcome {
                Outcome::Value(v) => Some((r.input.as_path(), v.as_str())),
                _ => None,
            })
            .collect()
    }
    pub fn cancelled(&self) -> bool {
        self.results.iter().any(|r| r.outcome == Outcome::Cancelled)
    }
}

#[derive(Clone, Debug)]
pub struct Progress {
    pub done: usize,
    pub total: usize,
}

pub struct Context<'a> {
    pub settings: &'a Settings,
    pub ffmpeg: Option<&'a Ffmpeg>,
    pub cancel: &'a AtomicBool,
}

/// Run `action` over `inputs`, reporting progress after each file.
pub fn run(
    action: Action,
    inputs: &[PathBuf],
    cx: &Context,
    on_progress: &(dyn Fn(Progress) + Sync),
) -> Report {
    if let Action::Tool(t) = action {
        if t.is_aggregate() {
            on_progress(Progress { done: 0, total: 1 });
            let outcome = match aggregate(t, inputs, cx) {
                Ok(p) => Outcome::Done(vec![p]),
                Err(CoreError::Cancelled) => Outcome::Cancelled,
                Err(e) => Outcome::Failed(e.to_string()),
            };
            on_progress(Progress { done: 1, total: 1 });
            let input = inputs.first().cloned().unwrap_or_default();
            return Report {
                results: vec![FileResult { input, outcome }],
            };
        }
    }

    // FFmpeg is already multi-threaded; native work parallelises per file.
    let uses_ffmpeg = matches!(action, Action::Convert(to) if inputs.iter().any(|p| {
        format::detect(p).and_then(|f| routes::route(f, to)) == Some(Engine::Ffmpeg)
    }));
    let workers = if uses_ffmpeg {
        1
    } else {
        std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(2)
            .clamp(1, 4)
    };

    let total = inputs.len();
    let next = AtomicUsize::new(0);
    let done = AtomicUsize::new(0);
    let results: Mutex<Vec<Option<FileResult>>> = Mutex::new(vec![None; total]);
    on_progress(Progress { done: 0, total });

    std::thread::scope(|s| {
        for _ in 0..workers.min(total.max(1)) {
            s.spawn(|| loop {
                let i = next.fetch_add(1, Ordering::SeqCst);
                if i >= total {
                    break;
                }
                let input = &inputs[i];
                let outcome = if cx.cancel.load(Ordering::Relaxed) {
                    Outcome::Cancelled
                } else {
                    run_one(action, input, cx)
                };
                results.lock().unwrap()[i] = Some(FileResult {
                    input: input.clone(),
                    outcome,
                });
                let d = done.fetch_add(1, Ordering::SeqCst) + 1;
                on_progress(Progress { done: d, total });
            });
        }
    });

    Report {
        results: results
            .into_inner()
            .unwrap()
            .into_iter()
            .flatten()
            .collect(),
    }
}

fn run_one(action: Action, input: &Path, cx: &Context) -> Outcome {
    let result = match action {
        Action::Convert(to) => convert(input, to, cx),
        Action::Tool(Tool::Sha256) => {
            return match checksum::sha256_file(input, cx.cancel) {
                Ok(h) => Outcome::Value(h),
                Err(CoreError::Cancelled) => Outcome::Cancelled,
                Err(e) => Outcome::Failed(e.to_string()),
            }
        }
        Action::Tool(Tool::SplitPdf) => split(input, cx),
        Action::Tool(Tool::Resize) => edit_image(input, cx, "resized", |img| {
            Ok(image_ops::fit_within(&img, cx.settings.resize_max_side))
        }),
        Action::Tool(Tool::RotateRight) => {
            edit_image(input, cx, "rotated", |img| image_ops::rotate(&img, 90))
        }
        Action::Tool(t) => Err(CoreError::InvalidInput(format!(
            "{} runs on the whole selection",
            t.label()
        ))),
    };
    match result {
        Ok(Some(paths)) => Outcome::Done(paths),
        Ok(None) => Outcome::Skipped("already in that format".into()),
        Err(CoreError::Cancelled) => Outcome::Cancelled,
        Err(e) => Outcome::Failed(e.to_string()),
    }
}

/// Reserve an output, fill it, and clean up on failure.
fn produce(
    dir: &Path,
    stem: &str,
    ext: &str,
    fill: impl FnOnce(&Path) -> Result<()>,
) -> Result<PathBuf> {
    let out = naming::reserve(dir, stem, ext)?;
    match fill(&out) {
        Ok(()) => Ok(out),
        Err(e) => {
            naming::release(&out);
            Err(e)
        }
    }
}

fn convert(input: &Path, to: Format, cx: &Context) -> Result<Option<Vec<PathBuf>>> {
    let from = format::detect(input).ok_or(CoreError::UnknownFormat)?;
    if from == to {
        return Ok(None);
    }
    let engine = routes::route(from, to).ok_or(CoreError::Unsupported { from, to })?;
    let dir = cx.settings.output.resolve(input);
    let stem = naming::stem_of(input);
    let quality = cx.settings.quality;

    let out = match engine {
        Engine::Ffmpeg => {
            let ff = cx.ffmpeg.ok_or(CoreError::FfmpegMissing)?;
            produce(&dir, &stem, to.extension(), |out| {
                let tmp = naming::external_temp(out);
                let args = ff.args(from, to, input, &tmp, quality)?;
                let timeout =
                    Duration::from_secs(u64::from(cx.settings.ffmpeg_timeout_minutes) * 60);
                let r = ff
                    .run(&args, cx.cancel, timeout)
                    .and_then(|_| Ok(naming::commit_external(&tmp, out)?));
                if r.is_err() {
                    let _ = std::fs::remove_file(&tmp);
                }
                r
            })?
        }
        Engine::Native => produce(&dir, &stem, to.extension(), |out| {
            naming::write_atomically(out, |w| native_convert(input, from, to, quality, w))
        })?,
    };
    Ok(Some(vec![out]))
}

fn native_convert(
    input: &Path,
    from: Format,
    to: Format,
    quality: u8,
    w: &mut std::io::BufWriter<std::fs::File>,
) -> Result<()> {
    match (from.kind(), to) {
        (Kind::Image, Format::Pdf) => pdf_ops::images_to_pdf(&[input], quality, w),
        (Kind::Image, _) => {
            let img = image_ops::load(input)?;
            image_ops::encode(&img, to, quality, w)
        }
        (Kind::Document, Format::Txt) => Ok(w.write_all(pdf_ops::extract_text(input)?.as_bytes())?),
        (Kind::Document, Format::Pdf) => {
            image_ops::check_size(input)?;
            let text = String::from_utf8_lossy(&std::fs::read(input)?).into_owned();
            pdf_ops::text_to_pdf(&text, w)
        }
        (Kind::Subtitle, _) => {
            image_ops::check_size(input)?;
            let text = String::from_utf8_lossy(&std::fs::read(input)?).into_owned();
            Ok(w.write_all(subtitles::convert(&text, to)?.as_bytes())?)
        }
        _ => Err(CoreError::Unsupported { from, to }),
    }
}

fn edit_image(
    input: &Path,
    cx: &Context,
    suffix: &str,
    edit: impl FnOnce(image::DynamicImage) -> Result<image::DynamicImage>,
) -> Result<Option<Vec<PathBuf>>> {
    let from = format::detect(input).ok_or(CoreError::UnknownFormat)?;
    if routes::route(from, Format::Pdf) != Some(Engine::Native) {
        return Err(CoreError::InvalidInput(format!(
            "{from} files can't be edited yet"
        )));
    }
    let img = edit(image_ops::load(input)?)?;
    let dir = cx.settings.output.resolve(input);
    let stem = format!("{} {suffix}", naming::stem_of(input));
    let out = produce(&dir, &stem, from.extension(), |out| {
        naming::write_atomically(out, |w| {
            image_ops::encode(&img, from, cx.settings.quality, w)
        })
    })?;
    Ok(Some(vec![out]))
}

fn split(input: &Path, cx: &Context) -> Result<Option<Vec<PathBuf>>> {
    let pages = pdf_ops::page_count(input)?;
    let dir = cx.settings.output.resolve(input);
    let stem = naming::stem_of(input);
    let mut outs: Vec<PathBuf> = Vec::with_capacity(pages);
    for n in 1..=pages as u32 {
        if cx.cancel.load(Ordering::Relaxed) {
            for o in &outs {
                naming::release(o);
            }
            return Err(CoreError::Cancelled);
        }
        let out = produce(&dir, &format!("{stem} page {n}"), "pdf", |out| {
            naming::write_atomically(out, |w| pdf_ops::extract_page(input, n, w))
        })?;
        outs.push(out);
    }
    Ok(Some(outs))
}

fn aggregate(tool: Tool, inputs: &[PathBuf], cx: &Context) -> Result<PathBuf> {
    let first = inputs
        .first()
        .ok_or_else(|| CoreError::InvalidInput("no files selected".into()))?;
    let dir = cx.settings.output.resolve(first);
    let refs: Vec<&Path> = inputs.iter().map(PathBuf::as_path).collect();
    let stem = naming::stem_of(first);
    match tool {
        Tool::ImagesToPdf => produce(&dir, &stem, "pdf", |out| {
            naming::write_atomically(out, |w| {
                pdf_ops::images_to_pdf(&refs, cx.settings.quality, w)
            })
        }),
        Tool::MergePdf => produce(&dir, &format!("{stem} merged"), "pdf", |out| {
            naming::write_atomically(out, |w| pdf_ops::merge(&refs, w))
        }),
        other => Err(CoreError::InvalidInput(format!(
            "{} works on one file at a time",
            other.label()
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::OutputLocation;
    use image::{DynamicImage, Rgba, RgbaImage};

    fn pngs(dir: &Path, n: usize) -> Vec<PathBuf> {
        (0..n)
            .map(|i| {
                let p = dir.join(format!("img{i}.png"));
                DynamicImage::ImageRgba8(RgbaImage::from_pixel(
                    30 + i as u32,
                    20,
                    Rgba([0, 120, 0, 255]),
                ))
                .save(&p)
                .unwrap();
                p
            })
            .collect()
    }

    fn ctx<'a>(settings: &'a Settings, cancel: &'a AtomicBool) -> Context<'a> {
        Context {
            settings,
            ffmpeg: None,
            cancel,
        }
    }

    #[test]
    fn batch_converts_each_file_and_isolates_failures() {
        let dir = tempfile::tempdir().unwrap();
        let mut inputs = pngs(dir.path(), 4);
        let broken = dir.path().join("broken.png");
        std::fs::write(&broken, b"\x89PNG\r\n\x1a\nnot really").unwrap();
        inputs.insert(2, broken);
        let settings = Settings::default();
        let cancel = AtomicBool::new(false);
        let seen = AtomicUsize::new(0);
        let report = run(
            Action::Convert(Format::Jpeg),
            &inputs,
            &ctx(&settings, &cancel),
            &|_| {
                seen.fetch_add(1, Ordering::SeqCst);
            },
        );
        assert_eq!(report.results.len(), 5);
        assert_eq!(report.outputs().len(), 4);
        assert_eq!(report.failures().len(), 1);
        assert_eq!(
            report.results[2].input, inputs[2],
            "results keep input order"
        );
        assert_eq!(seen.load(Ordering::SeqCst), 6, "initial + one per file");
        for out in report.outputs() {
            assert_eq!(format::detect(out), Some(Format::Jpeg));
        }
        // No temp or placeholder files left behind.
        let leftovers = std::fs::read_dir(dir.path())
            .unwrap()
            .filter(|e| {
                e.as_ref()
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .contains("calamansi")
            })
            .count();
        assert_eq!(leftovers, 0);
    }

    #[test]
    fn same_format_is_skipped_and_outputs_go_to_chosen_folder() {
        let dir = tempfile::tempdir().unwrap();
        let out_dir = dir.path().join("out");
        let inputs = pngs(dir.path(), 1);
        let settings = Settings {
            output: OutputLocation::Folder(out_dir.clone()),
            ..Default::default()
        };
        let cancel = AtomicBool::new(false);
        let r = run(
            Action::Convert(Format::Png),
            &inputs,
            &ctx(&settings, &cancel),
            &|_| {},
        );
        assert!(matches!(r.results[0].outcome, Outcome::Skipped(_)));
        let r = run(
            Action::Convert(Format::Webp),
            &inputs,
            &ctx(&settings, &cancel),
            &|_| {},
        );
        assert!(r.outputs()[0].starts_with(&out_dir));
    }

    #[test]
    fn tools() {
        let dir = tempfile::tempdir().unwrap();
        let inputs = pngs(dir.path(), 3);
        let settings = Settings {
            resize_max_side: 16,
            ..Default::default()
        };
        let cancel = AtomicBool::new(false);
        let cx = ctx(&settings, &cancel);

        let r = run(Action::Tool(Tool::ImagesToPdf), &inputs, &cx, &|_| {});
        let pdf = r.outputs()[0].to_path_buf();
        assert_eq!(pdf_ops::page_count(&pdf).unwrap(), 3);

        let r = run(
            Action::Tool(Tool::SplitPdf),
            std::slice::from_ref(&pdf),
            &cx,
            &|_| {},
        );
        assert_eq!(r.outputs().len(), 3);

        let r = run(
            Action::Tool(Tool::MergePdf),
            &[pdf.clone(), pdf],
            &cx,
            &|_| {},
        );
        assert_eq!(pdf_ops::page_count(r.outputs()[0]).unwrap(), 6);

        let r = run(Action::Tool(Tool::Resize), &inputs[..1], &cx, &|_| {});
        let img = image::open(r.outputs()[0]).unwrap();
        assert_eq!(img.width(), 16);
        assert!(r.outputs()[0].to_string_lossy().contains("resized"));

        let r = run(Action::Tool(Tool::Sha256), &inputs, &cx, &|_| {});
        assert_eq!(r.values().len(), 3);
    }

    #[test]
    fn cancelled_before_start() {
        let dir = tempfile::tempdir().unwrap();
        let inputs = pngs(dir.path(), 2);
        let settings = Settings::default();
        let cancel = AtomicBool::new(true);
        let r = run(
            Action::Convert(Format::Jpeg),
            &inputs,
            &ctx(&settings, &cancel),
            &|_| {},
        );
        assert!(r.cancelled() && r.outputs().is_empty());
    }

    #[test]
    fn ffmpeg_route_without_ffmpeg_fails_clearly() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("clip.mp4");
        std::fs::write(&p, b"\0\0\0\x18ftypisom\0\0\0\0").unwrap();
        let settings = Settings::default();
        let cancel = AtomicBool::new(false);
        let r = run(
            Action::Convert(Format::Mp3),
            &[p],
            &ctx(&settings, &cancel),
            &|_| {},
        );
        assert!(r.failures()[0].1.contains("FFmpeg"));
    }

    #[test]
    fn tool_applicability() {
        use Format::*;
        assert!(Tool::MergePdf.applies_to(&[Pdf, Pdf]));
        assert!(!Tool::MergePdf.applies_to(&[Pdf]));
        assert!(Tool::ImagesToPdf.applies_to(&[Png, Jpeg]));
        assert!(!Tool::ImagesToPdf.applies_to(&[Png, Heic]));
        assert!(Tool::Sha256.applies_to(&[Mp4]));
    }
}
