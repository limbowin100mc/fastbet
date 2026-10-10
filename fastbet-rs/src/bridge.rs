//! Browser bridge: a hidden WebView2 window parked on a provider's origin
//! that performs HTTP requests on our behalf. Slot providers sit behind
//! Cloudflare bot checks that reject plain HTTP clients but accept a real
//! browser engine, so every provider call goes through here.
//!
//! Child: `fastbet --bridge <page-url>`. Reads `REQ <id> <json>` lines on
//! stdin, runs `fetch` in the page, prints `RES <id> <json>` on stdout.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::sync::oneshot;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BridgeRequest {
    pub method: String,
    pub url: String,
    #[serde(default)]
    pub headers: Vec<(String, String)>,
    #[serde(default)]
    pub body: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct BridgeResponse {
    pub status: u16,
    #[serde(default)]
    pub body: String,
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub ms: f64,
}

// ---------------------------------------------------------------- child side

pub fn run_child(url: &str) -> Result<(), String> {
    use tao::dpi::LogicalSize;
    use tao::event::{Event, WindowEvent};
    use tao::event_loop::{ControlFlow, EventLoopBuilder};
    use tao::window::WindowBuilder;
    use wry::{WebContext, WebViewBuilder};

    enum UserEvent {
        Out(String),
        Eval(String),
    }
    let event_loop = EventLoopBuilder::<UserEvent>::with_user_event().build();
    let proxy = event_loop.create_proxy();
    let window = WindowBuilder::new()
        .with_title("FastBet bridge")
        .with_inner_size(LogicalSize::new(400.0, 300.0))
        .with_visible(std::env::var("FASTBET_BRIDGE_SHOW").is_ok())
        .build(&event_loop)
        .map_err(|e| e.to_string())?;

    let data_dir = directories::ProjectDirs::from("gg", "fastbet", "FastBet").map(|d| d.data_local_dir().join("webview").join("bridge"));
    let mut ctx = WebContext::new(data_dir);
    let builder = WebViewBuilder::new_with_web_context(&mut ctx)
        .with_url(url)
        .with_initialization_script(BRIDGE_SCRIPT)
        .with_ipc_handler({
            let proxy = proxy.clone();
            move |req| {
                let _ = proxy.send_event(UserEvent::Out(req.body().clone()));
            }
        });
    let webview = crate::login::attach(builder, &window).map_err(|e| e.to_string())?;

    // stdin -> event loop
    {
        let proxy = proxy.clone();
        std::thread::spawn(move || {
            let stdin = std::io::stdin();
            for line in stdin.lock().lines().map_while(Result::ok) {
                if let Some(rest) = line.strip_prefix("REQ ") {
                    // rest = "<id> <json>"
                    if let Some((id, json)) = rest.split_once(' ') {
                        let js = format!("window.__fbreq({id}, {json});");
                        if proxy.send_event(UserEvent::Eval(js)).is_err() {
                            break;
                        }
                    }
                } else if line.trim() == "QUIT" {
                    std::process::exit(0);
                }
            }
            std::process::exit(0);
        });
    }

    event_loop.run(move |event, _, control_flow| {
        *control_flow = ControlFlow::Wait;
        match event {
            Event::WindowEvent { event: WindowEvent::CloseRequested, .. } => *control_flow = ControlFlow::Exit,
            Event::UserEvent(UserEvent::Out(line)) => {
                println!("{line}");
                let _ = std::io::stdout().flush();
            }
            Event::UserEvent(UserEvent::Eval(js)) => {
                let _ = webview.evaluate_script(&js);
            }
            _ => {}
        }
    })
}

const BRIDGE_SCRIPT: &str = r#"(function () {
  if (window.top !== window || window.__fbreq) return;
  function out(line) { try { window.ipc.postMessage(line); } catch (e) {} }
  window.__fbreq = function (id, req) {
    var t0 = Date.now();
    var init = { method: req.method || 'GET', headers: {} };
    (req.headers || []).forEach(function (h) { init.headers[h[0]] = h[1]; });
    if (req.body != null) init.body = req.body;
    fetch(req.url, init).then(function (r) {
      return r.text().then(function (t) {
        out('RES ' + id + ' ' + JSON.stringify({ status: r.status, body: t, ms: Date.now() - t0 }));
      });
    }).catch(function (e) {
      out('RES ' + id + ' ' + JSON.stringify({ status: 0, body: '', error: String(e), ms: Date.now() - t0 }));
    });
  };
  document.addEventListener('DOMContentLoaded', function () { out('READY'); });
  if (document.readyState !== 'loading') out('READY');
})();"#;

// --------------------------------------------------------------- parent side

