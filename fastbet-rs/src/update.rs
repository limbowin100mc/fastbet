//! Updates from the GitHub releases of limbowin100mc/fastbet: compare the
//! latest release with this build's version, download the installer for this
//! system (checked against GitHub's SHA-256 digest) and start it.
//!
//! Windows runs the NSIS setup; a Linux AppImage replaces itself; a Linux
//! .deb install gets the package opened in the browser (installing it needs root).

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use parking_lot::Mutex;
use serde::Deserialize;
use sha2::{Digest, Sha256};

const LATEST: &str = "https://api.github.com/repos/limbowin100mc/fastbet/releases/latest";
pub const RELEASES_PAGE: &str = "https://github.com/limbowin100mc/fastbet/releases/latest";
const CURRENT: &str = env!("CARGO_PKG_VERSION");

/// This build's version. `FASTBET_PRETEND_VERSION` overrides it, to see the update flow (UI work).
pub fn current() -> String {
    std::env::var("FASTBET_PRETEND_VERSION").unwrap_or_else(|_| CURRENT.to_string())
}

#[derive(Clone, Debug, Deserialize)]
pub struct Asset {
    pub name: String,
    pub size: u64,
    pub browser_download_url: String,
    /// "sha256:<hex>"
    #[serde(default)]
    pub digest: Option<String>,
}

#[derive(Debug, Deserialize)]
struct Release {
    tag_name: String,
    html_url: String,
    #[serde(default)]
    assets: Vec<Asset>,
}

/// What this build can do with a release.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Platform {
    /// NSIS setup .exe, installed in-app.
    Windows,
    /// The running AppImage is swapped for the new one.
    AppImage,
    /// A .deb install: the package is opened in the browser.
    Deb,
    Other,
}

impl Platform {
    fn current() -> Self {
        if cfg!(windows) {
            Platform::Windows
        } else if cfg!(target_os = "linux") {
            if appimage_path().is_some() {
                Platform::AppImage
            } else {
                Platform::Deb
            }
        } else {
            Platform::Other
        }
    }

    fn pick(self, assets: &[Asset]) -> Option<Asset> {
        let suffix = match self {
            Platform::Windows => "-setup.exe",
            Platform::AppImage => ".appimage",
            Platform::Deb => ".deb",
            Platform::Other => return None,
        };
        assets.iter().find(|a| a.name.to_ascii_lowercase().ends_with(suffix)).cloned()
    }
}

#[derive(Clone, Debug, Default)]
pub enum State {
    #[default]
    Idle,
    Checking,
    UpToDate,
    Available {
        version: String,
        page: String,
        asset: Option<Asset>,
        /// The app can install `asset` itself; otherwise it is opened in the browser.
        in_app: bool,
    },
    Downloading {
        version: String,
        done: u64,
        total: u64,
    },
    /// The installer is running (or the new AppImage is in place); the app closes now.
    Installing {
        version: String,
    },
    Failed(String),
}

pub struct Updater {
    state: Arc<Mutex<State>>,
    cancel: Arc<AtomicBool>,
    repaint: Arc<dyn Fn() + Send + Sync>,
}

impl Updater {
    pub fn new(repaint: Arc<dyn Fn() + Send + Sync>) -> Self {
        Self { state: Arc::new(Mutex::new(State::Idle)), cancel: Arc::new(AtomicBool::new(false)), repaint }
    }

    pub fn state(&self) -> State {
        self.state.lock().clone()
    }

    pub fn busy(&self) -> bool {
        matches!(*self.state.lock(), State::Checking | State::Downloading { .. } | State::Installing { .. })
    }

    /// Asks GitHub for the latest release, on a worker thread.
    pub fn check(&self) {
        if self.busy() {
            return;
        }
        *self.state.lock() = State::Checking;
        let (state, repaint) = (self.state.clone(), self.repaint.clone());
        std::thread::spawn(move || {
            let next = match latest() {
                Ok(r) => {
                    let version = r.tag_name.trim_start_matches(['v', 'V']).to_string();
                    if is_newer(&version, &current()) {
                        let platform = Platform::current();
                        let asset = platform.pick(&r.assets);
                        let in_app = asset.is_some() && matches!(platform, Platform::Windows | Platform::AppImage);
                        State::Available { version, page: r.html_url, asset, in_app }
                    } else {
                        State::UpToDate
                    }
                }
                Err(e) => State::Failed(e),
            };
            *state.lock() = next;
            repaint();
        });
    }

