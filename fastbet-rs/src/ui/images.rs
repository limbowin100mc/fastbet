//! Async image cache for game tiles: downloads on worker threads, decodes
//! with `image`, keeps a disk copy, hands egui textures to the UI.

use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::mpsc;

use egui::{ColorImage, TextureHandle, TextureOptions};

enum Img {
    Loading,
    Ready(TextureHandle),
    Failed,
}

pub struct ImageCache {
    map: HashMap<String, Img>,
    queue: VecDeque<String>,
    inflight: usize,
    tx: mpsc::Sender<(String, Result<ColorImage, String>)>,
    rx: mpsc::Receiver<(String, Result<ColorImage, String>)>,
    dir: Option<PathBuf>,
}

const MAX_INFLIGHT: usize = 6;
const MAX_W: u32 = 320;

impl ImageCache {
    pub fn new() -> Self {
        let (tx, rx) = mpsc::channel();
        let dir = directories::ProjectDirs::from("gg", "fastbet", "FastBet").map(|d| d.cache_dir().join("images"));
        if let Some(d) = &dir {
            let _ = std::fs::create_dir_all(d);
        }
        Self { map: HashMap::new(), queue: VecDeque::new(), inflight: 0, tx, rx, dir }
    }

    /// Returns the texture when loaded; starts loading on first call.
    pub fn get(&mut self, ctx: &egui::Context, url: &str) -> Option<TextureHandle> {
        self.drain(ctx);
        if url.is_empty() {
            return None;
        }
        match self.map.get(url) {
            Some(Img::Ready(t)) => return Some(t.clone()),
            Some(_) => return None,
            None => {}
        }
        self.map.insert(url.to_string(), Img::Loading);
        self.queue.push_back(url.to_string());
        self.pump();
        None
    }

    pub fn failed(&self, url: &str) -> bool {
        matches!(self.map.get(url), Some(Img::Failed))
    }

    fn pump(&mut self) {
        while self.inflight < MAX_INFLIGHT {
            let Some(url) = self.queue.pop_front() else { break };
            self.inflight += 1;
            let tx = self.tx.clone();
            let dir = self.dir.clone();
            std::thread::spawn(move || {
                let res = load(&url, dir.as_deref());
                let _ = tx.send((url, res));
            });
        }
    }

    fn drain(&mut self, ctx: &egui::Context) {
        let mut got = false;
        while let Ok((url, res)) = self.rx.try_recv() {
            self.inflight = self.inflight.saturating_sub(1);
            got = true;
            let state = match res {
                Ok(img) => Img::Ready(ctx.load_texture(url.clone(), img, TextureOptions::LINEAR)),
                Err(e) => {
                    tracing::debug!("image {url}: {e}");
                    Img::Failed
                }
            };
            self.map.insert(url, state);
        }
        if got {
            self.pump();
            ctx.request_repaint();
        }
    }
}

fn cache_name(url: &str) -> String {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    url.hash(&mut h);
    format!("{:016x}.bin", h.finish())
}

fn load(url: &str, dir: Option<&std::path::Path>) -> Result<ColorImage, String> {
    let path = dir.map(|d| d.join(cache_name(url)));
    let bytes = match path.as_ref().and_then(|p| std::fs::read(p).ok()) {
        Some(b) => b,
        None => {
            let client = reqwest::blocking::Client::builder()
                .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 Chrome/154.0.0.0 Safari/537.36")
                .timeout(std::time::Duration::from_secs(20))
                .build()
                .map_err(|e| e.to_string())?;
            let res = client.get(url).send().map_err(|e| e.to_string())?;
            if !res.status().is_success() {
                return Err(format!("HTTP {}", res.status()));
            }
            let b = res.bytes().map_err(|e| e.to_string())?.to_vec();
            if let Some(p) = &path {
                let _ = std::fs::write(p, &b);
            }
            b
        }
    };
    let img = image::load_from_memory(&bytes).map_err(|e| e.to_string())?;
    let img = if img.width() > MAX_W { img.resize(MAX_W, MAX_W * 4, image::imageops::FilterType::Triangle) } else { img };
    let rgba = img.to_rgba8();
    let (w, h) = rgba.dimensions();
    Ok(ColorImage::from_rgba_unmultiplied([w as usize, h as usize], rgba.as_raw()))
}
