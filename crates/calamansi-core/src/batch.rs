//! Running an action over a selection of files.
//!
//! Each input gets its own result: one failure never stops the others.

use crate::checksum;
use crate::docx;
use crate::error::{CoreError, Result};
use crate::format::{self, Format, Kind, OfficeFamily};
use crate::image_ops;
use crate::media::{Ffmpeg, MediaTool};
use crate::naming;
use crate::office::Office;
use crate::pdf_ops;
use crate::routes::{self, Available, Engine};
use crate::settings::Settings;
use crate::subtitles;
use image::DynamicImage;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::Duration;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tool {
    // One output from the whole selection.
    ImagesToPdf,
    Collage,
    MergePdf,
    // Images.
    Resize,
    CompressImage,
    RotateRight,
    RotateLeft,
    Flip,
    Grayscale,
    StripMetadata,
    // PDFs.
    SplitPdf,
    RotatePdf,
    ReversePdf,
    CompressPdf,
    // Audio and video (FFmpeg).
    Media(MediaTool),
    // Any file.
    Duplicate,
    CopyPath,
    Sha256,
}

impl Tool {
    pub fn label(self) -> &'static str {
        match self {
            Tool::ImagesToPdf => "Make PDF",
            Tool::Collage => "Collage",
            Tool::MergePdf => "Merge",
            Tool::Resize => "Resize",
            Tool::CompressImage => "Compress",
            Tool::RotateRight => "Rotate \u{21bb}",
            Tool::RotateLeft => "Rotate \u{21ba}",
            Tool::Flip => "Flip",
            Tool::Grayscale => "Grayscale",
            Tool::StripMetadata => "No metadata",
            Tool::SplitPdf => "Split",
            Tool::RotatePdf => "Rotate",
            Tool::ReversePdf => "Reverse",
            Tool::CompressPdf => "Compress",
            Tool::Media(MediaTool::CompressVideo) => "Compress",
            Tool::Media(MediaTool::Mute) => "Mute",
            Tool::Media(MediaTool::Snapshot) => "Snapshot",
            Tool::Media(MediaTool::RotateVideo) => "Rotate",
            Tool::Media(MediaTool::Normalize) => "Normalize",
            Tool::Media(MediaTool::Mono) => "Mono",
            Tool::Duplicate => "Duplicate",
            Tool::CopyPath => "Copy path",
            Tool::Sha256 => "SHA-256",
        }
    }

    /// Whether this tool makes sense for the selection.
    pub fn applies_to(self, inputs: &[Format], available: Available) -> bool {
        if inputs.is_empty() {
            return false;
        }
        let all = |pred: fn(Format) -> bool| inputs.iter().all(|&f| pred(f));
        let image = |f: Format| routes::native_image_in(f);
        let pdf = |f: Format| f == Format::Pdf;
        let video = |f: Format| f.kind() == Kind::Video;
        let audio = |f: Format| f.kind() == Kind::Audio;
        match self {
            Tool::ImagesToPdf => all(image),
            Tool::Collage => inputs.len() >= 2 && all(image),
            Tool::MergePdf => inputs.len() >= 2 && all(pdf),
            Tool::Resize
            | Tool::CompressImage
            | Tool::RotateRight
            | Tool::RotateLeft
            | Tool::Flip
            | Tool::Grayscale
            | Tool::StripMetadata => all(image),
            Tool::SplitPdf | Tool::RotatePdf | Tool::ReversePdf | Tool::CompressPdf => all(pdf),
            Tool::Media(m) => {
                available.ffmpeg
                    && match m {
                        MediaTool::CompressVideo
                        | MediaTool::Mute
                        | MediaTool::Snapshot
                        | MediaTool::RotateVideo => all(video),
                        MediaTool::Normalize | MediaTool::Mono => all(audio),
                    }
            }
            Tool::Duplicate | Tool::CopyPath | Tool::Sha256 => true,
        }
    }

    /// Tools that produce one output from the whole selection.
    pub fn is_aggregate(self) -> bool {
        matches!(self, Tool::ImagesToPdf | Tool::Collage | Tool::MergePdf)
    }

    /// Tools whose result is text for the clipboard rather than files.
    pub fn is_value(self) -> bool {
        matches!(self, Tool::CopyPath | Tool::Sha256)
    }

    /// Wheel order: specific tools first, generic file actions last.
    pub const ALL: &'static [Tool] = &[
        Tool::ImagesToPdf,
        Tool::Collage,
        Tool::MergePdf,
        Tool::Resize,
        Tool::CompressImage,
        Tool::RotateRight,
        Tool::RotateLeft,
        Tool::Flip,
        Tool::Grayscale,
        Tool::StripMetadata,
        Tool::SplitPdf,
        Tool::RotatePdf,
        Tool::ReversePdf,
        Tool::CompressPdf,
        Tool::Media(MediaTool::CompressVideo),
        Tool::Media(MediaTool::Mute),
        Tool::Media(MediaTool::Snapshot),
        Tool::Media(MediaTool::RotateVideo),
        Tool::Media(MediaTool::Normalize),
        Tool::Media(MediaTool::Mono),
        Tool::Duplicate,
        Tool::CopyPath,
        Tool::Sha256,
    ];
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Convert(Format),
    Tool(Tool),
}

