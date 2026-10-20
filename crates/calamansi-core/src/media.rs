//! Audio, video and extra image codecs via an external FFmpeg.
//!
//! Calamansi never downloads FFmpeg on its own. It looks for an existing
//! install (PATH, winget, Scoop, Chocolatey or a path set in Settings) and
//! checks which encoders that build actually has.

use crate::error::{CoreError, Result};
use crate::format::{Format, Kind};
use std::ffi::OsString;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

#[derive(Clone, Debug)]
pub struct Ffmpeg {
    pub path: PathBuf,
    pub version: String,
    encoders: String,
}

const EXE: &str = if cfg!(windows) {
    "ffmpeg.exe"
} else {
    "ffmpeg"
};

/// Places FFmpeg is commonly installed, in search order.
pub fn candidate_paths(custom: Option<&Path>) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Some(c) = custom {
        out.push(if c.is_dir() {
            c.join(EXE)
        } else {
            c.to_path_buf()
        });
    }
    if let Some(path) = std::env::var_os("PATH") {
        out.extend(std::env::split_paths(&path).map(|d| d.join(EXE)));
    }
    if cfg!(windows) {
        let env = |k: &str| std::env::var_os(k).map(PathBuf::from);
        if let Some(local) = env("LOCALAPPDATA") {
            out.push(local.join(r"Microsoft\WinGet\Links\ffmpeg.exe"));
        }
        if let Some(home) = env("USERPROFILE") {
            out.push(home.join(r"scoop\shims\ffmpeg.exe"));
        }
        if let Some(pd) = env("ProgramData") {
            out.push(pd.join(r"chocolatey\bin\ffmpeg.exe"));
        }
        if let Some(pf) = env("ProgramFiles") {
            out.push(pf.join(r"ffmpeg\bin\ffmpeg.exe"));
        }
        out.push(PathBuf::from(r"C:\ffmpeg\bin\ffmpeg.exe"));
    }
    out
}

fn command(bin: &Path) -> Command {
    #[allow(unused_mut)]
    let mut cmd = Command::new(bin);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd.stdin(Stdio::null());
    cmd
}

impl Ffmpeg {
    /// Find a working FFmpeg, or `None`.
    pub fn locate(custom: Option<&Path>) -> Option<Ffmpeg> {
        candidate_paths(custom)
            .into_iter()
            .filter(|p| p.is_file())
            .find_map(|p| Ffmpeg::probe(&p).ok())
    }

    pub fn probe(path: &Path) -> Result<Ffmpeg> {
        let out = command(path).args(["-hide_banner", "-version"]).output()?;
        if !out.status.success() {
            return Err(CoreError::Ffmpeg("couldn't run ffmpeg -version".into()));
        }
        let version = String::from_utf8_lossy(&out.stdout)
            .lines()
            .next()
            .unwrap_or("ffmpeg")
            .to_string();
        let enc = command(path).args(["-hide_banner", "-encoders"]).output()?;
        Ok(Ffmpeg {
            path: path.to_path_buf(),
            version,
            encoders: String::from_utf8_lossy(&enc.stdout).into_owned(),
        })
    }

    pub fn has_encoder(&self, name: &str) -> bool {
        self.encoders
            .lines()
            .any(|l| l.split_whitespace().nth(1) == Some(name))
    }

