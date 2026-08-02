//! Append-only updater log under `~/Library/Logs`. Update checks fail in ways
//! that leave no trace in the UI (Sparkle silently drops a check while it is
//! busy, a feed probe times out, ...), and stderr goes nowhere when the app is
//! launched from Finder - so keep a copy on disk for remote diagnosis.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// A check writes a handful of lines, so a small cap holds many sessions.
/// Past it the file is dropped rather than rotated; old checks aren't useful.
const MAX_BYTES: u64 = 256 * 1024;

static WRITE_LOCK: Mutex<()> = Mutex::new(());

fn log_path() -> Option<PathBuf> {
    let dir = dirs::home_dir()?
        .join("Library/Logs")
        .join(crate::settings::APP_NAME);
    fs::create_dir_all(&dir).ok()?;
    Some(dir.join("updater.log"))
}

pub fn write(message: &str) {
    eprintln!("updater: {message}");

    let _guard = WRITE_LOCK.lock();
    if let Some(path) = log_path() {
        append(&path, message);
    }
}

fn append(path: &Path, message: &str) {
    if fs::metadata(path).is_ok_and(|meta| meta.len() > MAX_BYTES) {
        let _ = fs::remove_file(path);
    }
    let Ok(mut file) = OpenOptions::new().create(true).append(true).open(path) else {
        return;
    };
    let stamp = chrono::Local::now().format("%Y-%m-%d %H:%M:%S%.3f");
    let _ = writeln!(file, "[{stamp}] {message}");
}

macro_rules! log {
    ($($arg:tt)*) => {
        $crate::updater::logging::write(&format!($($arg)*))
    };
}
pub(crate) use log;

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_path(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(name);
        let _ = fs::remove_file(&path);
        path
    }

    #[test]
    fn appends_timestamped_lines() {
        let path = temp_path("tclc-updater-append.log");
        append(&path, "first");
        append(&path, "second");

        let contents = fs::read_to_string(&path).expect("log file");
        let lines: Vec<&str> = contents.lines().collect();
        assert_eq!(lines.len(), 2);
        assert!(lines[0].ends_with("] first"), "got {:?}", lines[0]);
        assert!(lines[1].ends_with("] second"), "got {:?}", lines[1]);
        assert!(lines[0].starts_with('['));
    }

    #[test]
    fn drops_the_file_once_it_outgrows_the_cap() {
        let path = temp_path("tclc-updater-cap.log");
        fs::write(&path, vec![b'x'; MAX_BYTES as usize + 1]).expect("seed log file");

        append(&path, "after reset");

        let contents = fs::read_to_string(&path).expect("log file");
        assert!(contents.ends_with("] after reset\n"), "got {contents:?}");
        assert!((contents.len() as u64) < MAX_BYTES);
    }

    #[test]
    fn logs_land_under_the_user_logs_directory() {
        let path = log_path().expect("log path");
        assert!(path.ends_with("Library/Logs/小小万年历/updater.log"), "got {path:?}");
    }
}
