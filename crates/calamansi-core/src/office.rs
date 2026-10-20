//! Office documents via an installed office suite.
//!
//! Like FFmpeg, Calamansi never bundles or downloads an office suite. It uses
//! Microsoft Office (Word, Excel, PowerPoint through COM automation in
//! PowerShell) or LibreOffice (`soffice --headless --convert-to`) when one is
//! installed. Files are opened read-only and never modified.

use crate::error::{CoreError, Result};
use crate::format::{Format, OfficeFamily};
use crate::media::{hidden_command, wait_with_timeout};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Office {
    Microsoft {
        word: bool,
        excel: bool,
        powerpoint: bool,
    },
    LibreOffice(PathBuf),
}

impl Office {
    /// Find an office suite: a configured LibreOffice path, then Microsoft
    /// Office, then LibreOffice in its usual locations.
    pub fn locate(custom: Option<&Path>) -> Option<Office> {
        if let Some(p) = custom.filter(|p| p.is_file()) {
            return Some(Office::LibreOffice(p.to_path_buf()));
        }
        if cfg!(windows) {
            let has = |prog_id: &str| registry_has(&format!(r"HKCR\{prog_id}\CurVer"));
            let (word, excel, powerpoint) = (
                has("Word.Application"),
                has("Excel.Application"),
                has("PowerPoint.Application"),
            );
            if word || excel || powerpoint {
                return Some(Office::Microsoft {
                    word,
                    excel,
                    powerpoint,
                });
            }
        }
        libreoffice_paths()
            .into_iter()
            .find(|p| p.is_file())
            .map(Office::LibreOffice)
    }

    pub fn name(&self) -> &'static str {
        match self {
            Office::Microsoft { .. } => "Microsoft Office",
            Office::LibreOffice(_) => "LibreOffice",
        }
    }

    pub fn handles(&self, family: OfficeFamily) -> bool {
        match self {
            Office::LibreOffice(_) => true,
            Office::Microsoft {
                word,
                excel,
                powerpoint,
            } => match family {
                OfficeFamily::Text => *word,
                OfficeFamily::Sheet => *excel,
                OfficeFamily::Slides => *powerpoint,
            },
        }
    }

    /// Whether Word can open PDFs for PDF → Word (it reflows them into editable text).
    pub fn word_reads_pdf(&self) -> bool {
        matches!(self, Office::Microsoft { word: true, .. })
    }

    /// Convert `input` to `to`, writing `output`. `family` is the family of
    /// the application that opens the input (Text for PDFs opened in Word).
    pub fn convert(
        &self,
        input: &Path,
        family: OfficeFamily,
        to: Format,
        output: &Path,
        cancel: &AtomicBool,
        timeout: Duration,
    ) -> Result<()> {
        if !self.handles(family) {
            return Err(CoreError::InvalidInput(format!(
                "{} can't open this kind of document",
                self.name()
            )));
        }
        match self {
            Office::Microsoft { .. } => microsoft(input, family, to, output, cancel, timeout),
            Office::LibreOffice(exe) => libreoffice(exe, input, to, output, cancel, timeout),
        }
    }
}

fn registry_has(key: &str) -> bool {
    hidden_command(Path::new("reg.exe"))
        .args(["query", key])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

fn libreoffice_paths() -> Vec<PathBuf> {
    let mut v = Vec::new();
    if cfg!(windows) {
        for var in ["ProgramFiles", "ProgramFiles(x86)"] {
            if let Some(pf) = std::env::var_os(var) {
                v.push(PathBuf::from(pf).join(r"LibreOffice\program\soffice.exe"));
            }
        }
    } else {
        if let Some(path) = std::env::var_os("PATH") {
            for d in std::env::split_paths(&path) {
                v.push(d.join("soffice"));
                v.push(d.join("libreoffice"));
            }
        }
        v.push(PathBuf::from(
            "/Applications/LibreOffice.app/Contents/MacOS/soffice",
        ));
    }
    v
}

/// PowerShell script for one Office conversion. Paths come in through
/// environment variables, so no quoting of user file names is needed.
fn ms_script(family: OfficeFamily, to: Format) -> Result<&'static str> {
    use Format::*;
    Ok(match (family, to) {
        // wdFormatPDF = 17, wdFormatDocumentDefault = 16. Open(FileName,
        // ConfirmConversions=false, ReadOnly=true, AddToRecentFiles=false).
        (OfficeFamily::Text, Pdf) => {
            "$w=New-Object -ComObject Word.Application;$w.Visible=$false;$w.DisplayAlerts=0;\
             try{$d=$w.Documents.Open($env:CAL_IN,$false,$true,$false);$d.ExportAsFixedFormat($env:CAL_OUT,17);$d.Close(0)}finally{$w.Quit()}"
        }
        (OfficeFamily::Text, Docx) => {
            "$w=New-Object -ComObject Word.Application;$w.Visible=$false;$w.DisplayAlerts=0;\
             try{$d=$w.Documents.Open($env:CAL_IN,$false,$true,$false);$d.SaveAs2($env:CAL_OUT,16);$d.Close(0)}finally{$w.Quit()}"
        }
        // xlTypePDF = 0, xlOpenXMLWorkbook = 51.
        (OfficeFamily::Sheet, Pdf) => {
            "$x=New-Object -ComObject Excel.Application;$x.Visible=$false;$x.DisplayAlerts=$false;\
             try{$b=$x.Workbooks.Open($env:CAL_IN,0,$true);$b.ExportAsFixedFormat(0,$env:CAL_OUT);$b.Close($false)}finally{$x.Quit()}"
        }
        (OfficeFamily::Sheet, Xlsx) => {
            "$x=New-Object -ComObject Excel.Application;$x.Visible=$false;$x.DisplayAlerts=$false;\
             try{$b=$x.Workbooks.Open($env:CAL_IN,0,$true);$b.SaveAs($env:CAL_OUT,51);$b.Close($false)}finally{$x.Quit()}"
        }
        // ppSaveAsPDF = 32, ppSaveAsOpenXMLPresentation = 24.
        // Open(FileName, ReadOnly=msoTrue, Untitled=msoFalse, WithWindow=msoFalse).
        (OfficeFamily::Slides, Pdf) => {
            "$p=New-Object -ComObject PowerPoint.Application;\
             try{$r=$p.Presentations.Open($env:CAL_IN,-1,0,0);$r.SaveAs($env:CAL_OUT,32);$r.Close()}finally{$p.Quit()}"
        }
        (OfficeFamily::Slides, Pptx) => {
            "$p=New-Object -ComObject PowerPoint.Application;\
             try{$r=$p.Presentations.Open($env:CAL_IN,-1,0,0);$r.SaveAs($env:CAL_OUT,24);$r.Close()}finally{$p.Quit()}"
        }
        _ => {
            return Err(CoreError::InvalidInput(format!(
                "Microsoft Office can't save this as {to}"
            )))
        }
    })
}

