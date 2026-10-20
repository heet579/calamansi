# Roadmap

Ordered roughly by value to users. Items here are plans, not current features.

## 0.2 — make the preview solid
- Work through the manual checklist on Windows 10 and 11; fix what breaks
- Settings window (instead of editing JSON), built with the same layered-window toolkit or Tauri
- App icon embedded in `Calamansi.exe` (shows in Explorer and the Send to menu)
- Signed releases via SignPath Foundation; `winget` manifest; Microsoft Store (MSIX) listing
- Bundled license texts generated with `cargo about`

## 0.3 — breadth
- HEIC/AVIF decoding through Windows Imaging Component (no FFmpeg needed when the Microsoft HEIF
  extension is installed)
- PDF → images via PDFium
- Lossy WebP without FFmpeg
- Video tools: trim, mute, change speed, take snapshot
- Several jobs at once, with a queue in the pill
- Windows 11 modern context-menu entry (`IExplorerCommand`, needs MSIX packaging)

## Later
- Run native conversions in a sandboxed worker process
- Translations
- Linux and macOS shells over the same engine
- Optional, opt-in, bring-your-own-key AI features (never on by default)
