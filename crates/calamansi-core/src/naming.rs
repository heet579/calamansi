//! Output file naming. Existing files are never overwritten.

use crate::error::{CoreError, Result};
use std::fs::{self, OpenOptions};
use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};

/// Atomically reserve a fresh output path: `stem.ext`, then `stem (2).ext`, …
///
/// The file is created empty with `create_new`, so two jobs racing for the
/// same name can never both get it. Callers fill it with [`write_atomically`]
/// or remove it with [`release`] on failure.
pub fn reserve(dir: &Path, stem: &str, ext: &str) -> io::Result<PathBuf> {
    fs::create_dir_all(dir)?;
    for n in 1..10_000u32 {
        let name = if n == 1 {
            format!("{stem}.{ext}")
        } else {
            format!("{stem} ({n}).{ext}")
        };
        let candidate = dir.join(name);
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&candidate)
        {
            Ok(_) => return Ok(candidate),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
    Err(io::Error::other("too many files with the same name"))
}

/// Remove a reserved placeholder after a failed or cancelled job.
pub fn release(path: &Path) {
    let _ = fs::remove_file(path);
    let _ = fs::remove_file(temp_path(path));
}

fn temp_path(path: &Path) -> PathBuf {
    let mut s = path.as_os_str().to_os_string();
    s.push(".calamansi-part");
    PathBuf::from(s)
}

/// Write `path` via a temporary sibling and rename it into place, so a crash
/// or error never leaves a half-written file under the final name.
pub fn write_atomically<F>(path: &Path, write: F) -> Result<()>
where
    F: FnOnce(&mut BufWriter<fs::File>) -> Result<()>,
{
    let tmp = temp_path(path);
    let result = (|| {
        let file = fs::File::create(&tmp)?;
        let mut w = BufWriter::new(file);
        write(&mut w)?;
        w.flush()?;
        let file = w.into_inner().map_err(|e| CoreError::Io(e.into_error()))?;
        file.sync_all()?;
        drop(file);
        fs::rename(&tmp, path)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

/// Temporary path an external tool (FFmpeg) can write to before
/// [`commit_external`] moves it over the reserved name.
pub fn external_temp(path: &Path) -> PathBuf {
    // Keep the real extension last so FFmpeg picks the right muxer.
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("output");
    let ext = path.extension().and_then(|s| s.to_str()).unwrap_or("bin");
    path.with_file_name(format!(".{stem}.calamansi-part.{ext}"))
}

pub fn commit_external(tmp: &Path, path: &Path) -> io::Result<()> {
    fs::rename(tmp, path)
}

/// File stem of an input, used to build output names.
pub fn stem_of(path: &Path) -> String {
    path.file_stem()
        .and_then(|s| s.to_str())
        .filter(|s| !s.is_empty())
        .unwrap_or("output")
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn never_reuses_a_name() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("photo.jpg"), b"original").unwrap();
        let a = reserve(dir.path(), "photo", "jpg").unwrap();
        let b = reserve(dir.path(), "photo", "jpg").unwrap();
        assert!(a.ends_with("photo (2).jpg"));
        assert!(b.ends_with("photo (3).jpg"));
        assert_eq!(fs::read(dir.path().join("photo.jpg")).unwrap(), b"original");
    }

    #[test]
    fn concurrent_reservations_are_unique() {
        let dir = Arc::new(tempfile::tempdir().unwrap());
        let handles: Vec<_> = (0..16)
            .map(|_| {
                let dir = dir.clone();
                std::thread::spawn(move || reserve(dir.path(), "x", "png").unwrap())
            })
            .collect();
        let mut names: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();
        names.sort();
        names.dedup();
        assert_eq!(names.len(), 16);
    }

    #[test]
    fn failed_write_leaves_only_placeholder_for_release() {
        let dir = tempfile::tempdir().unwrap();
        let p = reserve(dir.path(), "doc", "pdf").unwrap();
        let r = write_atomically(&p, |_| Err(CoreError::InvalidInput("boom".into())));
        assert!(r.is_err());
        release(&p);
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 0);
    }

    #[test]
    fn external_temp_keeps_extension() {
        let t = external_temp(Path::new("C:/out/clip.mp4"));
        assert_eq!(t.extension().unwrap(), "mp4");
        assert!(t.file_name().unwrap().to_str().unwrap().starts_with('.'));
    }
}
