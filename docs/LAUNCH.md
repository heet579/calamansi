# Open-source launch checklist

A maintainer's to-do list for taking Calamansi public. Tick items off as you go.

## 1. Before the first public commit
- [ ] Decide the final name. "Calamansi" is a working name: search GitHub, the Microsoft Store and
      winget for clashes. If you rename, update `Cargo.toml`, `README.md`, window class names in
      `app.rs` and the mutex name in `main.rs`.
- [ ] Replace `YOUR-USERNAME` in `README.md` and the `repository` URL in `Cargo.toml`.
- [ ] Optional: tell the Open Tangerine maintainers (Sintax Error) about the project. Calamansi shares
      no code with Open Tangerine, but they may want to cross-link or collaborate.

## 2. Create the repository
- [ ] `git remote add origin https://github.com/<you>/calamansi.git` and push `main`
- [ ] Settings → General: enable Issues and Discussions; add a description and topics
      (`file-converter`, `windows`, `rust`, `drag-and-drop`, `ffmpeg`, `privacy`)
- [ ] Settings → Code security: enable **Private vulnerability reporting** (SECURITY.md relies on it),
      Dependabot alerts and secret scanning
- [ ] Settings → Branches: protect `main` (require the CI checks to pass)
- [ ] Create labels `good first issue`, `help wanted`, `format request`, `needs windows testing`

## 3. First release (0.1.0 preview)
- [ ] Work through `docs/BUILD-STATUS.md` on your own PC; fix blockers
- [ ] `git tag v0.1.0 && git push --tags` → the Release workflow builds the zip and SHA256SUMS
- [ ] Record a 20–30 second GIF of Shift-drag → wheel → converted files for the README

## 4. Code signing (removes most SmartScreen warnings)
- [ ] Add a short "Code signing policy" section to the README (who can approve releases)
- [ ] Apply to SignPath Foundation's free open-source program (signpath.org); requirements include an
      OSI license, public source, free downloads and automated CI builds — all already in place
- [ ] Once accepted: add secret `SIGNPATH_API_TOKEN` and variables `SIGNPATH_ORGANIZATION_ID`,
      `SIGNPATH_PROJECT_SLUG`; the Release workflow's sign job turns on automatically

## 5. Distribution
- [ ] **winget:** submit a manifest to `microsoft/winget-pkgs` pointing at the release zip
- [ ] **Microsoft Store:** register as an individual developer (free), package as MSIX; the Store
      signs and auto-updates Store installs
- [ ] **Scoop:** add a manifest to a bucket (e.g. `extras`)

## 6. Tell people
- [ ] Show HN post, r/windows, r/opensource, r/rust ("Show-and-tell" threads)
- [ ] AlternativeTo listing as an alternative to other converters
- [ ] Ask early users to try the manual checklist on their setups and report back
