//! Which conversions exist, and which engine performs each one.

use crate::format::{Format, Kind, OfficeFamily};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Engine {
    /// Pure Rust, always available.
    Native,
    /// Requires an FFmpeg executable on the machine.
    Ffmpeg,
    /// Requires Microsoft Office or LibreOffice on the machine.
    Office,
}

/// Optional helper programs found on this machine.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Available {
    pub ffmpeg: bool,
    pub office: bool,
}

impl Available {
    pub const NONE: Available = Available {
        ffmpeg: false,
        office: false,
    };
    pub const ALL: Available = Available {
        ffmpeg: true,
        office: true,
    };

    pub fn has(self, engine: Engine) -> bool {
        match engine {
            Engine::Native => true,
            Engine::Ffmpeg => self.ffmpeg,
            Engine::Office => self.office,
        }
    }
}

/// Images the built-in decoder can read.
pub fn native_image_in(f: Format) -> bool {
    use Format::*;
    matches!(f, Png | Jpeg | Webp | Gif | Bmp | Tiff | Ico)
}

/// Images the built-in encoder can write.
fn native_image_out(f: Format) -> bool {
    use Format::*;
    matches!(f, Png | Jpeg | Webp | Gif | Bmp | Tiff | Ico)
}

/// The engine that converts `from` into `to`, or `None` if there is no route.
pub fn route(from: Format, to: Format) -> Option<Engine> {
    use Format::*;
    if from == to {
        return None;
    }
    match (from.kind(), to.kind()) {
        (Kind::Image, Kind::Image) => {
            if to == Heic {
                // No widely available HEIC encoder ships with FFmpeg builds.
                None
            } else if native_image_in(from) && native_image_out(to) {
                Some(Engine::Native)
            } else {
                Some(Engine::Ffmpeg)
            }
        }
        (Kind::Image, _) if to == Pdf => native_image_in(from).then_some(Engine::Native),
        (Kind::Document, _) => match (from, to) {
            (Pdf, Txt) | (Txt, Pdf) => Some(Engine::Native),
            // PDF pages rendered to pictures.
            (Pdf, Png | Jpeg | Webp | Tiff | Bmp) => Some(Engine::Native),
            // Built-in DOCX writer; Word is used instead when installed.
            (Pdf | Txt, Docx) => Some(Engine::Native),
            _ => None,
        },
        (Kind::Office, _) => {
            let family = from.office_family()?;
            match to {
                Pdf => Some(Engine::Office),
                Txt if from == Docx => Some(Engine::Native),
                Docx if family == OfficeFamily::Text => Some(Engine::Office),
                Xlsx if family == OfficeFamily::Sheet => Some(Engine::Office),
                Pptx if family == OfficeFamily::Slides => Some(Engine::Office),
                _ => None,
            }
        }
        (Kind::Subtitle, Kind::Subtitle) => Some(Engine::Native),
        (Kind::Subtitle, Kind::Document) if to == Txt => Some(Engine::Native),
        (Kind::Video, Kind::Video) | (Kind::Video, Kind::Audio) | (Kind::Audio, Kind::Audio) => {
            Some(Engine::Ffmpeg)
        }
        (Kind::Video, Kind::Image) if to == Gif => Some(Engine::Ffmpeg),
        _ => None,
    }
}

/// Wheel ordering: the most commonly wanted targets first.
const PRIORITY: &[Format] = &[
    Format::Pdf,
    Format::Jpeg,
    Format::Png,
    Format::Docx,
    Format::Webp,
    Format::Mp4,
    Format::Mp3,
    Format::Gif,
    Format::Txt,
    Format::Avif,
    Format::Xlsx,
    Format::Pptx,
    Format::Srt,
    Format::Vtt,
    Format::M4a,
    Format::Wav,
    Format::Webm,
    Format::Mov,
    Format::Tiff,
    Format::Bmp,
    Format::Ico,
    Format::Flac,
    Format::Ogg,
    Format::Aac,
    Format::Mkv,
    Format::Avi,
];

/// Targets offered for a selection of input files.
///
/// A target is offered when every input can reach it (files already in that
/// format are skipped, as long as at least one file actually changes) and
/// every engine involved is installed.
pub fn targets_for(inputs: &[Format], available: Available) -> Vec<(Format, Engine)> {
    if inputs.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::new();
    for &to in PRIORITY {
        let mut engine = Engine::Native;
        let mut any_change = false;
        let mut ok = true;
        for &from in inputs {
            if from == to {
                continue;
            }
            match route(from, to) {
                Some(e) if available.has(e) => {
                    any_change = true;
                    if e != Engine::Native {
                        engine = e;
                    }
                }
                _ => {
                    ok = false;
                    break;
                }
            }
        }
        if ok && any_change {
            out.push((to, engine));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use Format::*;

    fn targets(inputs: &[Format], a: Available) -> Vec<Format> {
        targets_for(inputs, a).into_iter().map(|t| t.0).collect()
    }

    #[test]
    fn image_routes() {
        assert_eq!(route(Png, Jpeg), Some(Engine::Native));
        assert_eq!(route(Png, Pdf), Some(Engine::Native));
        assert_eq!(route(Heic, Jpeg), Some(Engine::Ffmpeg));
        assert_eq!(route(Png, Avif), Some(Engine::Ffmpeg));
        assert_eq!(route(Png, Heic), None);
        assert_eq!(route(Png, Png), None);
        assert_eq!(route(Png, Mp3), None);
    }

    #[test]
    fn document_routes() {
        assert_eq!(route(Pdf, Png), Some(Engine::Native));
        assert_eq!(route(Pdf, Docx), Some(Engine::Native));
        assert_eq!(route(Docx, Pdf), Some(Engine::Office));
        assert_eq!(route(Docx, Txt), Some(Engine::Native));
        assert_eq!(route(Doc, Docx), Some(Engine::Office));
        assert_eq!(route(Xls, Xlsx), Some(Engine::Office));
        assert_eq!(route(Xlsx, Docx), None);
    }

    #[test]
    fn pdf_offers_images_and_word() {
        let t = targets(&[Pdf], Available::NONE);
        assert_eq!(t[..3], [Jpeg, Png, Docx]);
        assert!(t.contains(&Txt));
    }

    #[test]
    fn images_offer_pdf_first() {
        let t = targets(&[Png, Jpeg], Available::NONE);
        assert_eq!(t[0], Pdf);
        assert!(
            t.contains(&Jpeg),
            "PNG+JPEG -> JPEG converts the PNG, skips the JPEG"
        );
        assert!(!t.contains(&Avif), "AVIF needs FFmpeg");
    }

    #[test]
    fn helper_routes_hidden_when_missing() {
        assert!(targets(&[Mp4], Available::NONE).is_empty());
        let t = targets(
            &[Mp4],
            Available {
                ffmpeg: true,
                office: false,
            },
        );
        assert!(t.contains(&Mp3) && t.contains(&Gif) && !t.contains(&Mp4));
        assert!(targets(&[Docx], Available::NONE) == [Txt]);
        assert_eq!(targets(&[Docx], Available::ALL), [Pdf, Txt]);
    }

    #[test]
    fn incompatible_selection_has_no_targets() {
        assert!(targets_for(&[Png, Mp3], Available::ALL).is_empty());
        assert!(targets_for(&[], Available::ALL).is_empty());
    }

    #[test]
    fn priority_list_covers_every_output() {
        for &from in Format::ALL {
            for &to in Format::ALL {
                if route(from, to).is_some() {
                    assert!(PRIORITY.contains(&to), "{to:?} missing from PRIORITY");
                }
            }
        }
    }
}
