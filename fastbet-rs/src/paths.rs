//! Where FastBet keeps its files: the user's own folders on a PC (as `directories` finds them),
//! the app's private storage on a phone (set when the activity starts).

use std::path::PathBuf;

#[cfg(not(target_os = "android"))]
fn project() -> Option<directories::ProjectDirs> {
    directories::ProjectDirs::from("gg", "fastbet", "FastBet")
}

/// settings.json and its backups.
#[cfg(not(target_os = "android"))]
pub fn config_dir() -> Option<PathBuf> {
    project().map(|d| d.config_dir().to_path_buf())
}

/// Browser profiles and other local data.
#[cfg(not(target_os = "android"))]
pub fn data_local_dir() -> Option<PathBuf> {
    project().map(|d| d.data_local_dir().to_path_buf())
}

/// Things that can be fetched again (game pictures).
#[cfg(not(target_os = "android"))]
pub fn cache_dir() -> Option<PathBuf> {
    project().map(|d| d.cache_dir().to_path_buf())
}

/// Where a saved hit card goes: Downloads, else the home folder.
#[cfg(not(target_os = "android"))]
pub fn downloads_dir() -> Option<PathBuf> {
    let dirs = directories::UserDirs::new()?;
    Some(dirs.download_dir().unwrap_or(dirs.home_dir()).to_path_buf())
}

#[cfg(target_os = "android")]
static ROOT: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();

/// The app's private storage (the activity's internal data path).
#[cfg(target_os = "android")]
pub fn set_root(dir: PathBuf) {
    let _ = ROOT.set(dir);
}

#[cfg(target_os = "android")]
fn under(name: &str) -> Option<PathBuf> {
    ROOT.get().map(|r| r.join(name))
}

#[cfg(target_os = "android")]
pub fn config_dir() -> Option<PathBuf> {
    under("config")
}

#[cfg(target_os = "android")]
pub fn data_local_dir() -> Option<PathBuf> {
    under("data")
}

#[cfg(target_os = "android")]
pub fn cache_dir() -> Option<PathBuf> {
    under("cache")
}

#[cfg(target_os = "android")]
pub fn downloads_dir() -> Option<PathBuf> {
    under("cards")
}
