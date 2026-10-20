//! File formats Calamansi understands, and how to recognise them.

use serde::{Deserialize, Serialize};
use std::fmt;
use std::fs::File;
use std::io::Read;
use std::path::Path;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Format {
    // Images
    Png,
    Jpeg,
    Webp,
    Avif,
    Heic,
    Gif,
    Bmp,
    Tiff,
    Ico,
    // Documents
    Pdf,
    Txt,
    // Subtitles
    Srt,
    Vtt,
    // Video
    Mp4,
    Mov,
    Mkv,
    Webm,
    Avi,
    // Audio
    Mp3,
    Wav,
    M4a,
    Aac,
    Flac,
    Ogg,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Kind {
    Image,
    Document,
    Subtitle,
    Video,
    Audio,
}

impl Format {
    pub const ALL: &'static [Format] = &[
        Format::Png,
        Format::Jpeg,
        Format::Webp,
        Format::Avif,
        Format::Heic,
        Format::Gif,
        Format::Bmp,
        Format::Tiff,
        Format::Ico,
        Format::Pdf,
        Format::Txt,
        Format::Srt,
        Format::Vtt,
        Format::Mp4,
        Format::Mov,
        Format::Mkv,
        Format::Webm,
        Format::Avi,
        Format::Mp3,
        Format::Wav,
        Format::M4a,
        Format::Aac,
        Format::Flac,
        Format::Ogg,
    ];

    /// File extension used for outputs (without the dot).
    pub fn extension(self) -> &'static str {
        match self {
            Format::Png => "png",
            Format::Jpeg => "jpg",
            Format::Webp => "webp",
            Format::Avif => "avif",
            Format::Heic => "heic",
            Format::Gif => "gif",
            Format::Bmp => "bmp",
            Format::Tiff => "tiff",
            Format::Ico => "ico",
            Format::Pdf => "pdf",
            Format::Txt => "txt",
            Format::Srt => "srt",
            Format::Vtt => "vtt",
            Format::Mp4 => "mp4",
            Format::Mov => "mov",
            Format::Mkv => "mkv",
            Format::Webm => "webm",
            Format::Avi => "avi",
            Format::Mp3 => "mp3",
            Format::Wav => "wav",
            Format::M4a => "m4a",
            Format::Aac => "aac",
            Format::Flac => "flac",
            Format::Ogg => "ogg",
        }
    }

    /// Short label shown on the wheel.
    pub fn label(self) -> &'static str {
        match self {
            Format::Png => "PNG",
            Format::Jpeg => "JPEG",
            Format::Webp => "WebP",
            Format::Avif => "AVIF",
            Format::Heic => "HEIC",
            Format::Gif => "GIF",
            Format::Bmp => "BMP",
            Format::Tiff => "TIFF",
            Format::Ico => "ICO",
            Format::Pdf => "PDF",
            Format::Txt => "Text",
            Format::Srt => "SRT",
            Format::Vtt => "VTT",
            Format::Mp4 => "MP4",
            Format::Mov => "MOV",
            Format::Mkv => "MKV",
            Format::Webm => "WebM",
            Format::Avi => "AVI",
            Format::Mp3 => "MP3",
            Format::Wav => "WAV",
            Format::M4a => "M4A",
            Format::Aac => "AAC",
            Format::Flac => "FLAC",
            Format::Ogg => "Ogg",
        }
    }

    pub fn kind(self) -> Kind {
        use Format::*;
        match self {
            Png | Jpeg | Webp | Avif | Heic | Gif | Bmp | Tiff | Ico => Kind::Image,
            Pdf | Txt => Kind::Document,
            Srt | Vtt => Kind::Subtitle,
            Mp4 | Mov | Mkv | Webm | Avi => Kind::Video,
            Mp3 | Wav | M4a | Aac | Flac | Ogg => Kind::Audio,
        }
    }

    /// Parse a user-supplied name or extension ("jpg", ".JPEG", "tif").
    pub fn parse(name: &str) -> Option<Format> {
        let n = name.trim().trim_start_matches('.').to_ascii_lowercase();
        Some(match n.as_str() {
            "png" => Format::Png,
            "jpg" | "jpeg" | "jpe" | "jfif" => Format::Jpeg,
            "webp" => Format::Webp,
            "avif" => Format::Avif,
            "heic" | "heif" | "hif" => Format::Heic,
            "gif" => Format::Gif,
            "bmp" | "dib" => Format::Bmp,
            "tif" | "tiff" => Format::Tiff,
            "ico" => Format::Ico,
            "pdf" => Format::Pdf,
            "txt" | "text" | "md" | "log" => Format::Txt,
            "srt" => Format::Srt,
            "vtt" | "webvtt" => Format::Vtt,
            "mp4" | "m4v" => Format::Mp4,
            "mov" | "qt" => Format::Mov,
            "mkv" => Format::Mkv,
            "webm" => Format::Webm,
            "avi" => Format::Avi,
            "mp3" => Format::Mp3,
            "wav" | "wave" => Format::Wav,
            "m4a" => Format::M4a,
            "aac" => Format::Aac,
            "flac" => Format::Flac,
            "ogg" | "oga" | "opus" => Format::Ogg,
            _ => return None,
        })
    }

    pub fn from_extension(path: &Path) -> Option<Format> {
        path.extension()
            .and_then(|e| e.to_str())
            .and_then(Format::parse)
    }
}

