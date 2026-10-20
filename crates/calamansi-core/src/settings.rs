//! User settings, stored as JSON. Unknown or missing fields fall back to
//! defaults so older settings files keep working.

use crate::error::Result;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind", content = "path")]
pub enum OutputLocation {
    /// Next to each input file.
    BesideSource,
    Downloads,
    Desktop,
    Folder(PathBuf),
}

impl OutputLocation {
    /// Folder an output for `input` should go to.
    pub fn resolve(&self, input: &Path) -> PathBuf {
        let beside = || {
            input
                .parent()
                .map(Path::to_path_buf)
                .unwrap_or_else(|| PathBuf::from("."))
        };
        match self {
            OutputLocation::BesideSource => beside(),
            OutputLocation::Downloads => dirs::download_dir().unwrap_or_else(beside),
            OutputLocation::Desktop => dirs::desktop_dir().unwrap_or_else(beside),
            OutputLocation::Folder(p) => p.clone(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub output: OutputLocation,
    /// JPEG / AVIF / WebP (FFmpeg) quality, 1–100.
    pub quality: u8,
    /// Longest side used by the Resize tool.
    pub resize_max_side: u32,
    /// Show the wheel when dragging files with Shift held.
    pub drag_wheel: bool,
    /// Executable names (e.g. "CorelDRW.exe") where Shift-drag is ignored.
    pub excluded_apps: Vec<String>,
    /// Explicit FFmpeg location; otherwise common install paths are searched.
    pub ffmpeg_path: Option<PathBuf>,
    /// Explicit LibreOffice `soffice` location; otherwise Microsoft Office or
    /// LibreOffice is found automatically.
    pub office_path: Option<PathBuf>,
    /// Resolution for PDF pages turned into images.
    pub pdf_image_dpi: u32,
    /// How long a successful result stays on screen.
    pub success_dismiss_ms: u32,
    /// Maximum minutes an FFmpeg job may run before it's stopped.
    pub ffmpeg_timeout_minutes: u32,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            output: OutputLocation::BesideSource,
            quality: 85,
            resize_max_side: 1920,
            drag_wheel: true,
            excluded_apps: Vec::new(),
            ffmpeg_path: None,
            office_path: None,
            pdf_image_dpi: 150,
            success_dismiss_ms: 2000,
            ffmpeg_timeout_minutes: 60,
        }
    }
}

impl Settings {
    /// `%APPDATA%\Calamansi\settings.json` on Windows.
    pub fn default_path() -> Option<PathBuf> {
        dirs::config_dir().map(|d| d.join("Calamansi").join("settings.json"))
    }

    /// Load settings; a missing or unreadable file yields defaults.
    pub fn load(path: &Path) -> Settings {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|s| serde_json::from_str::<Settings>(&s).ok())
            .map(Settings::sanitized)
            .unwrap_or_default()
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let json = serde_json::to_string_pretty(self).expect("settings serialise");
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, json)?;
        std::fs::rename(&tmp, path)?;
        Ok(())
    }

    fn sanitized(mut self) -> Settings {
        self.quality = self.quality.clamp(1, 100);
        self.resize_max_side = self.resize_max_side.clamp(16, 16_384);
        self.ffmpeg_timeout_minutes = self.ffmpeg_timeout_minutes.clamp(1, 24 * 60);
        self.pdf_image_dpi = self.pdf_image_dpi.clamp(36, 600);
        self
    }

    pub fn is_excluded(&self, exe_name: &str) -> bool {
        self.excluded_apps
            .iter()
            .any(|e| e.eq_ignore_ascii_case(exe_name))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_and_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("sub").join("settings.json");
        assert_eq!(Settings::load(&p), Settings::default());
        let s = Settings {
            output: OutputLocation::Folder(dir.path().into()),
            excluded_apps: vec!["CorelDRW.exe".into()],
            ..Default::default()
        };
        s.save(&p).unwrap();
        assert_eq!(Settings::load(&p), s);
        assert!(s.is_excluded("coreldrw.EXE"));
    }

    #[test]
    fn partial_and_out_of_range_files_are_repaired() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("settings.json");
        std::fs::write(&p, r#"{"quality": 0, "output": {"kind": "downloads"}}"#).unwrap();
        let s = Settings::load(&p);
        assert_eq!(s.quality, 1);
        assert_eq!(s.output, OutputLocation::Downloads);
        assert!(s.drag_wheel);
        std::fs::write(&p, "not json").unwrap();
        assert_eq!(Settings::load(&p), Settings::default());
    }

    #[test]
    fn beside_source() {
        let p = Path::new("/photos/a.png");
        assert_eq!(
            OutputLocation::BesideSource.resolve(p),
            Path::new("/photos")
        );
    }
}