impl Action {
    pub fn label(self) -> String {
        match self {
            Action::Convert(f) => format!("Convert to {f}"),
            Action::Tool(t) => t.label().to_string(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Files written.
    Done(Vec<PathBuf>),
    /// A computed value (a checksum, a path), nothing written.
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
    pub office: Option<&'a Office>,
    pub cancel: &'a AtomicBool,
}

impl Context<'_> {
    pub fn available(&self) -> Available {
        Available {
            ffmpeg: self.ffmpeg.is_some(),
            office: self.office.is_some(),
        }
    }

    fn timeout(&self) -> Duration {
        Duration::from_secs(u64::from(self.settings.ffmpeg_timeout_minutes) * 60)
    }
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

    // Helper programs run one at a time (FFmpeg is already multi-threaded;
    // Office automation must not run concurrently). Native work parallelises.
    let external = match action {
        Action::Convert(to) => {
            inputs.iter().any(|p| {
                !matches!(
                    format::detect(p).and_then(|f| routes::route(f, to)),
                    Some(Engine::Native) | None
                )
            }) || (to == Format::Docx && cx.office.is_some_and(Office::word_reads_pdf))
        }
        Action::Tool(Tool::Media(_)) => true,
        Action::Tool(_) => false,
    };
    let workers = if external {
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
        Action::Tool(Tool::CopyPath) => {
            let abs = std::path::absolute(input).unwrap_or_else(|_| input.to_path_buf());
            return Outcome::Value(abs.display().to_string());
        }
        Action::Tool(t) => tool(t, input, cx),
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

/// Like [`produce`], for helper programs that write a file themselves.
fn produce_external(
    dir: &Path,
    stem: &str,
    ext: &str,
    write: impl FnOnce(&Path) -> Result<()>,
) -> Result<PathBuf> {
    produce(dir, stem, ext, |out| {
        let tmp = naming::external_temp(out);
        let r = write(&tmp).and_then(|_| Ok(naming::commit_external(&tmp, out)?));
        if r.is_err() {
            let _ = std::fs::remove_file(&tmp);
        }
        r
    })
}

fn read_text(input: &Path) -> Result<String> {
    image_ops::check_size(input)?;
    Ok(String::from_utf8_lossy(&std::fs::read(input)?).into_owned())
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

    // PDF pages to pictures: one image per page.
    if from == Format::Pdf && to.kind() == Kind::Image {
        return pdf_to_images(input, to, cx).map(Some);
    }

    // PDF → Word: Word itself gives a faithful, editable result when installed.
    if from == Format::Pdf && to == Format::Docx {
        if let Some(office) = cx.office.filter(|o| o.word_reads_pdf()) {
            let via_word = produce_external(&dir, &stem, "docx", |tmp| {
                office.convert(
                    input,
                    OfficeFamily::Text,
                    Format::Docx,
                    tmp,
                    cx.cancel,
                    cx.timeout(),
                )
            });
            match via_word {
                Ok(p) => return Ok(Some(vec![p])),
                Err(CoreError::Cancelled) => return Err(CoreError::Cancelled),
                Err(_) => {} // fall back to the built-in text conversion
            }
        }
    }

    let out = match engine {
        Engine::Ffmpeg => {
            let ff = cx.ffmpeg.ok_or(CoreError::FfmpegMissing)?;
            produce_external(&dir, &stem, to.extension(), |tmp| {
                let args = ff.args(from, to, input, tmp, quality)?;
                ff.run(&args, cx.cancel, cx.timeout())
            })?
        }
        Engine::Office => {
            let office = cx.office.ok_or_else(|| {
                CoreError::InvalidInput(
                    "this needs Microsoft Office or LibreOffice, which wasn't found".into(),
                )
            })?;
            let family = from
                .office_family()
                .ok_or(CoreError::Unsupported { from, to })?;
            produce_external(&dir, &stem, to.extension(), |tmp| {
                office.convert(input, family, to, tmp, cx.cancel, cx.timeout())
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
        (Kind::Document, Format::Pdf) => pdf_ops::text_to_pdf(&read_text(input)?, w),
        (Kind::Document, Format::Docx) => {
            let pages = if from == Format::Pdf {
                pdf_ops::page_texts(input)?
            } else {
                vec![read_text(input)?]
            };
            docx::write(&pages, w)
        }
        (Kind::Office, Format::Txt) => {
            image_ops::check_size(input)?;
            let text = docx::read_text(std::fs::File::open(input)?)?;
            Ok(w.write_all(text.as_bytes())?)
        }
        (Kind::Subtitle, _) => {
            Ok(w.write_all(subtitles::convert(&read_text(input)?, to)?.as_bytes())?)
        }
        _ => Err(CoreError::Unsupported { from, to }),
    }
}

fn pdf_to_images(input: &Path, to: Format, cx: &Context) -> Result<Vec<PathBuf>> {
    let dir = cx.settings.output.resolve(input);
    let stem = naming::stem_of(input);
    let pages = pdf_ops::page_count(input).unwrap_or(0);
    let mut outs: Vec<PathBuf> = Vec::new();
    let result = pdf_ops::render_pages(
        input,
        cx.settings.pdf_image_dpi as f32,
        cx.cancel,
        |n, img| {
            let name = if pages == 1 {
                stem.clone()
            } else {
                format!("{stem} page {n}")
            };
            let out = produce(&dir, &name, to.extension(), |out| {
                naming::write_atomically(out, |w| {
                    image_ops::encode(&img, to, cx.settings.quality, w)
                })
            })?;
            outs.push(out);
            Ok(())
        },
    );
    if let Err(e) = result {
        for o in &outs {
            naming::release(o);
        }
        return Err(e);
    }
    Ok(outs)
}

fn tool(t: Tool, input: &Path, cx: &Context) -> Result<Option<Vec<PathBuf>>> {
    let max = cx.settings.resize_max_side;
    match t {
        Tool::Resize => edit_image(input, cx, "resized", None, |img| {
            Ok(image_ops::fit_within(&img, max))
        }),
        Tool::CompressImage => edit_image(input, cx, "compressed", Some(Format::Jpeg), |img| {
            Ok(image_ops::fit_within(&img, 2560))
        }),
        Tool::RotateRight => edit_image(input, cx, "rotated", None, |img| {
            image_ops::rotate(&img, 90)
        }),
        Tool::RotateLeft => edit_image(input, cx, "rotated", None, |img| {
            image_ops::rotate(&img, 270)
        }),
        Tool::Flip => edit_image(input, cx, "flipped", None, |img| Ok(img.fliph())),
        Tool::Grayscale => edit_image(input, cx, "grayscale", None, |img| Ok(img.grayscale())),
        Tool::StripMetadata => edit_image(input, cx, "clean", None, Ok),
        Tool::SplitPdf => split(input, cx),
        Tool::RotatePdf => edit_pdf(input, cx, "rotated", |w| {
            pdf_ops::rotate_pages(input, 90, w)
        }),
        Tool::ReversePdf => edit_pdf(input, cx, "reversed", |w| pdf_ops::reverse_pages(input, w)),
        Tool::CompressPdf => edit_pdf(input, cx, "compressed", |w| pdf_ops::compress(input, w)),
        Tool::Media(m) => media_tool(m, input, cx),
        Tool::Duplicate => {
            let dir = input.parent().unwrap_or(Path::new("."));
            let ext = input.extension().and_then(|e| e.to_str()).unwrap_or("");
            let stem = format!("{} copy", naming::stem_of(input));
            let out = produce_external(dir, &stem, ext, |tmp| {
                std::fs::copy(input, tmp)?;
                Ok(())
            })?;
            Ok(Some(vec![out]))
        }
        Tool::ImagesToPdf | Tool::Collage | Tool::MergePdf | Tool::CopyPath | Tool::Sha256 => Err(
            CoreError::InvalidInput(format!("{} isn't a per-file tool", t.label())),
        ),
    }
}

fn edit_image(
    input: &Path,
    cx: &Context,
    suffix: &str,
    out_format: Option<Format>,
    edit: impl FnOnce(DynamicImage) -> Result<DynamicImage>,
) -> Result<Option<Vec<PathBuf>>> {
    let from = format::detect(input).ok_or(CoreError::UnknownFormat)?;
    if !routes::native_image_in(from) {
        return Err(CoreError::InvalidInput(format!(
            "{from} files can't be edited yet"
        )));
    }
    let to = out_format.unwrap_or(from);
    let quality = if out_format == Some(Format::Jpeg) {
        70
    } else {
        cx.settings.quality.max(90)
    };
    let img = edit(image_ops::load(input)?)?;
    let dir = cx.settings.output.resolve(input);
    let stem = format!("{} {suffix}", naming::stem_of(input));
    let out = produce(&dir, &stem, to.extension(), |out| {
        naming::write_atomically(out, |w| image_ops::encode(&img, to, quality, w))
    })?;
    Ok(Some(vec![out]))
}

fn edit_pdf(
    input: &Path,
    cx: &Context,
    suffix: &str,
    write: impl FnOnce(&mut std::io::BufWriter<std::fs::File>) -> Result<()>,
) -> Result<Option<Vec<PathBuf>>> {
    let dir = cx.settings.output.resolve(input);
    let stem = format!("{} {suffix}", naming::stem_of(input));
    let out = produce(&dir, &stem, "pdf", |out| {
        naming::write_atomically(out, write)
    })?;
    Ok(Some(vec![out]))
}

fn media_tool(m: MediaTool, input: &Path, cx: &Context) -> Result<Option<Vec<PathBuf>>> {
    let ff = cx.ffmpeg.ok_or(CoreError::FfmpegMissing)?;
    let from = format::detect(input).ok_or(CoreError::UnknownFormat)?;
    let (suffix, ext) = match m {
        MediaTool::CompressVideo => ("compressed", "mp4"),
        MediaTool::Mute => ("muted", from.extension()),
        MediaTool::Snapshot => ("frame", "png"),
        MediaTool::RotateVideo => ("rotated", "mp4"),
        MediaTool::Normalize => ("normalized", from.extension()),
        MediaTool::Mono => ("mono", from.extension()),
    };
    let dir = cx.settings.output.resolve(input);
    let stem = format!("{} {suffix}", naming::stem_of(input));
    let out = produce_external(&dir, &stem, ext, |tmp| {
        let args = ff.tool_args(m, from, input, tmp)?;
        ff.run(&args, cx.cancel, cx.timeout())
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
        Tool::Collage => {
            let mut images = Vec::with_capacity(inputs.len());
            for p in inputs {
                if cx.cancel.load(Ordering::Relaxed) {
                    return Err(CoreError::Cancelled);
                }
                images.push(image_ops::fit_within(&image_ops::load(p)?, 1200));
            }
            let img = image_ops::collage(&images, 600);
            produce(&dir, &format!("{stem} collage"), "jpg", |out| {
                naming::write_atomically(out, |w| image_ops::encode(&img, Format::Jpeg, 90, w))
            })
        }
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
    use image::{Rgba, RgbaImage};

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
            office: None,
            cancel,
        }
    }

    fn no_leftovers(dir: &Path) {
        for e in std::fs::read_dir(dir).unwrap() {
            let name = e.unwrap().file_name().to_string_lossy().into_owned();
            assert!(!name.contains("calamansi"), "leftover {name}");
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
        no_leftovers(dir.path());
    }

    #[test]
    fn image_to_pdf_and_back_to_image() {
        let dir = tempfile::tempdir().unwrap();
        let inputs = pngs(dir.path(), 2);
        let settings = Settings::default();
        let cancel = AtomicBool::new(false);
        let cx = ctx(&settings, &cancel);

        let r = run(Action::Convert(Format::Pdf), &inputs, &cx, &|_| {});
        assert_eq!(r.outputs().len(), 2, "one PDF per image");
        let pdf = r.outputs()[0].to_path_buf();
        assert_eq!(format::detect(&pdf), Some(Format::Pdf));
        assert_eq!(pdf.file_name().unwrap(), "img0.pdf");

        let r = run(
            Action::Convert(Format::Png),
            std::slice::from_ref(&pdf),
            &cx,
            &|_| {},
        );
        assert!(r.failures().is_empty(), "{:?}", r.failures());
        let png = r.outputs()[0];
        assert_eq!(
            png.file_name().unwrap(),
            "img0 (2).png",
            "never overwrites the original image"
        );
        assert_eq!(format::detect(png), Some(Format::Png));

        let r = run(
            Action::Convert(Format::Jpeg),
            std::slice::from_ref(&pdf),
            &cx,
            &|_| {},
        );
        assert_eq!(format::detect(r.outputs()[0]), Some(Format::Jpeg));
        no_leftovers(dir.path());
    }

    #[test]
    fn text_pdf_word_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let txt = dir.path().join("notes.txt");
        std::fs::write(&txt, "Quarterly notes\nSecond line\n").unwrap();
        let settings = Settings::default();
        let cancel = AtomicBool::new(false);
        let cx = ctx(&settings, &cancel);

        let r = run(
            Action::Convert(Format::Pdf),
            std::slice::from_ref(&txt),
            &cx,
            &|_| {},
        );
        let pdf = r.outputs()[0].to_path_buf();

        let r = run(
            Action::Convert(Format::Docx),
            std::slice::from_ref(&pdf),
            &cx,
            &|_| {},
        );
        let docx = r.outputs()[0].to_path_buf();
        assert_eq!(docx.extension().unwrap(), "docx");
        assert_eq!(format::detect(&docx), Some(Format::Docx));

        let r = run(
            Action::Convert(Format::Txt),
            std::slice::from_ref(&docx),
            &cx,
            &|_| {},
        );
        let text = std::fs::read_to_string(r.outputs()[0]).unwrap();
        assert!(text.contains("Quarterly notes"), "{text}");

        let r = run(
            Action::Convert(Format::Docx),
            std::slice::from_ref(&txt),
            &cx,
            &|_| {},
        );
        assert!(r.failures().is_empty());
    }

    #[test]
    fn office_route_without_office_fails_clearly() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("report.xlsx");
        std::fs::write(&p, b"PK\x03\x04").unwrap();
        let settings = Settings::default();
        let cancel = AtomicBool::new(false);
        let r = run(
            Action::Convert(Format::Pdf),
            &[p],
            &ctx(&settings, &cancel),
            &|_| {},
        );
        assert!(r.failures()[0].1.contains("Office"), "{:?}", r.failures());
        no_leftovers(dir.path());
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
    fn image_tools() {
        let dir = tempfile::tempdir().unwrap();
        let inputs = pngs(dir.path(), 3);
        let settings = Settings {
            resize_max_side: 16,
            ..Default::default()
        };
        let cancel = AtomicBool::new(false);
        let cx = ctx(&settings, &cancel);

        let r = run(Action::Tool(Tool::ImagesToPdf), &inputs, &cx, &|_| {});
        assert_eq!(pdf_ops::page_count(r.outputs()[0]).unwrap(), 3);

        let r = run(Action::Tool(Tool::Collage), &inputs, &cx, &|_| {});
        assert_eq!(format::detect(r.outputs()[0]), Some(Format::Jpeg));

        let r = run(Action::Tool(Tool::Resize), &inputs[..1], &cx, &|_| {});
        assert_eq!(image::open(r.outputs()[0]).unwrap().width(), 16);

        let r = run(Action::Tool(Tool::RotateLeft), &inputs[..1], &cx, &|_| {});
        assert_eq!(image::open(r.outputs()[0]).unwrap().width(), 20);

        for t in [
            Tool::Flip,
            Tool::Grayscale,
            Tool::StripMetadata,
            Tool::RotateRight,
        ] {
            let r = run(Action::Tool(t), &inputs[..1], &cx, &|_| {});
            assert_eq!(format::detect(r.outputs()[0]), Some(Format::Png), "{t:?}");
        }
        let r = run(
            Action::Tool(Tool::CompressImage),
            &inputs[..1],
            &cx,
            &|_| {},
        );
        assert_eq!(format::detect(r.outputs()[0]), Some(Format::Jpeg));
        no_leftovers(dir.path());
    }

    #[test]
    fn pdf_tools() {
        let dir = tempfile::tempdir().unwrap();
        let inputs = pngs(dir.path(), 3);
        let settings = Settings::default();
        let cancel = AtomicBool::new(false);
        let cx = ctx(&settings, &cancel);
        let pdf =
            run(Action::Tool(Tool::ImagesToPdf), &inputs, &cx, &|_| {}).outputs()[0].to_path_buf();

        let r = run(
            Action::Tool(Tool::SplitPdf),
            std::slice::from_ref(&pdf),
            &cx,
            &|_| {},
        );
        assert_eq!(r.outputs().len(), 3);
        let r = run(
            Action::Tool(Tool::MergePdf),
            &[pdf.clone(), pdf.clone()],
            &cx,
            &|_| {},
        );
        assert_eq!(pdf_ops::page_count(r.outputs()[0]).unwrap(), 6);
        for t in [Tool::RotatePdf, Tool::ReversePdf, Tool::CompressPdf] {
            let r = run(Action::Tool(t), std::slice::from_ref(&pdf), &cx, &|_| {});
            assert!(r.failures().is_empty(), "{t:?} {:?}", r.failures());
            assert_eq!(pdf_ops::page_count(r.outputs()[0]).unwrap(), 3, "{t:?}");
        }
        let r = run(
            Action::Convert(Format::Png),
            std::slice::from_ref(&pdf),
            &cx,
            &|_| {},
        );
        assert_eq!(r.outputs().len(), 3, "one image per page");
        assert!(r.outputs()[1].to_string_lossy().contains("page 2"));
    }

    #[test]
    fn file_actions() {
        let dir = tempfile::tempdir().unwrap();
        let inputs = pngs(dir.path(), 2);
        let settings = Settings::default();
        let cancel = AtomicBool::new(false);
        let cx = ctx(&settings, &cancel);
        let r = run(Action::Tool(Tool::Sha256), &inputs, &cx, &|_| {});
        assert_eq!(r.values().len(), 2);
        let r = run(Action::Tool(Tool::CopyPath), &inputs, &cx, &|_| {});
        assert!(r.values()[0].1.ends_with("img0.png"));
        let r = run(Action::Tool(Tool::Duplicate), &inputs[..1], &cx, &|_| {});
        assert_eq!(r.outputs()[0].file_name().unwrap(), "img0 copy.png");
        assert_eq!(
            std::fs::read(r.outputs()[0]).unwrap(),
            std::fs::read(&inputs[0]).unwrap()
        );
        no_leftovers(dir.path());
    }

    #[test]
    fn media_tools_with_ffmpeg_if_installed() {
        let Some(ff) = Ffmpeg::locate(None) else {
            return;
        };
        let dir = tempfile::tempdir().unwrap();
        let clip = dir.path().join("clip.mp4");
        let ok = std::process::Command::new(&ff.path)
            .args([
                "-loglevel",
                "error",
                "-y",
                "-f",
                "lavfi",
                "-i",
                "testsrc=duration=2:size=160x120:rate=10",
                "-f",
                "lavfi",
                "-i",
                "sine=duration=2",
                "-shortest",
                "-pix_fmt",
                "yuv420p",
            ])
            .arg(&clip)
            .status()
            .unwrap();
        assert!(ok.success());
        let settings = Settings::default();
        let cancel = AtomicBool::new(false);
        let cx = Context {
            settings: &settings,
            ffmpeg: Some(&ff),
            office: None,
            cancel: &cancel,
        };
        for m in [
            MediaTool::CompressVideo,
            MediaTool::Mute,
            MediaTool::Snapshot,
            MediaTool::RotateVideo,
        ] {
            let r = run(
                Action::Tool(Tool::Media(m)),
                std::slice::from_ref(&clip),
                &cx,
                &|_| {},
            );
            assert!(r.failures().is_empty(), "{m:?}: {:?}", r.failures());
        }
        let wav = run(
            Action::Convert(Format::Wav),
            std::slice::from_ref(&clip),
            &cx,
            &|_| {},
        )
        .outputs()[0]
            .to_path_buf();
        for m in [MediaTool::Normalize, MediaTool::Mono] {
            let r = run(
                Action::Tool(Tool::Media(m)),
                std::slice::from_ref(&wav),
                &cx,
                &|_| {},
            );
            assert!(r.failures().is_empty(), "{m:?}: {:?}", r.failures());
        }
        no_leftovers(dir.path());
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
        let none = Available::NONE;
        assert!(Tool::MergePdf.applies_to(&[Pdf, Pdf], none));
        assert!(!Tool::MergePdf.applies_to(&[Pdf], none));
        assert!(Tool::ImagesToPdf.applies_to(&[Png, Jpeg], none));
        assert!(!Tool::ImagesToPdf.applies_to(&[Png, Heic], none));
        assert!(!Tool::Collage.applies_to(&[Png], none));
        assert!(Tool::Sha256.applies_to(&[Mp4], none));
        assert!(!Tool::Media(MediaTool::Mute).applies_to(&[Mp4], none));
        assert!(Tool::Media(MediaTool::Mute).applies_to(&[Mp4], Available::ALL));
        assert!(!Tool::Media(MediaTool::Mute).applies_to(&[Mp3], Available::ALL));
    }
}