impl fmt::Display for Format {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.label())
    }
}

/// Recognise a file by its leading bytes, falling back to its extension.
///
/// Magic bytes win over the extension so a PNG renamed to `.jpg` is still
/// decoded correctly. Text-based formats (TXT, SRT, VTT) have no reliable
/// signature and are identified by extension.
pub fn detect(path: &Path) -> Option<Format> {
    let mut head = [0u8; 32];
    let n = File::open(path)
        .and_then(|mut f| f.read(&mut head))
        .unwrap_or(0);
    detect_bytes(&head[..n]).or_else(|| Format::from_extension(path))
}

pub fn detect_bytes(b: &[u8]) -> Option<Format> {
    if b.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Some(Format::Png);
    }
    if b.starts_with(&[0xFF, 0xD8, 0xFF]) {
        return Some(Format::Jpeg);
    }
    if b.starts_with(b"GIF87a") || b.starts_with(b"GIF89a") {
        return Some(Format::Gif);
    }
    if b.starts_with(b"%PDF-") {
        return Some(Format::Pdf);
    }
    if b.starts_with(b"II*\0") || b.starts_with(b"MM\0*") {
        return Some(Format::Tiff);
    }
    if b.len() >= 12 && &b[0..4] == b"RIFF" {
        match &b[8..12] {
            b"WEBP" => return Some(Format::Webp),
            b"WAVE" => return Some(Format::Wav),
            b"AVI " => return Some(Format::Avi),
            _ => {}
        }
    }
    if b.starts_with(b"BM") && b.len() >= 14 {
        return Some(Format::Bmp);
    }
    if b.starts_with(&[0, 0, 1, 0]) && b.len() >= 6 {
        return Some(Format::Ico);
    }
    if b.starts_with(b"fLaC") {
        return Some(Format::Flac);
    }
    if b.starts_with(b"OggS") {
        return Some(Format::Ogg);
    }
    if b.starts_with(&[0x1A, 0x45, 0xDF, 0xA3]) {
        // Matroska and WebM share the EBML header; the extension decides.
        return None;
    }
    if b.starts_with(b"WEBVTT") {
        return Some(Format::Vtt);
    }
    if b.len() >= 12 && &b[4..8] == b"ftyp" {
        return match &b[8..12] {
            b"heic" | b"heix" | b"heim" | b"heis" | b"hevc" | b"hevx" | b"mif1" | b"msf1" => {
                Some(Format::Heic)
            }
            b"avif" | b"avis" => Some(Format::Avif),
            b"qt  " => Some(Format::Mov),
            b"M4A " | b"M4B " => Some(Format::M4a),
            _ => Some(Format::Mp4),
        };
    }
    if b.starts_with(b"ID3") || (b.len() >= 2 && b[0] == 0xFF && (b[1] & 0xE6) == 0xE2) {
        return Some(Format::Mp3);
    }
    if b.len() >= 2 && b[0] == 0xFF && (b[1] & 0xF6) == 0xF0 {
        return Some(Format::Aac);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_aliases() {
        assert_eq!(Format::parse("JPG"), Some(Format::Jpeg));
        assert_eq!(Format::parse(".tif"), Some(Format::Tiff));
        assert_eq!(Format::parse("heif"), Some(Format::Heic));
        assert_eq!(Format::parse("docx"), None);
    }

    #[test]
    fn magic_bytes_win_over_extension() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("actually-png.jpg");
        std::fs::write(&p, b"\x89PNG\r\n\x1a\n0000000000000000").unwrap();
        assert_eq!(detect(&p), Some(Format::Png));
    }

    #[test]
    fn signatures() {
        assert_eq!(detect_bytes(b"RIFF\0\0\0\0WEBPVP8 "), Some(Format::Webp));
        assert_eq!(
            detect_bytes(b"\0\0\0\x18ftypheic\0\0\0\0"),
            Some(Format::Heic)
        );
        assert_eq!(
            detect_bytes(b"\0\0\0\x18ftypavif\0\0\0\0"),
            Some(Format::Avif)
        );
        assert_eq!(
            detect_bytes(b"\0\0\0\x18ftypisom\0\0\0\0"),
            Some(Format::Mp4)
        );
        assert_eq!(detect_bytes(b"%PDF-1.7"), Some(Format::Pdf));
        assert_eq!(detect_bytes(b"hello"), None);
    }

    #[test]
    fn every_format_round_trips_through_its_extension() {
        for f in Format::ALL {
            assert_eq!(Format::parse(f.extension()), Some(*f), "{f:?}");
        }
    }
}
