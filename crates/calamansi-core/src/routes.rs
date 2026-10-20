//! Which conversions exist, and which engine performs each one.

use crate::format::{Format, Kind};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Engine {
    /// Pure Rust, always available.
    Native,
    /// Requires an FFmpeg executable on the machine.
    Ffmpeg,
}

/// Images the built-in decoder can read.
fn native_image_in(f: Format) -> bool {
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
        (Kind::Image, Kind::Document) if to == Pdf => {
            if native_image_in(from) {
                Some(Engine::Native)
            } else {
                None
            }
        }
        (Kind::Document, _) => match (from, to) {
            (Pdf, Txt) | (Txt, Pdf) => Some(Engine::Native),
            _ => None,
        },
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
    Format::Jpeg,
    Format::Png,
    Format::Webp,
    Format::Pdf,
    Format::Mp4,
    Format::Mp3,
    Format::Gif,
    Format::Avif,
    Format::Txt,
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
/// format are skipped, as long as at least one file actually changes).
/// FFmpeg routes are left out when FFmpeg isn't installed.
pub fn targets_for(inputs: &[Format], ffmpeg_available: bool) -> Vec<(Format, Engine)> {
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
                Some(e) => {
                    any_change = true;
                    if e == Engine::Ffmpeg {
                        engine = Engine::Ffmpeg;
                    }
                }
                None => {
                    ok = false;
                    break;
                }
            }
        }
        if ok && any_change && (engine == Engine::Native || ffmpeg_available) {
            out.push((to, engine));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use Format::*;

    #[test]
    fn image_routes() {
        assert_eq!(route(Png, Jpeg), Some(Engine::Native));
        assert_eq!(route(Heic, Jpeg), Some(Engine::Ffmpeg));
        assert_eq!(route(Png, Avif), Some(Engine::Ffmpeg));
        assert_eq!(route(Png, Heic), None);
        assert_eq!(route(Png, Png), None);
        assert_eq!(route(Png, Mp3), None);
    }

    #[test]
    fn mixed_selection_offers_shared_targets() {
        let t: Vec<Format> = targets_for(&[Png, Jpeg], false)
            .into_iter()
            .map(|t| t.0)
            .collect();
        assert!(
            t.contains(&Jpeg),
            "PNG+JPEG -> JPEG converts the PNG, skips the JPEG"
        );
        assert!(t.contains(&Pdf));
        assert!(!t.contains(&Avif), "AVIF needs FFmpeg");
        assert_eq!(t[0], Jpeg, "priority order");
    }

    #[test]
    fn ffmpeg_routes_hidden_without_ffmpeg() {
        assert!(targets_for(&[Mp4], false).is_empty());
        let t: Vec<Format> = targets_for(&[Mp4], true).into_iter().map(|t| t.0).collect();
        assert!(t.contains(&Mp3) && t.contains(&Gif) && !t.contains(&Mp4));
    }

    #[test]
    fn incompatible_selection_has_no_targets() {
        assert!(targets_for(&[Png, Mp3], true).is_empty());
        assert!(targets_for(&[], true).is_empty());
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
