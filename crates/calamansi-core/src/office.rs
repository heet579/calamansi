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

/// The Office application that opens a family, with its process name.
fn ms_app(family: OfficeFamily) -> (&'static str, &'static str) {
    match family {
        OfficeFamily::Text => ("Word.Application", "WINWORD"),
        OfficeFamily::Sheet => ("Excel.Application", "EXCEL"),
        OfficeFamily::Slides => ("PowerPoint.Application", "POWERPNT"),
    }
}

/// PowerShell for one Office conversion, using the application object `$a`.
/// Paths come in through environment variables, so no quoting of user file
/// names is needed.
fn ms_script(family: OfficeFamily, to: Format) -> Result<&'static str> {
    use Format::*;
    Ok(match (family, to) {
        // wdFormatPDF = 17, wdFormatDocumentDefault = 16. Open(FileName,
        // ConfirmConversions=false, ReadOnly=true, AddToRecentFiles=false).
        (OfficeFamily::Text, Pdf) => {
            "$a.Visible=$false;$a.DisplayAlerts=0;\
             $d=$a.Documents.Open($env:CAL_IN,$false,$true,$false);$d.ExportAsFixedFormat($env:CAL_OUT,17);$d.Close(0)"
        }
        (OfficeFamily::Text, Docx) => {
            "$a.Visible=$false;$a.DisplayAlerts=0;\
             $d=$a.Documents.Open($env:CAL_IN,$false,$true,$false);$d.SaveAs2($env:CAL_OUT,16);$d.Close(0)"
        }
        // xlTypePDF = 0, xlOpenXMLWorkbook = 51.
        (OfficeFamily::Sheet, Pdf) => {
            "$a.Visible=$false;$a.DisplayAlerts=$false;\
             $b=$a.Workbooks.Open($env:CAL_IN,0,$true);$b.ExportAsFixedFormat(0,$env:CAL_OUT);$b.Close($false)"
        }
        (OfficeFamily::Sheet, Xlsx) => {
            "$a.Visible=$false;$a.DisplayAlerts=$false;\
             $b=$a.Workbooks.Open($env:CAL_IN,0,$true);$b.SaveAs($env:CAL_OUT,51);$b.Close($false)"
        }
        // ppSaveAsPDF = 32, ppSaveAsOpenXMLPresentation = 24.
        // Open(FileName, ReadOnly=msoTrue, Untitled=msoFalse, WithWindow=msoFalse).
        (OfficeFamily::Slides, Pdf) => {
            "$r=$a.Presentations.Open($env:CAL_IN,-1,0,0);$r.SaveAs($env:CAL_OUT,32);$r.Close()"
        }
        (OfficeFamily::Slides, Pptx) => {
            "$r=$a.Presentations.Open($env:CAL_IN,-1,0,0);$r.SaveAs($env:CAL_OUT,24);$r.Close()"
        }
        _ => {
            return Err(CoreError::InvalidInput(format!(
                "Microsoft Office can't save this as {to}"
            )))
        }
    })
}

/// Word asks "Word will now convert your PDF…" before opening a PDF. A
/// hidden Word can't show that prompt, so it would wait forever. Turn the
/// prompt off for the conversion (per Office version) and put it back after.
const WORD_PDF_PROMPT_OFF: &str = "$keys=@(Get-ChildItem 'HKCU:\\Software\\Microsoft\\Office' -EA SilentlyContinue|\
Where-Object{$_.PSChildName -match '^\\d+\\.\\d+$'}|ForEach-Object{Join-Path $_.PSPath 'Word\\Options'});$old=@{};\
foreach($k in $keys){if(!(Test-Path $k)){New-Item $k -Force|Out-Null};\
$old[$k]=(Get-ItemProperty $k -Name DisableConvertPdfWarning -EA SilentlyContinue).DisableConvertPdfWarning;\
Set-ItemProperty $k -Name DisableConvertPdfWarning -Value 1 -Type DWord};";

const WORD_PDF_PROMPT_RESTORE: &str = "foreach($k in $keys){if($null -eq $old[$k]){\
Remove-ItemProperty $k -Name DisableConvertPdfWarning -EA SilentlyContinue}else{\
Set-ItemProperty $k -Name DisableConvertPdfWarning -Value $old[$k] -Type DWord}};";

/// The whole PowerShell command for a conversion.
fn ms_command(input: &Path, family: OfficeFamily, to: Format) -> Result<String> {
    let (prog_id, process) = ms_app(family);
    let body = ms_script(family, to)?;
    let pdf_in = crate::format::detect(input) == Some(Format::Pdf);
    let (pre, post) = if pdf_in && family == OfficeFamily::Text {
        (WORD_PDF_PROMPT_OFF, WORD_PDF_PROMPT_RESTORE)
    } else {
        ("", "")
    };
    // Record the process id of the instance we start, so a hung Office can
    // be stopped (killing PowerShell alone leaves Office running).
    Ok(format!(
        "$ErrorActionPreference='Stop';{pre}\
         $a=New-Object -ComObject {prog_id};\
         $p=Get-Process {process} -EA SilentlyContinue|Sort-Object StartTime -Descending|Select-Object -First 1;\
         if($p){{Set-Content -LiteralPath $env:CAL_PIDFILE -Value $p.Id}};\
         try{{{body}}}finally{{try{{$a.Quit()}}catch{{}};{post}}}"
    ))
}

fn microsoft(
    input: &Path,
    family: OfficeFamily,
    to: Format,
    output: &Path,
    cancel: &AtomicBool,
    timeout: Duration,
) -> Result<()> {
    let script = ms_command(input, family, to)?;
    // A healthy Office conversion takes seconds; a stuck one should give up
    // well before the general helper timeout.
    let timeout = timeout.min(Duration::from_secs(180));
    // Office needs absolute paths.
    let abs = |p: &Path| std::path::absolute(p).unwrap_or_else(|_| p.to_path_buf());
    let pidfile = std::env::temp_dir().join(format!(
        "calamansi-office-{}-{}.pid",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
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
        .env("CAL_PIDFILE", &pidfile)
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()?;
    let result = wait_with_timeout(child, cancel, timeout, "Microsoft Office");
    if result.is_err() {
        // Stop the Office instance this conversion started, if it's still there.
        if let Some(pid) = std::fs::read_to_string(&pidfile)
            .ok()
            .and_then(|s| s.trim().parse::<u32>().ok())
        {
            let _ = hidden_command(Path::new("taskkill.exe"))
                .args(["/PID", &pid.to_string(), "/T", "/F"])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
        }
    }
    let _ = std::fs::remove_file(&pidfile);
    result?;
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
