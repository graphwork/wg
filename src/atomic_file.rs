//! Small helpers for crash-safe runtime file updates.
//!
//! Runtime JSON files are read by concurrent CLI/status commands. Writing via
//! temp-file + rename prevents readers from observing a half-written file after
//! disk-full or process-exit failures.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

static NEXT_TMP_ID: AtomicU64 = AtomicU64::new(0);

fn unique_suffix() -> String {
    let seq = NEXT_TMP_ID.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("{}.{}.{}", std::process::id(), nanos, seq)
}

fn file_name_lossy(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "file".to_string())
}

/// Write `contents` to `path` by creating a same-directory temporary file,
/// syncing it, and atomically renaming it into place.
pub fn write_atomic(path: &Path, contents: impl AsRef<[u8]>) -> io::Result<()> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent)?;

    let tmp_path = parent.join(format!(
        ".{}.tmp.{}",
        file_name_lossy(path),
        unique_suffix()
    ));

    let result = (|| -> io::Result<()> {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp_path)?;
        file.write_all(contents.as_ref())?;
        file.sync_all()?;
        Ok(())
    })();

    if let Err(err) = result {
        let _ = fs::remove_file(&tmp_path);
        return Err(err);
    }

    if let Err(err) = fs::rename(&tmp_path, path) {
        let _ = fs::remove_file(&tmp_path);
        return Err(err);
    }

    sync_parent_dir(parent);
    Ok(())
}

/// Move a corrupt runtime file aside so subsequent loads do not repeatedly
/// warn on the same bytes. Returns the quarantine path when a file was moved.
pub fn quarantine_corrupt_file(path: &Path) -> io::Result<Option<PathBuf>> {
    if !path.exists() {
        return Ok(None);
    }

    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let base = file_name_lossy(path);
    let mut last_err = None;

    for _ in 0..10 {
        let quarantine = parent.join(format!("{}.corrupt-{}", base, unique_suffix()));
        match fs::rename(path, &quarantine) {
            Ok(()) => {
                sync_parent_dir(parent);
                return Ok(Some(quarantine));
            }
            Err(err) if err.kind() == io::ErrorKind::AlreadyExists => {
                last_err = Some(err);
            }
            Err(err) => return Err(err),
        }
    }

    Err(last_err.unwrap_or_else(|| io::Error::other("failed to choose quarantine path")))
}

fn sync_parent_dir(parent: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::io::AsRawFd;
        if let Ok(dir) = File::open(parent) {
            let _ = unsafe { libc::fsync(dir.as_raw_fd()) };
        }
    }

    #[cfg(not(unix))]
    {
        let _ = parent;
    }
}
