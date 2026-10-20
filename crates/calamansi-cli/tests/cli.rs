use std::path::Path;
use std::process::Command;

fn calamansi(out: &Path) -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_calamansi-cli"));
    c.arg("--out").arg(out);
    c
}

#[test]
fn convert_targets_and_tools() {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("out");
    let png = dir.path().join("pic.png");
    image::RgbaImage::from_pixel(64, 64, image::Rgba([10, 200, 10, 255]))
        .save(&png)
        .unwrap();

    let t = calamansi(&out).arg("targets").arg(&png).output().unwrap();
    assert!(t.status.success());
    assert!(String::from_utf8_lossy(&t.stdout).starts_with("PDF"));

    // Image -> PDF -> PNG pages, and a tool.
    let c = calamansi(&out)
        .args(["convert", "--to", "pdf"])
        .arg(&png)
        .output()
        .unwrap();
    assert!(c.status.success(), "{}", String::from_utf8_lossy(&c.stderr));
    let pdf = out.join("pic.pdf");
    assert!(pdf.is_file());
    let c = calamansi(&out)
        .args(["convert", "--to", "png"])
        .arg(&pdf)
        .output()
        .unwrap();
    assert!(c.status.success(), "{}", String::from_utf8_lossy(&c.stderr));
    let c = calamansi(&out)
        .args(["tool", "rotate-pdf"])
        .arg(&pdf)
        .output()
        .unwrap();
    assert!(c.status.success(), "{}", String::from_utf8_lossy(&c.stderr));
    assert!(out.join("pic rotated.pdf").is_file());

    let c = calamansi(&out)
        .args(["convert", "--to", "jpg"])
        .arg(&png)
        .output()
        .unwrap();
    assert!(c.status.success(), "{}", String::from_utf8_lossy(&c.stderr));
    assert!(out.join("pic.jpg").is_file());

    // Converting again never overwrites.
    calamansi(&out)
        .args(["convert", "--to", "jpg"])
        .arg(&png)
        .status()
        .unwrap();
    assert!(out.join("pic (2).jpg").is_file());

    let s = calamansi(&out)
        .args(["tool", "sha256"])
        .arg(&png)
        .output()
        .unwrap();
    assert_eq!(
        String::from_utf8_lossy(&s.stdout)
            .split_whitespace()
            .next()
            .unwrap()
            .len(),
        64
    );

    let bad = calamansi(&out)
        .args(["convert", "--to", "docx"])
        .arg(&png)
        .output()
        .unwrap();
    assert!(!bad.status.success());
}

#[test]
fn video_with_ffmpeg_if_installed() {
    let doctor = Command::new(env!("CARGO_BIN_EXE_calamansi-cli"))
        .arg("doctor")
        .output()
        .unwrap();
    if String::from_utf8_lossy(&doctor.stdout).contains("not found") {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let mp4 = dir.path().join("clip.mp4");
    let ok = Command::new("ffmpeg")
        .args([
            "-loglevel",
            "error",
            "-y",
            "-f",
            "lavfi",
            "-i",
            "testsrc=duration=1:size=160x120:rate=10",
            "-f",
            "lavfi",
            "-i",
            "sine=duration=1",
            "-shortest",
            "-pix_fmt",
            "yuv420p",
        ])
        .arg(&mp4)
        .status()
        .unwrap();
    assert!(ok.success());
    for (to, ext) in [("gif", "gif"), ("wav", "wav"), ("webm", "webm")] {
        let r = calamansi(dir.path())
            .args(["convert", "--to", to])
            .arg(&mp4)
            .output()
            .unwrap();
        let err = String::from_utf8_lossy(&r.stderr);
        if err.contains("has no") {
            continue; // this FFmpeg build lacks the encoder; that's reported, not a crash
        }
        assert!(r.status.success(), "{to}: {err}");
        assert!(dir.path().join(format!("clip.{ext}")).is_file(), "{to}");
    }
}
