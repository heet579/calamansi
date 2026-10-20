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
| Ctrl + Shift + drag | Tools wheel (Make PDF, Merge, Split, Resize, Rotate, SHA-256). |
| Right-click → Send to → Calamansi | Wheel opens at the cursor; click a format. (Turn on in the tray menu.) |
| Click the tray icon | Pick files, then pick a format. |
| `calamansi-cli` | The same engine on the command line, for scripts. |

Holding Shift normally means "move" in Explorer. Calamansi always answers a drop with *copy*, so your
originals are never moved or deleted, and outputs never overwrite existing files (`photo (2).jpg`).

## What it converts

| Area | Built in | With FFmpeg installed |
| --- | --- | --- |
| Images | PNG, JPEG, WebP (lossless), GIF, BMP, TIFF, ICO, any image → PDF | HEIC/AVIF input, AVIF output, lossy WebP |
| Documents | PDF → text, text → PDF, merge, split, images → PDF | |
| Subtitles | SRT ⇄ VTT, subtitles → text | |
| Video | | MP4, MOV, MKV, WebM, AVI, video → GIF, extract audio |
| Audio | | MP3, WAV, M4A, AAC, FLAC, Ogg |

The wheel only offers routes that work for *every* file in the selection, and hides FFmpeg routes
when FFmpeg isn't installed. Calamansi never downloads FFmpeg itself. To add audio and video:

```powershell
winget install ffmpeg
```

## Install

Download the latest zip from [Releases](../../releases), unzip anywhere (for example
`%LOCALAPPDATA%\Programs\Calamansi`) and run `Calamansi.exe`. Use the tray menu to start it with
Windows and to add it to the **Send to** menu.

Early builds aren't code-signed yet, so SmartScreen may warn. Choose **More info → Run anyway** only if
you downloaded the zip from this repository's Releases page and its SHA-256 matches `SHA256SUMS.txt`.

## Build from source

Requirements: Windows 10 1703+ or Windows 11, [Rust](https://rustup.rs) (stable, MSVC toolchain).

```powershell
git clone https://github.com/YOUR-USERNAME/calamansi
cd calamansi
cargo test --workspace
cargo build --release
.\target\release\Calamansi.exe
```

The engine and CLI also build and test on Linux and macOS (`cargo test -p calamansi-core -p calamansi-cli`),
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

- No HEIC *output*, no OCR, no Office document conversion, no background removal yet.
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
