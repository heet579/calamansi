# Build status and test checklist

## Automated

| Part | Status |
| --- | --- |
| `calamansi-core` | 58 unit tests: formats, routes, naming races, images and image tools, PDF create/merge/split/rotate/reverse/compress/render/text, Word read/write, subtitles, checksum, settings, batch isolation and cancellation, wheel layout, self-test; real FFmpeg and LibreOffice conversions when installed |
| `calamansi-cli` | Integration tests: convert, targets, no-overwrite naming, checksums, video → GIF/WAV/WebM via FFmpeg |
| `calamansi-win` | Type-checked and linted (clippy, `-D warnings`). Unit tests for packing, parsing and the icon run on Windows CI |

## Not yet verified on real hardware

The Windows shell was written and type-checked before its first run on a Windows PC. Until someone
works through the checklist below, treat 0.1 as a preview.

### Manual checklist

Record Windows version, display scaling (100 %, 150 %, mixed monitors) and results in the PR or issue.

**Self-test (do this first)**
- [ ] Tray → *Run self-test* → "All N checks passed". If not, the log lists each failing check. This
      separates engine problems from drag-and-drop problems.

**Startup**
- [ ] First launch shows the "Calamansi is in your system tray" pill and creates `%APPDATA%\Calamansi\settings.json`
- [ ] Tray icon appears with tooltip; survives restarting Explorer (Task Manager → Restart)
- [ ] Launching a second copy shows "already running" and doesn't add a second icon

**Shift-drag wheel (Explorer)**
- [ ] Select 4 PNGs, drag 10+ px, hold Shift → wheel appears under the cursor
- [ ] Hovering segments highlights them; centre shows the hovered format
- [ ] Drop on JPEG → 4 JPEGs beside the originals, pill shows progress then "Converted 4 files to JPEG"
- [ ] Originals are still there (not moved)
- [ ] Converting again creates `name (2).jpg`, never overwrites
- [ ] Drop in the centre → nothing happens
- [ ] Moving out of the wheel and dropping in another folder performs a normal Shift-move
- [ ] Ctrl+Shift-drag 2 PDFs → tools wheel → Merge works
- [ ] Shift-drag on the desktop works too
- [ ] Shift-drawing a line in Paint shows **nothing**
- [ ] Shift-selecting text in a browser shows **nothing**
- [ ] With `"excluded_apps": ["mspaint.exe"]`, drags from Paint are ignored

**Other entry points**
- [ ] Tray left-click → file dialog → wheel at cursor → click a format works; Esc and clicking outside cancel
- [ ] Tray menu → "Show in Send to menu" → right-click files → Send to → Calamansi opens the wheel
- [ ] Send to with Calamansi *not* running starts it and opens the wheel
- [ ] "Start with Windows" survives sign-out/sign-in

**Pill**
- [ ] Success hides after ~2 s; clicking it opens Explorer with the output selected
- [ ] A failure (e.g. a renamed `.txt` as `.png`) stays until clicked and names the file
- [ ] Clicking during a long FFmpeg job cancels it and leaves no partial file
- [ ] SHA-256 tool copies `hash  filename` lines to the clipboard

**Office**
- [ ] With Microsoft Office: Word/Excel/PowerPoint files offer PDF; conversion runs without any Office window appearing
- [ ] PDF → Word with Word installed keeps the layout

**FFmpeg**
- [ ] Without FFmpeg: video files show "Nothing to do"; tray says "FFmpeg not found"
- [ ] After `winget install ffmpeg`, reopening the tray menu finds it without restarting
- [ ] MP4 → MP3, MOV → MP4, MP4 → GIF work

**Display scaling**
- [ ] Wheel and pill are crisp and correctly sized at 100 %, 150 % and 200 %
- [ ] Moving between monitors with different scaling sizes the wheel for the monitor under the cursor
