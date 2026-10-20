# Calamansi

**A local file converter for Windows. Hold Shift while dragging files, drop them on a format, done.**

Calamansi lives in the system tray. Drag any selection of files with **Shift** held and a
conversion wheel appears under your cursor; drop on JPEG, PDF, MP3… and each file gets its own
output next to the original. Hold **Ctrl+Shift** instead for tools: make a PDF from images, merge or
split PDFs, resize, rotate, copy SHA-256 checksums.

No account, no upload, no subscription, no telemetry. Files never leave your PC.

> **Status: 0.1 developer preview.** The conversion engine is tested; the Windows shell is new and
> needs testing on real machines — see [docs/BUILD-STATUS.md](docs/BUILD-STATUS.md). Calamansi is an
> independent project inspired by the macOS app
> [Open Tangerine](https://github.com/sintaxerrorpvtltd/Open-Tangerine). It shares no code with it and
> is not affiliated with it or with Tangerine.

## Ways to use it

| How | What happens |
| --- | --- |
| Shift + drag files | Convert wheel appears under the cursor. Drop on a format. Drop in the centre to cancel. |
| Ctrl + Shift + drag | Tools wheel: Make PDF, Collage, Merge, Split, Resize, Compress, Rotate, Flip, Grayscale, Mute, Snapshot, Copy path, SHA-256… (only the tools that fit the selection). |
| Right-click → Send to → Calamansi | Wheel opens at the cursor; click a format. (Turn on in the tray menu.) |
| Click the tray icon | Pick files, then pick a format. |
| Tray → Run self-test | Converts sample files and reports which features work on this PC. |
| `calamansi-cli` | The same engine on the command line, for scripts. |

Holding Shift normally means "move" in Explorer. Calamansi always answers a drop with *copy*, so your
originals are never moved or deleted, and outputs never overwrite existing files (`photo (2).jpg`).

## What it converts

| Area | Built in | With FFmpeg | With Microsoft Office or LibreOffice |
| --- | --- | --- | --- |
| Images | PNG, JPEG, WebP (lossless), GIF, BMP, TIFF, ICO; image → PDF; resize, compress, rotate, flip, grayscale, remove metadata, collage, many images → one PDF | HEIC/AVIF input, AVIF output, lossy WebP | |
| PDF | PDF → PNG/JPEG/WebP/TIFF (one image per page), PDF → Word, PDF → text, text → PDF; merge, split, rotate, reverse, compress | | PDF → Word through Word itself (keeps layout) |
| Word, Excel, PowerPoint | Word → text | | DOCX/DOC/ODT/RTF, XLSX/XLS/ODS, PPTX/PPT/ODP → PDF; old formats → DOCX/XLSX/PPTX |
| Subtitles | SRT ⇄ VTT, subtitles → text | | |
| Video | | MP4, MOV, MKV, WebM, AVI, → GIF, extract audio; compress, mute, rotate, snapshot | |
| Audio | | MP3, WAV, M4A, AAC, FLAC, Ogg; normalize loudness, mono | |
| Any file | Duplicate, copy path, SHA-256 | | |

The wheel only offers what works for *every* file in the selection and hides routes whose helper
program isn't installed. Calamansi never downloads helpers itself. To add them:

```powershell
winget install ffmpeg                      # audio and video
winget install TheDocumentFoundation.LibreOffice   # only if you don't have Microsoft Office
```

## Install

No installer, no Rust, nothing else to download:

1. Download `Calamansi-…-windows-x64.zip` from [Releases](../../releases) and unzip it somewhere
   permanent (for example `%LOCALAPPDATA%\Programs\Calamansi`).
2. Run `Calamansi.exe`. It lives in the system tray.
3. Shift-drag some files. Right-click the tray icon → **Run self-test** to check every conversion on
   your PC.

The zip contains a single self-contained `Calamansi.exe` (no Visual C++ runtime or other DLLs
needed), `calamansi-cli.exe`, and `START HERE.txt`. FFmpeg (audio/video) and Microsoft Office or
LibreOffice (Word/Excel/PowerPoint) are optional and used only if already installed.

Early builds aren't code-signed yet, so SmartScreen may warn. Choose **More info → Run anyway** only if
you downloaded the zip from this repository's Releases page and its SHA-256 matches `SHA256SUMS.txt`.

## Publishing a release (maintainers)

```powershell
powershell -ExecutionPolicy Bypass -File scripts\release.ps1 v0.1.0
```

This builds the self-contained executables, packages the zip with `START HERE.txt` and SHA-256
checksums, pushes the tag and publishes the GitHub release (needs `gh auth login` once).

## Build from source

Requirements: Windows 10 1703+ or Windows 11, [Rust](https://rustup.rs) (stable, MSVC toolchain).

```powershell
git clone https://github.com/heet579/calamansi
cd calamansi
cargo test --workspace
cargo build --release
.\target\release\Calamansi.exe
```

The engine and CLI also build and test on Linux and macOS (`cargo test -p calamansi-core -p calamansi-cli`;
a plain `cargo build` there fails because the Windows app only links on Windows),
and the Windows app type-checks there (`cargo clippy -p calamansi-win`).

## Settings

The tray menu covers the common options. Everything else is in
`%APPDATA%\Calamansi\settings.json` (tray → *Edit settings file…*):

```json
{
  "output": { "kind": "beside_source" },
  "quality": 85,
  "resize_max_side": 1920,
  "drag_wheel": true,
  "excluded_apps": ["CorelDRW.exe"],
  "ffmpeg_path": null,
  "office_path": null,
  "pdf_image_dpi": 150,
  "success_dismiss_ms": 2000,
  "ffmpeg_timeout_minutes": 60
}
```

`output.kind` can be `beside_source`, `downloads`, `desktop`, or `folder` with `"path": "D:\\Converted"`.
Shift-drags that start in an `excluded_apps` program are ignored. Errors are logged to
`%LOCALAPPDATA%\Calamansi\calamansi.log`.

## How it works

```
crates/
  calamansi-core/   conversion engine: formats, routes, images, PDF, subtitles, FFmpeg, batch jobs
  calamansi-cli/    command-line front end
  calamansi-win/    tray app: mouse hook, drop-target wheel, progress pill (Win32 via windows-sys)
```

See [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) for how the Shift-drag wheel works without a global
drag API on Windows.

## Limitations

- No HEIC *output*, no OCR, no background removal yet.
- The built-in PDF → Word keeps the text but not the layout; with Microsoft Word installed, Word does
  the conversion and keeps the layout.
- Built-in WebP output is lossless (larger files); lossy WebP needs FFmpeg with libwebp.
- Images are limited to 100 MB / 40 megapixels; animated images are converted as a single frame.
- Text → PDF uses a standard PDF font, so characters outside Latin-1 become `?`.
- PDF → text is best-effort; scanned PDFs contain no text to extract.
- Password-protected PDFs aren't supported.
- One job runs at a time.

## Contributing

Contributions are welcome — new formats, translations, testing on different Windows setups. Start with
[CONTRIBUTING.md](CONTRIBUTING.md) and issues labelled `good first issue`.

## License

Calamansi is free software under the **GNU General Public License v3.0 only** ([LICENSE](LICENSE)).
You may use, study, share and modify it; distributing modified versions requires sharing their source
under the same license. Third-party components are listed in [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md).
