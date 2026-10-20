# Architecture

```
┌──────────────── calamansi-win (Windows only) ───────────────┐
│ hook.rs        low-level mouse hook: "Shift held + dragging" │
│ droptarget.rs  hand-written COM IDropTarget on the wheel     │
│ app.rs         tray, wheel window, progress pill, jobs       │
│ layered.rs     per-pixel-alpha windows + GDI text            │
│ integration.rs Start with Windows, "Send to" shortcut        │
└───────────────┬──────────────────────────────────────────────┘
                │ batch::run(action, files) on a worker thread
┌───────────────▼──────────── calamansi-core (any OS) ─────────┐
│ format   detect by magic bytes, then extension               │
│ routes   which conversions exist and which engine does them  │
│ wheel    menu items, layout, hit-testing, shape rendering    │
│ batch    per-file results, parallel native work, cancel      │
│ image_ops · pdf_ops · subtitles · checksum · media (FFmpeg)  │
│ naming   exclusive output reservation, atomic writes         │
│ settings JSON settings with safe defaults                    │
└──────────────────────────────────────────────────────────────┘
```

## The Shift-drag wheel

macOS lets an app observe drags system-wide. Windows doesn't, so Calamansi combines three pieces:

1. **Detect.** A `WH_MOUSE_LL` hook (`hook.rs`) notices left-button-down, movement past 10 px and
   Shift held. The hook only compares numbers and posts `WM_APP_DRAG_START`; anything slower would lag
   the mouse for the whole system.
2. **Probe.** The app shows the wheel window under the cursor with every pixel at alpha 1/255:
   invisible, but layered windows hit-test any non-zero alpha, so OLE drag-and-drop treats it as a
   drop target.
3. **Confirm.** If the drag carries files, OLE calls `DragEnter` with an `IDataObject`; Calamansi reads
   `CF_HDROP`, works out the wheel items, and only now draws the visible wheel. If the drag isn't a
   file drag (Shift-drawing a straight line in Paint, selecting text) there's no `DragEnter`, nothing
   appears, and the window hides on button-up.

`Drop` re-reads the files from the data object rather than trusting earlier state, picks the segment
under the pointer, and starts a job. The drop effect is always `COPY` (or `LINK` if the source doesn't
offer copy) and never `MOVE`, because Explorer deletes the originals after a move.

The wheel's geometry, hit-testing and shapes live in `calamansi-core::wheel` so they're unit-tested on
any OS; Windows only adds text with GDI and presents the result with `UpdateLayeredWindow`.

## Threads

- **UI thread:** message loop, hook callbacks, COM drop-target calls, all window state (`APP`).
- **Job thread:** one per job; runs `batch::run`, which uses up to four worker threads for native
  conversions and one for FFmpeg (already multi-threaded). Progress and the final report are posted
  back with `PostMessageW`.

UI state is reached through `with()`, which uses `try_borrow_mut`, so a message delivered re-entrantly
never panics. Modal loops (tray menu, file dialog) are entered with the state released.

## Outputs

`naming::reserve` creates the output file with `create_new` (atomic on NTFS), trying `name.ext`,
`name (2).ext`, … Converters write to a hidden temporary sibling and rename it over the reservation, so
a crash never leaves a half-written file under the final name and parallel jobs never collide.

## Why `windows-sys` and not the `windows` crate

`windows-sys` is plain FFI with no COM runtime, which keeps the build small and lets the Windows crate
**type-check on Linux and macOS** (it can't link there). The one COM object Calamansi implements,
`IDropTarget`, is a small hand-written vtable.