    /// Downloads the update and starts it, on a worker thread.
    pub fn install(&self) {
        let (version, asset) = match &*self.state.lock() {
            State::Available { version, asset: Some(asset), in_app: true, .. } => (version.clone(), asset.clone()),
            _ => return,
        };
        self.cancel.store(false, Ordering::SeqCst);
        *self.state.lock() = State::Downloading { version: version.clone(), done: 0, total: asset.size };
        let (state, cancel, repaint) = (self.state.clone(), self.cancel.clone(), self.repaint.clone());
        std::thread::spawn(move || {
            let progress = |done: u64| {
                if let State::Downloading { done: d, .. } = &mut *state.lock() {
                    *d = done;
                }
                repaint();
            };
            let res = download_dir().and_then(|dir| {
                let file = dir.join(&asset.name);
                download(&asset, &file, &cancel, progress)?;
                // earlier installers aren't needed any more (an AppImage's folder is the user's: left alone)
                if appimage_path().is_none() {
                    crate::storage::prune_updates(&dir, &asset.name);
                }
                launch(&file)
            });
            *state.lock() = match res {
                Ok(()) => State::Installing { version },
                Err(e) if cancel.load(Ordering::SeqCst) => {
                    tracing::info!("update cancelled: {e}");
                    State::Idle
                }
                Err(e) => State::Failed(e),
            };
            repaint();
        });
    }

    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::SeqCst);
    }

    /// Forgets the last result (the dialog was closed), unless work is still running.
    pub fn dismiss(&self) {
        if !self.busy() {
            *self.state.lock() = State::Idle;
        }
    }
}

fn client() -> Result<reqwest::blocking::Client, String> {
    reqwest::blocking::Client::builder()
        .user_agent(format!("FastBet/{CURRENT}"))
        .connect_timeout(Duration::from_secs(15))
        // per read, so a slow download is fine but a stalled one gives up
        .timeout(Duration::from_secs(30))
        .build()
        .map_err(|e| e.to_string())
}

