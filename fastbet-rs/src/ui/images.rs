//! Async image cache for game tiles: downloads on worker threads, decodes
//! with `image`, keeps a disk copy, hands egui textures to the UI.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::mpsc;

use egui::emath::GuiRounding as _;
use egui::{pos2, vec2, Color32, ColorImage, CornerRadius, Rect, TextureHandle, TextureOptions};

/// A picture to make: its key in the cache, where it comes from, and the size in pixels it is cut
/// and scaled to.
type Job = (String, String, [u32; 2]);
type Done = (String, String, Result<ColorImage, String>);

enum Img {
    Loading,
    Ready(TextureHandle),
    Failed,
}

pub struct ImageCache {
    map: HashMap<String, Img>,
    /// URLs that didn't load, at any size.
    failed: HashSet<String>,
    queue: VecDeque<Job>,
    inflight: usize,
    tx: mpsc::Sender<Done>,
    rx: mpsc::Receiver<Done>,
    dir: Option<PathBuf>,
}

const MAX_INFLIGHT: usize = 6;

impl ImageCache {
    pub fn new() -> Self {
        let (tx, rx) = mpsc::channel();
        let dir = cache_dir();
        if let Some(d) = &dir {
            let _ = std::fs::create_dir_all(d);
        }
        Self { map: HashMap::new(), failed: HashSet::new(), queue: VecDeque::new(), inflight: 0, tx, rx, dir }
    }

    /// The picture cut to fill `size` pixels (its middle: art of another shape loses its sides,
    /// or its top and bottom) and scaled with a sharp filter, to draw pixel for pixel at that
    /// size. Returns the texture when loaded; starts loading on first call.
    pub fn get_fit(&mut self, ctx: &egui::Context, url: &str, size: [u32; 2]) -> Option<TextureHandle> {
        self.drain(ctx);
        if url.is_empty() {
            return None;
        }
        let key = format!("{url}#{}x{}", size[0], size[1]);
        match self.map.get(&key) {
            Some(Img::Ready(t)) => return Some(t.clone()),
            Some(_) => return None,
            None => {}
        }
        self.map.insert(key.clone(), Img::Loading);
        self.queue.push_back((key, url.to_string(), size));
        self.pump();
        None
    }

    /// Paints `url` filling `rect` (put on the pixel grid with [`snap`]) pixel for pixel. False
    /// while it loads, or when it can't.
    pub fn paint_fit(&mut self, p: &egui::Painter, rect: Rect, url: &str, radius: u8) -> bool {
        let ppp = p.ctx().pixels_per_point();
        let size = [(rect.width() * ppp).round() as u32, (rect.height() * ppp).round() as u32];
        if size[0] == 0 || size[1] == 0 {
            return false;
        }
        let Some(tex) = self.get_fit(p.ctx(), url, size) else { return false };
        let uv = Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0));
        p.add(egui::epaint::RectShape::filled(rect, CornerRadius::same(radius), Color32::WHITE).with_texture(tex.id(), uv));
        true
    }

    pub fn failed(&self, url: &str) -> bool {
        self.failed.contains(url)
    }

    fn pump(&mut self) {
        while self.inflight < MAX_INFLIGHT {
            let Some((key, url, size)) = self.queue.pop_front() else { break };
            self.inflight += 1;
            let tx = self.tx.clone();
            let dir = self.dir.clone();
            std::thread::spawn(move || {
                let res = load(&url, dir.as_deref(), size);
                let _ = tx.send((key, url, res));
            });
        }
    }

    fn drain(&mut self, ctx: &egui::Context) {
        let mut got = false;
        while let Ok((key, url, res)) = self.rx.try_recv() {
            self.inflight = self.inflight.saturating_sub(1);
            got = true;
            let state = match res {
                Ok(img) => Img::Ready(ctx.load_texture(key.clone(), img, TextureOptions::LINEAR)),
                Err(e) => {
                    tracing::debug!("image {url}: {e}");
                    self.failed.insert(url);
                    Img::Failed
                }
            };
            self.map.insert(key, state);
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

fn cache_dir() -> Option<PathBuf> {
    crate::paths::cache_dir().map(|d| d.join("images"))
}

/// The image's file, from the disk cache or downloaded (and then cached). Blocks: call it off
/// the UI thread.
pub fn bytes(url: &str) -> Option<Vec<u8>> {
    let dir = cache_dir();
    if let Some(d) = &dir {
        let _ = std::fs::create_dir_all(d);
    }
    load_bytes(url, dir.as_deref()).ok()
}

/// `rect` moved onto the pixel grid, its size kept to whole pixels, so a picture made for it with
/// [`ImageCache::get_fit`] lands pixel for pixel.
pub fn snap(rect: Rect, ppp: f32) -> Rect {
    let px = |v: f32| (v * ppp).round() / ppp;
    Rect::from_min_size(rect.min.round_to_pixels(ppp), vec2(px(rect.width()), px(rect.height())))
}

fn load(url: &str, dir: Option<&std::path::Path>, [w, h]: [u32; 2]) -> Result<ColorImage, String> {
    let bytes = load_bytes(url, dir)?;
    let img = image::load_from_memory(&bytes).map_err(|e| e.to_string())?;
    let img = img.resize_to_fill(w, h, image::imageops::FilterType::Lanczos3);
    let rgba = img.to_rgba8();
    let (w, h) = rgba.dimensions();
    Ok(ColorImage::from_rgba_unmultiplied([w as usize, h as usize], rgba.as_raw()))
}

/// Up to `n` portrait pictures from the disk cache, as URLs the cache loads: game tiles to look
/// at without a casino (FASTBET_PREVIEW_ART, debug builds).
pub fn cached_portraits(n: usize) -> Vec<String> {
    let Some(dir) = cache_dir() else { return vec![] };
    let mut files: Vec<PathBuf> = std::fs::read_dir(&dir).into_iter().flatten().flatten().map(|e| e.path()).collect();
    files.sort();
    let portrait = |p: &PathBuf| {
        let size = image::ImageReader::open(p).ok().and_then(|r| r.with_guessed_format().ok()).and_then(|r| r.into_dimensions().ok());
        size.is_some_and(|(w, h)| w >= 200 && h * 10 > w * 12)
    };
    files.into_iter().filter(portrait).take(n).map(|p| format!("file://{}", p.display())).collect()
}

fn load_bytes(url: &str, dir: Option<&std::path::Path>) -> Result<Vec<u8>, String> {
    if cfg!(debug_assertions) {
        if let Some(file) = url.strip_prefix("file://") {
            return std::fs::read(file).map_err(|e| e.to_string());
        }
    }
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
    Ok(bytes)
}