    /// First encoder from `prefs` this build supports.
    fn pick<'a>(&self, prefs: &[&'a str]) -> Option<&'a str> {
        prefs.iter().copied().find(|e| self.has_encoder(e))
    }

    /// FFmpeg arguments for one conversion. `quality` is 1–100.
    pub fn args(
        &self,
        from: Format,
        to: Format,
        input: &Path,
        output: &Path,
        quality: u8,
    ) -> Result<Vec<OsString>> {
        let mut a: Vec<OsString> = ["-hide_banner", "-nostdin", "-loglevel", "error", "-y", "-i"]
            .iter()
            .map(OsString::from)
            .collect();
        a.push(input.as_os_str().to_owned());
        let missing =
            |what: &str| CoreError::Ffmpeg(format!("this FFmpeg build has no {what} encoder"));
        let mut push = |xs: &[&str]| a.extend(xs.iter().map(OsString::from));

        match (from.kind(), to) {
            // ---- Still images (HEIC/AVIF in, AVIF out) ----
            (Kind::Image, Format::Avif) => {
                let enc = self.pick(&["libaom-av1", "libsvtav1", "librav1e"]).ok_or_else(|| missing("AV1"))?;
                let crf = (63 - (u32::from(quality.clamp(1, 100)) * 63 / 100)).max(10).to_string();
                push(&["-frames:v", "1", "-c:v", enc, "-crf", &crf, "-pix_fmt", "yuv420p"]);
                if enc == "libaom-av1" {
                    push(&["-still-picture", "1", "-cpu-used", "6"]);
                }
            }
            (Kind::Image, Format::Jpeg) => {
                let q = (31 - (u32::from(quality.clamp(1, 100)) * 29 / 100)).to_string();
                push(&["-frames:v", "1", "-q:v", &q]);
            }
            (Kind::Image, Format::Webp) => {
                let enc = self.pick(&["libwebp", "libwebp_anim"]).ok_or_else(|| missing("WebP"))?;
                push(&["-frames:v", "1", "-c:v", enc, "-quality", &quality.to_string()]);
            }
            (Kind::Image, Format::Ico) => push(&[
                "-frames:v",
                "1",
                "-vf",
                "scale=w='min(256,iw)':h='min(256,ih)':force_original_aspect_ratio=decrease",
            ]),
            (Kind::Image, Format::Png | Format::Bmp | Format::Tiff | Format::Gif) => {
                push(&["-frames:v", "1"]);
            }
            // ---- Video to animated GIF ----
            (Kind::Video, Format::Gif) => push(&[
                "-vf",
                "fps=12,scale='min(480,iw)':-2:flags=lanczos,split[a][b];[a]palettegen[p];[b][p]paletteuse",
                "-loop",
                "0",
            ]),
            // ---- Video ----
            (Kind::Video, Format::Mp4 | Format::Mov | Format::Mkv) => {
                let v = self.pick(&["libx264", "h264_mf", "mpeg4"]).ok_or_else(|| missing("H.264"))?;
                push(&["-c:v", v]);
                if v == "libx264" {
                    push(&["-preset", "medium", "-crf", "20"]);
                }
                push(&["-pix_fmt", "yuv420p", "-c:a", "aac", "-b:a", "192k"]);
                if to != Format::Mkv {
                    push(&["-movflags", "+faststart"]);
                }
            }
            (Kind::Video, Format::Webm) => {
                let v = self.pick(&["libvpx-vp9", "libvpx"]).ok_or_else(|| missing("VP9"))?;
                let au = self.pick(&["libopus", "libvorbis"]).ok_or_else(|| missing("Opus"))?;
                push(&["-c:v", v, "-b:v", "0", "-crf", "32", "-c:a", au]);
            }
            (Kind::Video, Format::Avi) => {
                let au = self.pick(&["libmp3lame", "ac3"]).unwrap_or("pcm_s16le");
                push(&["-c:v", "mpeg4", "-q:v", "4", "-c:a", au]);
            }
            // ---- Audio (also audio extracted from video) ----
            (Kind::Video | Kind::Audio, Format::Mp3) => {
                let enc = self.pick(&["libmp3lame"]).ok_or_else(|| missing("MP3"))?;
                push(&["-vn", "-c:a", enc, "-q:a", "2"]);
            }
            (Kind::Video | Kind::Audio, Format::Wav) => push(&["-vn", "-c:a", "pcm_s16le"]),
            (Kind::Video | Kind::Audio, Format::Flac) => push(&["-vn", "-c:a", "flac"]),
            (Kind::Video | Kind::Audio, Format::M4a | Format::Aac) => push(&["-vn", "-c:a", "aac", "-b:a", "192k"]),
            (Kind::Video | Kind::Audio, Format::Ogg) => {
                match self.pick(&["libvorbis", "libopus", "vorbis"]).ok_or_else(|| missing("Ogg"))? {
                    "vorbis" => push(&["-vn", "-c:a", "vorbis", "-strict", "-2", "-ac", "2"]),
                    "libopus" => push(&["-vn", "-c:a", "libopus", "-b:a", "128k"]),
                    enc => push(&["-vn", "-c:a", enc, "-q:a", "5"]),
                }
            }
            _ => return Err(CoreError::Unsupported { from, to }),
        }
        a.push(output.as_os_str().to_owned());
        Ok(a)
    }

    /// Run FFmpeg, killing it on cancellation or after `timeout`.
    pub fn run(&self, args: &[OsString], cancel: &AtomicBool, timeout: Duration) -> Result<()> {
        let mut child = command(&self.path)
            .args(args)
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()?;
        let mut stderr = child.stderr.take().expect("piped");
        let reader = std::thread::spawn(move || {
            let mut s = String::new();
            let _ = stderr.read_to_string(&mut s);
            s
        });
        let started = Instant::now();
        let status = loop {
            if let Some(status) = child.try_wait()? {
                break status;
            }
            if cancel.load(Ordering::Relaxed) {
                let _ = child.kill();
                let _ = child.wait();
                return Err(CoreError::Cancelled);
            }
            if started.elapsed() > timeout {
                let _ = child.kill();
                let _ = child.wait();
                return Err(CoreError::Ffmpeg(format!(
                    "timed out after {} minutes",
                    timeout.as_secs() / 60
                )));
            }
            std::thread::sleep(Duration::from_millis(100));
        };
        let log = reader.join().unwrap_or_default();
        if status.success() {
            Ok(())
        } else {
            let tail: Vec<&str> = log
                .lines()
                .filter(|l| !l.trim().is_empty())
                .rev()
                .take(3)
                .collect();
            let msg = tail.into_iter().rev().collect::<Vec<_>>().join(" / ");
            Err(CoreError::Ffmpeg(if msg.is_empty() {
                format!("exit status {status}")
            } else {
                msg
            }))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fake(encoders: &str) -> Ffmpeg {
        Ffmpeg {
            path: "ffmpeg".into(),
            version: "test".into(),
            encoders: encoders.into(),
        }
    }

    const LIST: &str = " V....D libx264              libx264 H.264\n A....D aac                  AAC\n A....D libmp3lame           MP3\n";

    #[test]
    fn picks_available_encoders() {
        let f = fake(LIST);
        assert!(f.has_encoder("libx264") && !f.has_encoder("libvpx-vp9"));
        let a = f
            .args(
                Format::Mov,
                Format::Mp4,
                Path::new("in.mov"),
                Path::new("out.mp4"),
                80,
            )
            .unwrap();
        let s: Vec<String> = a.iter().map(|x| x.to_string_lossy().into_owned()).collect();
        assert!(s.windows(2).any(|w| w == ["-c:v", "libx264"]));
        assert_eq!(s.last().unwrap(), "out.mp4");
        assert!(f
            .args(
                Format::Mp4,
                Format::Webm,
                Path::new("a"),
                Path::new("b"),
                80
            )
            .is_err());
    }

    #[test]
    fn audio_extraction_drops_video() {
        let a = fake(LIST)
            .args(
                Format::Mp4,
                Format::Mp3,
                Path::new("a.mp4"),
                Path::new("a.mp3"),
                80,
            )
            .unwrap();
        assert!(a.iter().any(|x| x == "-vn"));
    }

    #[test]
    fn real_ffmpeg_if_installed() {
        let Some(ff) = Ffmpeg::locate(None) else {
            return;
        };
        let dir = tempfile::tempdir().unwrap();
        let wav = dir.path().join("tone.wav");
        let gen: Vec<OsString> = [
            "-hide_banner",
            "-loglevel",
            "error",
            "-y",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=440:duration=1",
        ]
        .iter()
        .map(OsString::from)
        .chain([wav.clone().into_os_string()])
        .collect();
        ff.run(&gen, &AtomicBool::new(false), Duration::from_secs(60))
            .unwrap();
        let flac = dir.path().join("tone.flac");
        let args = ff.args(Format::Wav, Format::Flac, &wav, &flac, 80).unwrap();
        ff.run(&args, &AtomicBool::new(false), Duration::from_secs(60))
            .unwrap();
        assert_eq!(crate::format::detect(&flac), Some(Format::Flac));
    }
}
