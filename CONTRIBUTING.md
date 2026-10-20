# Contributing to Calamansi

Thanks for helping. Bug reports, testing on different Windows setups, new formats and docs are all
welcome.

## Ground rules

- **Local only.** No network calls during conversion, no telemetry, no bundled downloaders.
- **Never lose user data.** Outputs never overwrite existing files; originals are never modified,
  moved or deleted. Drops are always answered with *copy*.
- **Honest UI.** Only offer a route on the wheel if it works for every selected file.
- **Small dependency tree.** Prefer pure-Rust crates; justify anything that needs a C toolchain.

## Getting started

```powershell
cargo test --workspace          # Windows: everything
cargo test -p calamansi-core    # any OS: the engine
cargo clippy -p calamansi-win   # any OS: type-check the Windows app
```

Before opening a pull request, run:

```powershell
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

## Where things live

| You want to… | Look in |
| --- | --- |
| Add a format or route | `calamansi-core/src/format.rs`, `routes.rs`, then the converter (`image_ops.rs`, `media.rs`, …) and `batch.rs` |
| Add a tool to the Ctrl+Shift wheel | `batch.rs` (`Tool`), `wheel.rs` picks it up automatically |
| Change the wheel's look or hit-testing | `calamansi-core/src/wheel.rs` (unit-tested), text in `calamansi-win/src/app.rs` |
| Change tray, pill or drag behaviour | `calamansi-win/src/` (`app.rs`, `hook.rs`, `droptarget.rs`) |

Every new route needs a test that converts a real file and checks the output's signature. FFmpeg tests
must skip cleanly when FFmpeg isn't installed (see `media.rs`).

## Manual testing on Windows

The Windows shell can't be unit-tested end to end. If your change touches `calamansi-win`, run through
the checklist in [docs/BUILD-STATUS.md](docs/BUILD-STATUS.md) and say in the PR which items you tried
and on which Windows version and display scaling.

## Licensing of contributions

Calamansi is GPL-3.0-only. By submitting a contribution you agree it is licensed under the same terms.
You keep the copyright in your contribution. Please add a `Signed-off-by:` line (`git commit -s`) to
certify the [Developer Certificate of Origin](https://developercertificate.org/).

Don't copy code, artwork or text from other projects unless their license allows it and you note the
source in the PR. In particular, Calamansi is *inspired by* Open Tangerine but must not copy from it
without keeping its GPL notices and attribution.

## Code of conduct

Be kind and assume good faith. See [CODE_OF_CONDUCT.md](CODE_OF_CONDUCT.md).
