//! What FastBet keeps on this PC (Settings → Storage): how big each part is, and clearing it.
//! Bets never touch the disk; history, hits and stats live in memory only.

use std::path::{Path, PathBuf};

/// A part of FastBet's files that can be cleared.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Part {
    /// Slot and original artwork (re-downloaded when shown).
    Pictures,
    /// WebView2 profiles: login windows, the browser bridges and their Cloudflare cookies.
    Browser,
    /// Installers of updates already applied.
    Updates,
    /// Copies of settings.json kept when it couldn't be read; they hold old logins.
    Backups,
}

impl Part {
    pub const ALL: [Part; 4] = [Part::Pictures, Part::Browser, Part::Updates, Part::Backups];
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Usage {
    pub bytes: u64,
    pub files: u64,
}

/// How much each part takes.
#[derive(Clone, Debug, Default)]
pub struct Report {
    pub pictures: Usage,
    pub browser: Usage,
    pub updates: Usage,
    pub backups: Usage,
}

impl Report {
    pub fn get(&self, part: Part) -> Usage {
        match part {
            Part::Pictures => self.pictures,
            Part::Browser => self.browser,
            Part::Updates => self.updates,
            Part::Backups => self.backups,
        }
    }
}

fn project() -> Option<directories::ProjectDirs> {
    directories::ProjectDirs::from("gg", "fastbet", "FastBet")
}

/// The folder with settings.json (and the backups next to it).
pub fn settings_dir() -> Option<PathBuf> {
    project().map(|d| d.config_dir().to_path_buf())
}

fn pictures_dir() -> Option<PathBuf> {
    project().map(|d| d.cache_dir().join("images"))
}

fn browser_dir() -> Option<PathBuf> {
    project().map(|d| d.data_local_dir().join("webview"))
}

/// Where the updater saves installers (an AppImage updates next to itself instead: not ours to clear).
pub fn updates_dir() -> PathBuf {
    std::env::temp_dir().join("fastbet-update")
}

/// settings.json's backups: `settings.<anything>.json` beside it, never settings.json itself.
fn backups() -> Vec<PathBuf> {
    let Some(dir) = settings_dir() else { return vec![] };
    let Ok(rd) = std::fs::read_dir(dir) else { return vec![] };
    rd.flatten()
        .map(|e| e.path())
        .filter(|p| p.is_file())
        .filter(|p| {
            let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
            name.starts_with("settings.") && name.ends_with(".json") && name != "settings.json"
        })
        .collect()
}

/// Measures every part (walks the folders: call it off the UI thread).
pub fn measure() -> Report {
    let tree = |d: Option<PathBuf>| d.map(|d| usage(&d)).unwrap_or_default();
    Report {
        pictures: tree(pictures_dir()),
        browser: tree(browser_dir()),
        updates: usage(&updates_dir()),
        backups: backups().iter().fold(Usage::default(), |u, p| Usage { bytes: u.bytes + p.metadata().map(|m| m.len()).unwrap_or(0), files: u.files + 1 }),
    }
}

/// Deletes a part. Files another process has open (a login window, a connection's browser
/// bridge) stay; what's left is in the next [`measure`].
pub fn clear(part: Part) {
    match part {
        Part::Pictures => {
            if let Some(d) = pictures_dir() {
                remove_tree(&d);
            }
        }
        Part::Browser => {
            if let Some(d) = browser_dir() {
                remove_tree(&d);
            }
        }
        Part::Updates => remove_tree(&updates_dir()),
        Part::Backups => {
            for p in backups() {
                let _ = std::fs::remove_file(p);
            }
        }
    }
}

fn usage(dir: &Path) -> Usage {
    let mut u = Usage::default();
    let Ok(rd) = std::fs::read_dir(dir) else { return u };
    for e in rd.flatten() {
        let Ok(t) = e.file_type() else { continue };
        if t.is_dir() {
            let sub = usage(&e.path());
            u.bytes += sub.bytes;
            u.files += sub.files;
        } else {
            u.bytes += e.metadata().map(|m| m.len()).unwrap_or(0);
            u.files += 1;
        }
    }
    u
}

/// Removes everything under `dir` that can go, file by file, then the folders left empty.
fn remove_tree(dir: &Path) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        match e.file_type() {
            Ok(t) if t.is_dir() => {
                remove_tree(&p);
                let _ = std::fs::remove_dir(&p);
            }
            _ => {
                let _ = std::fs::remove_file(&p);
            }
        }
    }
}

/// Deletes the installers of earlier updates in `dir`, keeping `keep`.
pub fn prune_updates(dir: &Path, keep: &str) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        if name != keep && (name.ends_with("-setup.exe") || name.ends_with(".deb") || name.ends_with(".part")) {
            let _ = std::fs::remove_file(e.path());
        }
    }
}

/// 1234567 -> "1.2 MB".
pub fn fmt_bytes(b: u64) -> String {
    const UNITS: [&str; 4] = ["KB", "MB", "GB", "TB"];
    if b < 1024 {
        return format!("{b} B");
    }
    let mut v = b as f64 / 1024.0;
    let mut i = 0;
    while v >= 1024.0 && i < UNITS.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    if v >= 100.0 {
        format!("{v:.0} {}", UNITS[i])
    } else {
        format!("{v:.1} {}", UNITS[i])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_read_like_a_file_manager() {
        assert_eq!(fmt_bytes(0), "0 B");
        assert_eq!(fmt_bytes(1023), "1023 B");
        assert_eq!(fmt_bytes(1536), "1.5 KB");
        assert_eq!(fmt_bytes(874_000_000), "834 MB");
        assert_eq!(fmt_bytes(130_000_000), "124 MB");
        assert_eq!(fmt_bytes(5 * 1024 * 1024 * 1024), "5.0 GB");
    }

    #[test]
    fn a_tree_is_measured_and_emptied_and_old_installers_go() {
        let dir = std::env::temp_dir().join(format!("fastbet-storage-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("a/b")).unwrap();
        std::fs::write(dir.join("a/one.bin"), [0u8; 100]).unwrap();
        std::fs::write(dir.join("a/b/two.bin"), [0u8; 50]).unwrap();
        assert_eq!(usage(&dir), Usage { bytes: 150, files: 2 });
        remove_tree(&dir);
        assert_eq!(usage(&dir), Usage::default());
        assert!(dir.exists(), "the folder itself stays");

        for f in ["fastbet_3.12.0_x64-setup.exe", "fastbet_3.13.0_x64-setup.exe", ".fastbet_3.13.0_x64-setup.exe.part", "notes.txt"] {
            std::fs::write(dir.join(f), b"x").unwrap();
        }
        prune_updates(&dir, "fastbet_3.13.0_x64-setup.exe");
        let mut left: Vec<String> = std::fs::read_dir(&dir).unwrap().flatten().map(|e| e.file_name().to_string_lossy().to_string()).collect();
        left.sort();
        assert_eq!(left, ["fastbet_3.13.0_x64-setup.exe", "notes.txt"]);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