type Pending = Arc<Mutex<HashMap<u64, oneshot::Sender<BridgeResponse>>>>;

/// A running bridge process. Cloning shares it.
#[derive(Clone)]
pub struct Bridge {
    inner: Arc<Inner>,
}

struct Inner {
    child: Mutex<Child>,
    stdin: Mutex<std::process::ChildStdin>,
    pending: Pending,
    seq: AtomicU64,
}

impl Bridge {
    /// Spawns the child and waits until the page has loaded.
    pub async fn open(page_url: &str) -> Result<Self, String> {
        let exe = std::env::current_exe().map_err(|e| e.to_string())?;
        let mut cmd = Command::new(exe);
        cmd.arg("--bridge").arg(page_url).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
        }
        let mut child = cmd.spawn().map_err(|e| format!("Could not start the browser bridge: {e}"))?;
        let stdin = child.stdin.take().ok_or("no stdin")?;
        let stdout = child.stdout.take().ok_or("no stdout")?;
        let pending: Pending = Arc::new(Mutex::new(HashMap::new()));
        let (ready_tx, ready_rx) = oneshot::channel::<()>();
        {
            let pending = pending.clone();
            std::thread::spawn(move || {
                let mut ready = Some(ready_tx);
                for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                    if line == "READY" {
                        if let Some(t) = ready.take() {
                            let _ = t.send(());
                        }
                    } else if let Some(rest) = line.strip_prefix("RES ") {
                        if let Some((id, json)) = rest.split_once(' ') {
                            if let (Ok(id), Ok(res)) = (id.parse::<u64>(), serde_json::from_str::<BridgeResponse>(json)) {
                                if let Some(tx) = pending.lock().unwrap().remove(&id) {
                                    let _ = tx.send(res);
                                }
                            }
                        }
                    }
                }
                // child gone: fail everything still waiting
                for (_, tx) in pending.lock().unwrap().drain() {
                    let _ = tx.send(BridgeResponse { status: 0, error: Some("bridge closed".into()), ..Default::default() });
                }
            });
        }
        let bridge = Self {
            inner: Arc::new(Inner { child: Mutex::new(child), stdin: Mutex::new(stdin), pending, seq: AtomicU64::new(1) }),
        };
        match tokio::time::timeout(Duration::from_secs(45), ready_rx).await {
            Ok(Ok(())) => Ok(bridge),
            _ => {
                bridge.close();
                Err("The browser bridge did not come up".into())
            }
        }
    }

    pub async fn request(&self, req: BridgeRequest) -> Result<BridgeResponse, String> {
        let id = self.inner.seq.fetch_add(1, Ordering::SeqCst);
        let (tx, rx) = oneshot::channel();
        self.inner.pending.lock().unwrap().insert(id, tx);
        let line = format!("REQ {id} {}\n", serde_json::to_string(&req).map_err(|e| e.to_string())?);
        {
            let mut stdin = self.inner.stdin.lock().unwrap();
            stdin.write_all(line.as_bytes()).and_then(|_| stdin.flush()).map_err(|e| format!("bridge write: {e}"))?;
        }
        match tokio::time::timeout(Duration::from_secs(30), rx).await {
            Ok(Ok(res)) => match res.error {
                Some(e) => Err(e),
                None => Ok(res),
            },
            Ok(Err(_)) => Err("bridge closed".into()),
            Err(_) => {
                self.inner.pending.lock().unwrap().remove(&id);
                Err("bridge timeout".into())
            }
        }
    }

    /// POST JSON, expect JSON back.
    pub async fn post_json(&self, url: &str, headers: &[(&str, &str)], body: &serde_json::Value) -> Result<(u16, serde_json::Value), String> {
        let mut h: Vec<(String, String)> = headers.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        h.push(("Content-Type".into(), "application/json".into()));
        let res = self.request(BridgeRequest { method: "POST".into(), url: url.into(), headers: h, body: Some(body.to_string()) }).await?;
        let v = serde_json::from_str(&res.body).map_err(|_| format!("HTTP {}: {}", res.status, res.body.chars().take(200).collect::<String>()))?;
        Ok((res.status, v))
    }

    pub fn close(&self) {
        if let Ok(mut stdin) = self.inner.stdin.lock() {
            let _ = stdin.write_all(b"QUIT\n");
            let _ = stdin.flush();
        }
        if let Ok(mut c) = self.inner.child.lock() {
            let _ = c.kill();
            let _ = c.wait();
        }
    }
}

impl Drop for Inner {
    fn drop(&mut self) {
        if let Ok(mut c) = self.child.lock() {
            let _ = c.kill();
            let _ = c.wait();
        }
    }
}
