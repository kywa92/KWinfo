//! Where log files live, how they are named, and how old ones are pruned.

use std::fs::File;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use anyhow::Context as _;

use crate::civil::Utc;

/// Newest files kept per directory, and the size budget for all of them together.
pub const KEEP_FILES: usize = 30;
pub const MAX_TOTAL_BYTES: u64 = 50 * 1024 * 1024;

pub struct LogFile {
    pub path: PathBuf,
    pub file: File,
}

/// The platform's log directory: `$XDG_STATE_HOME/kwinfo/logs` (usually
/// `~/.local/state/kwinfo/logs`) on Linux, the local data directory elsewhere, or the
/// temp directory if neither can be determined.
pub fn default_log_dir() -> PathBuf {
    match directories::ProjectDirs::from("io.github", "kylanwalters", "kwinfo") {
        Some(dirs) => dirs
            .state_dir()
            .unwrap_or_else(|| dirs.data_local_dir())
            .join("logs"),
        None => std::env::temp_dir().join("kwinfo").join("logs"),
    }
}

/// `override_dir`, else `KWINFO_LOG_DIR`, else [`default_log_dir`].
pub fn log_dir(override_dir: Option<&Path>) -> PathBuf {
    if let Some(dir) = override_dir {
        return dir.to_path_buf();
    }
    match std::env::var_os("KWINFO_LOG_DIR") {
        Some(dir) if !dir.is_empty() => PathBuf::from(dir),
        _ => default_log_dir(),
    }
}

/// Creates the directory, prunes old files and opens a fresh `<app>-<utc>-<pid>.log`
/// that starts with a one-line banner.
pub fn open(override_dir: Option<&Path>, app: &str, version: &str) -> anyhow::Result<LogFile> {
    let dir = log_dir(override_dir);
    std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    let _ = prune(&dir, KEEP_FILES, MAX_TOTAL_BYTES);
    let now = Utc::now();
    let path = dir.join(format!(
        "{app}-{}-{}.log",
        now.compact(),
        std::process::id()
    ));
    let mut file = File::create(&path).with_context(|| format!("creating {}", path.display()))?;
    writeln!(
        file,
        "# {app} {version} diagnostic log, started {} (UTC)",
        now.iso()
    )?;
    Ok(LogFile { path, file })
}

/// Log files in `dir`, newest first.
pub fn list(dir: &Path) -> Vec<(PathBuf, SystemTime, u64)> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut files: Vec<(PathBuf, SystemTime, u64)> = entries
        .flatten()
        .filter(|e| {
            let name = e.file_name();
            let name = name.to_string_lossy();
            name.starts_with("kwinfo") && name.ends_with(".log")
        })
        .filter_map(|e| {
            let meta = e.metadata().ok()?;
            Some((e.path(), meta.modified().ok()?, meta.len()))
        })
        .collect();
    files.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| b.0.cmp(&a.0)));
    files
}

/// Deletes everything beyond the newest `keep_files` or past `max_total_bytes`.
/// Returns how many files were removed.
pub fn prune(dir: &Path, keep_files: usize, max_total_bytes: u64) -> usize {
    let mut total = 0u64;
    let mut removed = 0;
    for (index, (path, _, size)) in list(dir).into_iter().enumerate() {
        total += size;
        let over_budget = index >= keep_files || total > max_total_bytes;
        if over_budget && std::fs::remove_file(&path).is_ok() {
            removed += 1;
        }
    }
    removed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prune_keeps_the_newest_files_within_budget() {
        let dir = std::env::temp_dir().join(format!("kwinfo-diag-prune-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        for i in 0..5 {
            let path = dir.join(format!("kwinfo-20260101-00000{i}-1.log"));
            std::fs::write(&path, vec![b'x'; 100]).unwrap();
            let mtime =
                SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000 + i * 60);
            File::options()
                .write(true)
                .open(&path)
                .unwrap()
                .set_modified(mtime)
                .unwrap();
        }
        std::fs::write(dir.join("unrelated.txt"), "keep me").unwrap();

        assert_eq!(prune(&dir, 3, u64::MAX), 2);
        let left = list(&dir);
        assert_eq!(left.len(), 3);
        assert!(left[0].0.ends_with("kwinfo-20260101-000004-1.log"));
        assert!(dir.join("unrelated.txt").exists());

        assert_eq!(
            prune(&dir, 10, 250),
            1,
            "the third file pushes the total past 250 bytes"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