fn microsoft(
    input: &Path,
    family: OfficeFamily,
    to: Format,
    output: &Path,
    cancel: &AtomicBool,
    timeout: Duration,
) -> Result<()> {
    let script = format!("$ErrorActionPreference='Stop';{}", ms_script(family, to)?);
    // Office needs absolute paths.
    let abs = |p: &Path| std::path::absolute(p).unwrap_or_else(|_| p.to_path_buf());
    let child = hidden_command(Path::new("powershell.exe"))
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-Command",
            &script,
        ])
        .env("CAL_IN", abs(input))
        .env("CAL_OUT", abs(output))
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()?;
    wait_with_timeout(child, cancel, timeout, "Microsoft Office")?;
    if !output.is_file() {
        return Err(CoreError::InvalidInput(
            "Microsoft Office didn't produce a file".into(),
        ));
    }
    Ok(())
}

fn libreoffice(
    exe: &Path,
    input: &Path,
    to: Format,
    output: &Path,
    cancel: &AtomicBool,
    timeout: Duration,
) -> Result<()> {
    // LibreOffice names its output after the input, so convert into a
    // private temporary folder and move the result into place.
    let dir = output
        .parent()
        .unwrap_or(Path::new("."))
        .join(format!(".calamansi-office-{}", std::process::id()));
    std::fs::create_dir_all(&dir)?;
    let profile = dir.join("profile");
    let profile_url = format!(
        "file:///{}",
        profile
            .display()
            .to_string()
            .replace('\\', "/")
            .trim_start_matches('/')
    );
    let result = (|| {
        let child = hidden_command(exe)
            .arg(format!("-env:UserInstallation={profile_url}"))
            .args([
                "--headless",
                "--norestore",
                "--convert-to",
                to.extension(),
                "--outdir",
            ])
            .arg(&dir)
            .arg(input)
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()?;
        wait_with_timeout(child, cancel, timeout, "LibreOffice")?;
        let stem = input.file_stem().unwrap_or_default();
        let produced = dir.join(stem).with_extension(to.extension());
        if !produced.is_file() {
            return Err(CoreError::InvalidInput(
                "LibreOffice didn't produce a file".into(),
            ));
        }
        std::fs::rename(&produced, output)?;
        Ok(())
    })();
    let _ = std::fs::remove_dir_all(&dir);
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scripts_exist_for_every_office_route() {
        use crate::routes::{route, Engine};
        for &from in Format::ALL {
            for &to in Format::ALL {
                if route(from, to) == Some(Engine::Office) {
                    let family = from.office_family().unwrap();
                    assert!(ms_script(family, to).is_ok(), "{from:?} -> {to:?}");
                }
            }
        }
        assert!(
            ms_script(OfficeFamily::Text, Format::Docx).is_ok(),
            "PDF -> Word via Word"
        );
    }

    #[test]
    fn microsoft_capabilities() {
        let o = Office::Microsoft {
            word: true,
            excel: false,
            powerpoint: true,
        };
        assert!(o.handles(OfficeFamily::Text) && !o.handles(OfficeFamily::Sheet));
        assert!(o.word_reads_pdf());
    }

    #[test]
    fn libreoffice_if_installed() {
        let Some(office @ Office::LibreOffice(_)) = Office::locate(None) else {
            return;
        };
        let dir = tempfile::tempdir().unwrap();
        let input = dir.path().join("note.docx");
        let mut f = std::fs::File::create(&input).unwrap();
        crate::docx::write(&["Hello from Calamansi".into()], &mut f).unwrap();
        drop(f);
        let out = dir.path().join("note.pdf");
        office
            .convert(
                &input,
                OfficeFamily::Text,
                Format::Pdf,
                &out,
                &AtomicBool::new(false),
                Duration::from_secs(120),
            )
            .unwrap();
        assert_eq!(crate::format::detect(&out), Some(Format::Pdf));
    }
}
