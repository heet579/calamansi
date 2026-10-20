# Security policy

Calamansi opens files that may come from anywhere, so parser and file-handling bugs matter.

## Reporting a vulnerability

Please **don't open a public issue**. Use GitHub's private reporting instead: the repository's
**Security** tab → **Report a vulnerability**. Include the Calamansi version, Windows version, and a
sample file if one triggers the problem.

You should get a reply within 7 days. Fixes are released as soon as practical and credited to the
reporter unless they prefer otherwise.

## Scope

In scope: crashes or memory exhaustion from crafted input files, writing outside the chosen output
folder, overwriting or deleting user files, running unexpected programs, anything that sends data off
the machine.

FFmpeg is not part of Calamansi; report FFmpeg bugs to the FFmpeg project. Calamansi's own handling of
FFmpeg (arguments, paths, timeouts) is in scope.

## Current hardening and known gaps

- Inputs over 100 MB or 40 megapixels are refused before decoding images and PDFs.
- Outputs are reserved with exclusive create and written via a temporary file, so existing files are
  never overwritten and failures leave no partial outputs.
- FFmpeg runs as a separate process with a timeout and can be cancelled.
- Native conversions run in-process. Moving them into a sandboxed worker process is on the roadmap.
- Archives are not handled at all, by design, until extraction is properly bounded.
- Metadata is not deliberately stripped; don't rely on Calamansi to anonymise files.