fn latest() -> Result<Release, String> {
    let resp = client()?
        .get(LATEST)
        .header("Accept", "application/vnd.github+json")
        .send()
        .map_err(|e| format!("Couldn't reach GitHub: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("GitHub answered {}", resp.status()));
    }
    resp.json::<Release>().map_err(|e| format!("Unexpected reply from GitHub: {e}"))
}

/// "3.10.0" > "3.9.2"; a leading "v" and any "-beta"/"+build" suffix are ignored.
pub fn is_newer(latest: &str, current: &str) -> bool {
    fn parts(v: &str) -> Vec<u64> {
        let v = v.trim().trim_start_matches(['v', 'V']);
        let core = v.split(['-', '+']).next().unwrap_or("");
        core.split('.').map(|p| p.parse().unwrap_or(0)).collect()
    }
    let (a, b) = (parts(latest), parts(current));
    let n = a.len().max(b.len());
    let get = |v: &Vec<u64>, i: usize| v.get(i).copied().unwrap_or(0);
    (0..n).map(|i| get(&a, i).cmp(&get(&b, i))).find(|o| o.is_ne()).is_some_and(|o| o.is_gt())
}

/// Where the download goes: the temp dir for an installer, next to the AppImage for an AppImage.
fn download_dir() -> Result<PathBuf, String> {
    let dir = match appimage_path() {
        Some(p) => p.parent().map(Path::to_path_buf).ok_or("AppImage has no folder")?,
        None => std::env::temp_dir().join("fastbet-update"),
    };
    std::fs::create_dir_all(&dir).map_err(|e| format!("Couldn't create {}: {e}", dir.display()))?;
    Ok(dir)
}

/// Streams the asset to `dest`, checking its size and SHA-256 against what GitHub published.
fn download(asset: &Asset, dest: &Path, cancel: &AtomicBool, progress: impl Fn(u64)) -> Result<(), String> {
    let mut resp = client()?.get(&asset.browser_download_url).send().map_err(|e| format!("Download failed: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("Download failed: HTTP {}", resp.status()));
    }
    let part = dest.with_file_name(format!(".{}.part", asset.name));
    let res = (|| {
        let mut file = std::fs::File::create(&part).map_err(|e| format!("Couldn't save the update: {e}"))?;
        let mut hasher = Sha256::new();
        let mut buf = vec![0u8; 64 * 1024];
        let mut done = 0u64;
        loop {
            if cancel.load(Ordering::SeqCst) {
                return Err("Cancelled".to_string());
            }
            let n = resp.read(&mut buf).map_err(|e| format!("Download failed: {e}"))?;
            if n == 0 {
                break;
            }
            file.write_all(&buf[..n]).map_err(|e| format!("Couldn't save the update: {e}"))?;
            hasher.update(&buf[..n]);
            done += n as u64;
            progress(done);
        }
        file.flush().map_err(|e| format!("Couldn't save the update: {e}"))?;
        if done != asset.size {
            return Err(format!("Download incomplete ({done} of {} bytes)", asset.size));
        }
        if let Some(want) = asset.digest.as_deref().and_then(|d| d.strip_prefix("sha256:")) {
            let got: String = hasher.finalize().iter().map(|b| format!("{b:02x}")).collect();
            if !got.eq_ignore_ascii_case(want) {
                return Err("The download doesn't match GitHub's checksum, so it wasn't run".into());
            }
        }
        Ok(())
    })();
    if let Err(e) = res {
        let _ = std::fs::remove_file(&part);
        return Err(e);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&part, std::fs::Permissions::from_mode(0o755)).map_err(|e| e.to_string())?;
    }
    // over the running AppImage too: Linux keeps the old file alive for the running process
    std::fs::rename(&part, appimage_path().unwrap_or_else(|| dest.to_path_buf())).map_err(|e| format!("Couldn't put the update in place: {e}"))
}

/// Starts the installer (Windows) or the new AppImage (Linux).
fn launch(downloaded: &Path) -> Result<(), String> {
    let target = appimage_path().unwrap_or_else(|| downloaded.to_path_buf());
    let mut cmd = std::process::Command::new(&target);
    match cmd.spawn() {
        Ok(_) => Ok(()),
        // the setup wants admin rights: let the shell raise the UAC prompt
        #[cfg(windows)]
        Err(e) if e.raw_os_error() == Some(740) => {
            use std::os::windows::process::CommandExt;
            std::process::Command::new("cmd")
                .args(["/C", "start", ""])
                .arg(&target)
                .creation_flags(0x0800_0000) // CREATE_NO_WINDOW
                .spawn()
                .map(|_| ())
                .map_err(|e| format!("Couldn't start the installer: {e}"))
        }
        Err(e) => Err(format!("Couldn't start the update: {e}")),
    }
}

/// The AppImage this process runs from, if any (set by the AppImage runtime).
fn appimage_path() -> Option<PathBuf> {
    if !cfg!(target_os = "linux") {
        return None;
    }
    std::env::var_os("APPIMAGE").map(PathBuf::from).filter(|p| p.is_file())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions() {
        assert!(is_newer("3.1.0", "3.0.0"));
        assert!(is_newer("v3.10.0", "3.9.9"));
        assert!(is_newer("4", "3.9"));
        assert!(!is_newer("3.0.0", "3.0.0"));
        assert!(!is_newer("v3.0", "3.0.0"));
        assert!(!is_newer("2.9.9", "3.0.0"));
        assert!(!is_newer("3.1.0-beta", "3.1.0"));
    }

    /// `cargo test -- --ignored`: fetches the latest release and downloads its Windows setup twice,
    /// once with GitHub's digest and once with a wrong one.
    #[test]
    #[ignore = "network: downloads a release file from GitHub"]
    fn downloads_and_verifies_a_release_file() {
        let release = latest().expect("latest release");
        let mut asset = Platform::Windows.pick(&release.assets).expect("setup.exe in the release");
        assert!(asset.digest.as_deref().is_some_and(|d| d.starts_with("sha256:")), "GitHub sent no digest");
        let dir = std::env::temp_dir().join("fastbet-update-test");
        std::fs::create_dir_all(&dir).unwrap();
        let dest = dir.join(&asset.name);
        download(&asset, &dest, &AtomicBool::new(false), |_| {}).expect("download");
        assert_eq!(std::fs::metadata(&dest).unwrap().len(), asset.size);

        asset.digest = Some(format!("sha256:{}", "0".repeat(64)));
        let _ = std::fs::remove_file(&dest);
        let err = download(&asset, &dest, &AtomicBool::new(false), |_| {}).unwrap_err();
        assert!(err.contains("checksum"), "{err}");
        assert!(!dest.exists(), "a file that failed the checksum was kept");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn picks_the_file_for_each_system() {
        let a = |name: &str| Asset { name: name.into(), size: 1, browser_download_url: String::new(), digest: None };
        let assets = vec![a("fastbet_3.1.0_amd64.deb"), a("FastBet_3.1.0_x64-portable.exe"), a("fastbet_3.1.0_x64-setup.exe"), a("fastbet_3.1.0_x86_64.AppImage")];
        assert_eq!(Platform::Windows.pick(&assets).unwrap().name, "fastbet_3.1.0_x64-setup.exe");
        assert_eq!(Platform::AppImage.pick(&assets).unwrap().name, "fastbet_3.1.0_x86_64.AppImage");
        assert_eq!(Platform::Deb.pick(&assets).unwrap().name, "fastbet_3.1.0_amd64.deb");
        assert!(Platform::Other.pick(&assets).is_none());
    }
}
