//! A quick end-to-end check of the engine on this machine.
//!
//! Generates sample files in a folder and runs the most common conversions
//! and tools on them, including FFmpeg and Office routes when available. Used
//! by the tray's "Run self-test" and `calamansi-cli doctor --self-test`, so a
//! user can tell an engine problem from a drag-and-drop problem.

use crate::batch::{self, Action, Context, Outcome, Tool};
use crate::format::Format;
use crate::media::MediaTool;
use image::{DynamicImage, Rgba, RgbaImage};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug)]
pub struct Check {
    pub name: String,
    pub result: Result<Vec<PathBuf>, String>,
}

impl Check {
    pub fn passed(&self) -> bool {
        self.result.is_ok()
    }
}

fn sample_image(path: &Path, w: u32, h: u32, color: [u8; 3]) -> std::io::Result<()> {
    let img = RgbaImage::from_fn(w, h, |x, y| {
        let shade = ((x + y) % 64) as u8;
        Rgba([
            color[0].saturating_add(shade),
            color[1],
            color[2].saturating_add(shade),
            255,
        ])
    });
    DynamicImage::ImageRgba8(img)
        .save(path)
        .map_err(std::io::Error::other)
}

/// Run the checks in `dir` (created if needed). Outputs stay in `dir` so the
/// user can look at them.
pub fn run(dir: &Path, cx: &Context) -> Vec<Check> {
    let mut checks = Vec::new();
    if let Err(e) = std::fs::create_dir_all(dir) {
        checks.push(Check {
            name: "Create test folder".into(),
            result: Err(e.to_string()),
        });
        return checks;
    }
    let a = dir.join("sample A.png");
    let b = dir.join("sample B.png");
    let txt = dir.join("sample notes.txt");
    let setup = sample_image(&a, 640, 480, [200, 80, 40])
        .and_then(|_| sample_image(&b, 480, 640, [40, 120, 200]))
        .and_then(|_| {
            std::fs::write(
                &txt,
                "Calamansi self-test\nThis line should survive every conversion.\n",
            )
        });
    if let Err(e) = setup {
        checks.push(Check {
            name: "Create sample files".into(),
            result: Err(e.to_string()),
        });
        return checks;
    }

    let mut step = |name: &str, action: Action, inputs: &[PathBuf]| -> Option<PathBuf> {
        let report = batch::run(action, inputs, cx, &|_| {});
        let failures: Vec<String> = report
            .failures()
            .iter()
            .map(|(_, m)| m.to_string())
            .collect();
        let outputs: Vec<PathBuf> = report.outputs().into_iter().map(PathBuf::from).collect();
        let values = report.count(|o| matches!(o, Outcome::Value(_)));
        let result = if !failures.is_empty() {
            Err(failures.join("; "))
        } else if outputs.is_empty() && values == 0 {
            Err("no output".into())
        } else {
            Ok(outputs.clone())
        };
        checks.push(Check {
            name: name.into(),
            result,
        });
        outputs.into_iter().next()
    };

    step(
        "PNG → JPEG",
        Action::Convert(Format::Jpeg),
        std::slice::from_ref(&a),
    );
    let pdf = step(
        "Image → PDF",
        Action::Convert(Format::Pdf),
        std::slice::from_ref(&a),
    );
    step(
        "Images → one PDF",
        Action::Tool(Tool::ImagesToPdf),
        &[a.clone(), b.clone()],
    );
    step(
        "Collage",
        Action::Tool(Tool::Collage),
        &[a.clone(), b.clone()],
    );
    step(
        "Resize",
        Action::Tool(Tool::Resize),
        std::slice::from_ref(&b),
    );
    step(
        "Rotate",
        Action::Tool(Tool::RotateRight),
        std::slice::from_ref(&b),
    );
    let text_pdf = step(
        "Text → PDF",
        Action::Convert(Format::Pdf),
        std::slice::from_ref(&txt),
    );
    if let Some(pdf) = &pdf {
        step(
            "PDF → PNG",
            Action::Convert(Format::Png),
            std::slice::from_ref(pdf),
        );
    }
    if let Some(tp) = &text_pdf {
        step(
            "PDF → Word",
            Action::Convert(Format::Docx),
            std::slice::from_ref(tp),
        );
        step(
            "PDF → Text",
            Action::Convert(Format::Txt),
            std::slice::from_ref(tp),
        );
        if let Some(pdf) = &pdf {
            step(
                "Merge PDFs",
                Action::Tool(Tool::MergePdf),
                &[pdf.clone(), tp.clone()],
            );
        }
        step(
            "Rotate PDF",
            Action::Tool(Tool::RotatePdf),
            std::slice::from_ref(tp),
        );
    }
    step(
        "SHA-256",
        Action::Tool(Tool::Sha256),
        std::slice::from_ref(&a),
    );

    if let Some(ff) = cx.ffmpeg {
        let clip = dir.join("sample clip.mp4");
        let made = std::process::Command::new(&ff.path)
            .args([
                "-hide_banner",
                "-loglevel",
                "error",
                "-y",
                "-f",
                "lavfi",
                "-i",
                "testsrc=duration=2:size=320x240:rate=15",
                "-f",
                "lavfi",
                "-i",
                "sine=duration=2",
                "-shortest",
                "-pix_fmt",
                "yuv420p",
            ])
            .arg(&clip)
            .stdin(std::process::Stdio::null())
            .output();
        match made {
            Ok(o) if o.status.success() => {
                step(
                    "Video → MP3",
                    Action::Convert(Format::Mp3),
                    std::slice::from_ref(&clip),
                );
                step(
                    "Video → GIF",
                    Action::Convert(Format::Gif),
                    std::slice::from_ref(&clip),
                );
                step(
                    "Video snapshot",
                    Action::Tool(Tool::Media(MediaTool::Snapshot)),
                    std::slice::from_ref(&clip),
                );
            }
            _ => checks.push(Check {
                name: "FFmpeg sample video".into(),
                result: Err("FFmpeg couldn't create a test video".into()),
            }),
        }
    }

    if cx.office.is_some() {
        let mut step2 = |name: &str, action: Action, inputs: &[PathBuf]| {
            let report = batch::run(action, inputs, cx, &|_| {});
            let failures: Vec<String> = report
                .failures()
                .iter()
                .map(|(_, m)| m.to_string())
                .collect();
            let outputs: Vec<PathBuf> = report.outputs().into_iter().map(PathBuf::from).collect();
            let result = if failures.is_empty() && !outputs.is_empty() {
                Ok(outputs)
            } else {
                Err(failures.join("; "))
            };
            checks.push(Check {
                name: name.into(),
                result,
            });
        };
        let docx = dir.join("sample letter.docx");
        let made = std::fs::File::create(&docx)
            .map_err(crate::CoreError::from)
            .and_then(|f| crate::docx::write(&["Calamansi self-test (Word document)".into()], f));
        if made.is_ok() {
            step2(
                "Word → PDF (Office)",
                Action::Convert(Format::Pdf),
                std::slice::from_ref(&docx),
            );
        }
    }
    checks
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::Settings;
    use std::sync::atomic::AtomicBool;

    #[test]
    fn all_native_checks_pass() {
        let dir = tempfile::tempdir().unwrap();
        let settings = Settings::default();
        let cancel = AtomicBool::new(false);
        let cx = Context {
            settings: &settings,
            ffmpeg: None,
            office: None,
            cancel: &cancel,
        };
        let checks = run(&dir.path().join("self-test"), &cx);
        assert!(checks.len() >= 12);
        for c in &checks {
            assert!(c.passed(), "{}: {:?}", c.name, c.result);
        }
    }
}
