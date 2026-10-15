//! Embedded casino login.
//!
//! The login page opens in a WebView2 window that runs as a *child process*
//! (`fastbet --login <casino>`), because the webview needs its own event loop
//! on a main thread and egui already owns ours. The child watches the page's
//! localStorage and prints every new `tokenid` it sees as `TOKEN <value>` on
//! stdout. The parent verifies each token against the casino API and kills
//! the child as soon as one logs in.

use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::{mpsc, Arc, Mutex};

use crate::casino::CasinoId;

const TOKEN_PREFIX: &str = "TOKEN ";

/// Where the webview keeps cookies, so the next login is instant. One profile per account
/// (named after it), so two accounts on one casino never share a site session.
fn profile_dir(profile: &str) -> Option<PathBuf> {
    let dirs = directories::ProjectDirs::from("gg", "fastbet", "FastBet")?;
    let safe: String = profile.chars().filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_')).collect();
    (!safe.is_empty()).then(|| dirs.data_local_dir().join("webview").join(safe))
}

/// Forgets a login window profile's cookies and site storage, so the next login starts logged
/// out (to log in to another account). Only while no login window is open.
pub fn clear_profile(profile: &str) -> Result<(), String> {
    let Some(dir) = profile_dir(profile) else { return Ok(()) };
    match std::fs::remove_dir_all(&dir) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(format!("Could not clear the cookies: {e}")),
    }
}

/// Puts a webview into a tao window. On Linux a plain `build` only works under
/// X11, so it goes through the window's GTK box to work under Wayland too.
pub(crate) fn attach<'a>(builder: wry::WebViewBuilder<'a>, window: &'a tao::window::Window) -> wry::Result<wry::WebView> {
    #[cfg(target_os = "linux")]
    {
        use tao::platform::unix::WindowExtUnix;
        use wry::WebViewBuilderExtUnix;
        let vbox = window.default_vbox().ok_or(wry::Error::UnsupportedWindowHandle)?;
        builder.build_gtk(vbox)
    }
    #[cfg(not(target_os = "linux"))]
    builder.build(window)
}

/// The AppImage's GTK hook forces `GDK_BACKEND=x11`, and GTK can't start at all on a Wayland
/// desktop without XWayland (niri, some Hyprland setups): the login window never appeared. There
/// GTK goes back to Wayland. Runs before any thread or GTK window exists; children inherit it.
pub fn pick_gtk_backend() {
    #[cfg(target_os = "linux")]
    {
        let set = |k: &str| std::env::var_os(k).is_some_and(|v| !v.is_empty());
        if std::env::var("GDK_BACKEND").as_deref() == Ok("x11") && !set("DISPLAY") && set("WAYLAND_DISPLAY") {
            std::env::set_var("GDK_BACKEND", "wayland");
        }
    }
}

// ---------------------------------------------------------------- child side

/// Runs the login window on `url` (the casino's own login page if `None`) in the browser
/// profile `profile` (the casino's key if `None`). Never returns on success; the parent kills us.
pub fn run_window(id: CasinoId, url: Option<&str>, profile: Option<&str>) -> Result<(), String> {
    use tao::dpi::LogicalSize;
    use tao::event::{Event, WindowEvent};
    use tao::event_loop::{ControlFlow, EventLoopBuilder};
    use tao::window::WindowBuilder;
    use wry::{WebContext, WebViewBuilder};

    enum UserEvent {
        Token(String),
    }

    let event_loop = EventLoopBuilder::<UserEvent>::with_user_event().build();
    let proxy = event_loop.create_proxy();
    let window = WindowBuilder::new()
        .with_title(format!("Log in to {} — FastBet", id.name()))
        .with_inner_size(LogicalSize::new(1120.0, 840.0))
        .build(&event_loop)
        .map_err(|e| e.to_string())?;

    // FASTBET_WATCH_SCRIPT overrides the injected script, handy when adding a casino.
    let script = std::env::var("FASTBET_WATCH_SCRIPT").unwrap_or_else(|_| id.token_watch_script().to_string());
    let mut ctx = WebContext::new(profile_dir(profile.unwrap_or(id.key())));
    let builder = WebViewBuilder::new_with_web_context(&mut ctx)
        .with_url(url.map(str::to_string).unwrap_or_else(|| id.login_url(None)))
        .with_initialization_script(script)
        .with_devtools(true)
        .with_ipc_handler(move |req| {
            let _ = proxy.send_event(UserEvent::Token(req.body().clone()));
        });
    let webview = attach(builder, &window).map_err(|e| e.to_string())?;

    let mut last = String::new();
    let mut misses = 0u32;
    event_loop.run(move |event, _, control_flow| {
        *control_flow = ControlFlow::Wait;
        match event {
            Event::WindowEvent { event: WindowEvent::CloseRequested, .. } => *control_flow = ControlFlow::Exit,
            Event::UserEvent(UserEvent::Token(t)) => {
                let t = match (id.login_cookie(), id.cookie_login()) {
                    (Some((url, name)), _) => with_cookie(&webview, &t, url, name, &mut misses),
                    (None, Some(_)) if id.cookie_needs_page_login() && !reports_login(&t) => serde_json::json!({ "token": "", "loggedIn": false }).to_string(),
                    (None, Some((url, name))) => cookie_token(&webview, url, name),
                    (None, None) => t,
                };
                let t = t.trim().to_string();
                if !t.is_empty() && t != last {
                    last = t.clone();
                    println!("{TOKEN_PREFIX}{t}");
                    let _ = std::io::stdout().flush();
                }
            }
            _ => {}
        }
    })
}

/// A logged-in report whose token is the access token -> one carrying the pair, with the refresh
/// token taken from the browser's cookie `name` (HttpOnly: the page can't read it). Until the
/// cookie is there the report says "not yet"; after ~10 reports without it, the access token goes
/// alone (it works until it runs out).
fn with_cookie(webview: &wry::WebView, report: &str, url: &str, name: &str, misses: &mut u32) -> String {
    let Ok(mut v) = serde_json::from_str::<serde_json::Value>(report) else { return report.to_string() };
    let access = v["token"].as_str().unwrap_or("").to_string();
    if v["loggedIn"].as_bool() != Some(true) || access.is_empty() {
        *misses = 0;
        return serde_json::json!({ "token": "", "loggedIn": false }).to_string();
    }
    let refresh = webview.cookies_for_url(url).ok().and_then(|cs| cs.iter().find(|c| c.name() == name).map(|c| c.value().to_string()));
    let pair = match refresh {
        Some(r) => crate::casino::creds::Creds { access, refresh: r },
        None if *misses >= 10 => crate::casino::creds::Creds { access, refresh: String::new() },
        None => {
            *misses += 1;
            return serde_json::json!({ "token": "", "loggedIn": false }).to_string();
        }
    };
    v["token"] = serde_json::Value::String(pair.to_token());
    if let Some(o) = v.as_object_mut() {
        o.remove("tick");
    }
    v.to_string()
}

/// A login that is an HttpOnly cookie (Gambulls, Metaspins): the report is the cookie `name` as the token,
/// logged in once the browser holds it.
fn cookie_token(webview: &wry::WebView, url: &str, name: &str) -> String {
    let value = webview.cookies_for_url(url).ok().and_then(|cs| cs.iter().find(|c| c.name() == name).map(|c| c.value().to_string())).filter(|v| !v.is_empty());
    match value {
        Some(v) => serde_json::json!({ "token": v, "loggedIn": true }).to_string(),
        None => serde_json::json!({ "token": "", "loggedIn": false }).to_string(),
    }
}

/// The page's report says it is logged in.
fn reports_login(report: &str) -> bool {
    serde_json::from_str::<serde_json::Value>(report).is_ok_and(|v| v["loggedIn"].as_bool() == Some(true))
}

/// Traffic capture for reverse-engineering a slot provider: opens `url` in a
/// webview and prints every fetch / XHR / websocket exchange of every frame as
/// `CAP {json}` lines on stdout. `fastbet --capture <url>`.
pub fn run_capture(url: &str) -> Result<(), String> {
    use tao::dpi::LogicalSize;
    use tao::event::{Event, WindowEvent};
    use tao::event_loop::{ControlFlow, EventLoopBuilder};
    use tao::window::WindowBuilder;
    use wry::{WebContext, WebViewBuilder};

    enum UserEvent {
        Line(String),
    }
    let event_loop = EventLoopBuilder::<UserEvent>::with_user_event().build();
    let proxy = event_loop.create_proxy();
    let window = WindowBuilder::new()
        .with_title("Capture — FastBet")
        .with_inner_size(LogicalSize::new(1280.0, 860.0))
        .build(&event_loop)
        .map_err(|e| e.to_string())?;
    let mut ctx = WebContext::new(profile_dir(CasinoId::ChipsGg.key()).map(|p| p.join("capture")));
    // FASTBET_CAP_DRIVE=1 also clicks the game and presses Space every few seconds
    let inspect: Vec<String> = std::env::var("FASTBET_CAP_INSPECT").map(|s| s.split(',').map(|x| x.trim().to_string()).filter(|x| !x.is_empty()).collect()).unwrap_or_default();
    let base = CAPTURE_SCRIPT.replace("__FB_INSPECT__", &serde_json::to_string(&inspect).unwrap_or_else(|_| "[]".into()));
    let script = if std::env::var("FASTBET_CAP_DRIVE").is_ok() { format!("{base}\n{DRIVER_SCRIPT}") } else { base };
    let builder = WebViewBuilder::new_with_web_context(&mut ctx)
        .with_url(url)
        .with_initialization_script(script)
        .with_devtools(true)
        .with_ipc_handler(move |req| {
            let _ = proxy.send_event(UserEvent::Line(req.body().clone()));
        });
    let _webview = attach(builder, &window).map_err(|e| e.to_string())?;
    event_loop.run(move |event, _, control_flow| {
        *control_flow = ControlFlow::Wait;
        match event {
            Event::WindowEvent { event: WindowEvent::CloseRequested, .. } => *control_flow = ControlFlow::Exit,
            Event::UserEvent(UserEvent::Line(l)) => {
                println!("CAP {l}");
                let _ = std::io::stdout().flush();
            }
            _ => {}
        }
    })
}

const CAPTURE_SCRIPT: &str = r#"(function () {
  if (window.__fbcap) return;
  window.__fbcap = true;
  var LIM = 60000;
  function cut(s) { s = String(s == null ? '' : s); return s.length > LIM ? s.slice(0, LIM) + '…[' + s.length + ']' : s; }
  var JS0 = JSON.stringify;
  var inSend = false;
  function send(obj) {
    if (inSend) return;
    inSend = true;
    obj.frame = location.href.slice(0, 200);
    var line = JS0.call(JSON, obj);
    inSend = false;
    try { if (window.ipc && window.ipc.postMessage) { window.ipc.postMessage(line); return; } } catch (e) {}
    try { window.top.postMessage({ __fbcap: line }, '*'); } catch (e) {}
  }
  window.__fbsend = send;
  if (window.top === window) {
    window.addEventListener('message', function (ev) {
      try { if (ev.data && ev.data.__fbcap && window.ipc) window.ipc.postMessage(ev.data.__fbcap); } catch (e) {}
    });
  }
  // fetch
  var of = window.fetch;
  if (of) window.fetch = function (input, init) {
    var url = typeof input === 'string' ? input : (input && input.url) || '';
    var method = (init && init.method) || (input && input.method) || 'GET';
    var body = init && init.body;
    var reqHeaders = init && init.headers ? JSON.stringify(init.headers) : '';
    var t0 = Date.now();
    return of.apply(this, arguments).then(function (res) {
      try {
        var clone = res.clone();
        clone.text().then(function (txt) {
          send({ kind: 'fetch', t0: t0, method: method, url: url, reqHeaders: reqHeaders, req: cut(body), status: res.status, ms: Date.now() - t0, res: cut(txt) });
        }).catch(function () {});
      } catch (e) {}
      return res;
    });
  };
  // XHR
  var XO = XMLHttpRequest.prototype.open, XS = XMLHttpRequest.prototype.send, XH = XMLHttpRequest.prototype.setRequestHeader;
  XMLHttpRequest.prototype.open = function (m, u) { this.__m = m; this.__u = u; this.__h = {}; return XO.apply(this, arguments); };
  XMLHttpRequest.prototype.setRequestHeader = function (k, v) { try { this.__h[k] = v; } catch (e) {} return XH.apply(this, arguments); };
  XMLHttpRequest.prototype.send = function (body) {
    var x = this, t0 = Date.now();
    x.addEventListener('loadend', function () {
      var txt = '';
      try {
        if (x.responseType === '' || x.responseType === 'text') txt = x.responseText;
        else if (x.responseType === 'json') txt = JSON.stringify(x.response);
        else if (x.responseType === 'arraybuffer') {
          var bytes = new Uint8Array(x.response || new ArrayBuffer(0));
          var first = bytes.length ? bytes[0] : 0;
          if (first === 123 || first === 91 || first === 34 || (first >= 32 && first < 127 && bytes.length < 200000)) txt = new TextDecoder('utf-8').decode(bytes);
          else txt = '[binary ' + bytes.length + ']';
        }
        else txt = '[' + x.responseType + ']';
      } catch (e) {}
      send({ kind: 'xhr', t0: t0, method: x.__m, url: x.__u, reqHeaders: JSON.stringify(x.__h || {}), req: cut(body), status: x.status, ms: Date.now() - t0, res: cut(txt) });
    });
    return XS.apply(this, arguments);
  };
  // plaintext on both sides of any encryption / compression layer
  var JS = JSON.stringify, JP = JSON.parse;
  JSON.stringify = function (v) {
    var r = JS.apply(JSON, arguments);
    try { if (!inSend && typeof r === 'string' && r.length > 8 && r.length < 20000 && r.charAt(0) === '{' && r.indexOf('"kind":"') !== 0) send({ kind: 'json-out', req: cut(r) }); } catch (e) {}
    return r;
  };
  JSON.parse = function (s) {
    var r = JP.apply(JSON, arguments);
    try { if (!inSend && typeof s === 'string' && s.length > 8 && s.length < 60000 && (s.charAt(0) === '{' || s.charAt(0) === '[')) send({ kind: 'json-in', res: cut(s) }); } catch (e) {}
    return r;
  };
  // deep-ish dump of named globals (API discovery): FASTBET_CAP_INSPECT=a,b,c
  var inspectNames = __FB_INSPECT__;
  function describe(obj, depth) {
    var out = {};
    try {
      var names = Object.getOwnPropertyNames(obj);
      if (Object.getPrototypeOf(obj) && Object.getPrototypeOf(obj) !== Object.prototype) names = names.concat(Object.getOwnPropertyNames(Object.getPrototypeOf(obj)));
      names.slice(0, 120).forEach(function (k) {
        try {
          var v = obj[k];
          var t = typeof v;
          if (t === 'function') out[k] = 'fn(' + v.length + ')';
          else if (v && t === 'object' && depth > 0) out[k] = describe(v, depth - 1);
          else if (t === 'object') out[k] = v === null ? 'null' : (Array.isArray(v) ? 'array[' + v.length + ']' : 'object');
          else out[k] = t + ':' + String(v).slice(0, 60);
        } catch (e) { out[k] = '?'; }
      });
    } catch (e) {}
    return out;
  }
  setTimeout(function () {
    inspectNames.forEach(function (n) {
      try { if (window[n] !== undefined) send({ kind: 'inspect', url: n, res: JS0.call(JSON, describe(window[n], 2)).slice(0, 20000) }); } catch (e) {}
    });
  }, 30000);
  // globals the page adds (API discovery), once
  setTimeout(function () {
    try {
      var f = document.createElement('iframe'); f.style.display = 'none'; document.documentElement.appendChild(f);
      var base = new Set(Object.getOwnPropertyNames(f.contentWindow));
      var extra = Object.getOwnPropertyNames(window).filter(function (k) { return !base.has(k) && k.indexOf('__fb') !== 0; });
      f.remove();
      send({ kind: 'globals', res: JSON.stringify(extra.slice(0, 300)) });
    } catch (e) {}
  }, 25000);
  // websockets (binary frames as base64: "b64:…")
  var NW = window.WebSocket;
  function wsData(d) {
    try {
      var b = d instanceof ArrayBuffer ? new Uint8Array(d) : (ArrayBuffer.isView(d) ? new Uint8Array(d.buffer, d.byteOffset, d.byteLength) : null);
      if (b) { var s = ''; for (var i = 0; i < Math.min(b.length, LIM); i++) s += String.fromCharCode(b[i]); return 'b64:' + btoa(s); }
    } catch (e) {}
    return cut(d);
  }
  function W(u, p) {
    var ws = p === undefined ? new NW(u) : new NW(u, p);
    send({ kind: 'ws-open', url: String(u) });
    var os = ws.send;
    ws.send = function (d) { send({ kind: 'ws-send', url: String(u), req: wsData(d) }); return os.apply(ws, arguments); };
    ws.addEventListener('message', function (ev) { send({ kind: 'ws-recv', url: String(u), res: wsData(ev.data) }); });
    return ws;
  }
  W.prototype = NW.prototype; W.CONNECTING = 0; W.OPEN = 1; W.CLOSING = 2; W.CLOSED = 3;
  window.WebSocket = W;
  send({ kind: 'frame', url: location.href });
})();"#;

const DRIVER_SCRIPT: &str = r#"(function () {
  if (window.__fbdrv) return;
  window.__fbdrv = true;
  var n = 0;
  setInterval(function () {
    var c = document.querySelector('canvas');
    if (!c) return;
    n++;
    if (n < 8) return;
    var r = c.getBoundingClientRect();
    // alternate between a few spots: centre, the left/right thirds and lower middle
    // (intro screens with buttons, spin buttons at the bottom)
    var spots = [[0.5, 0.5], [0.36, 0.62], [0.5, 0.86], [0.64, 0.62], [0.75, 0.88]];
    var sp = spots[n % spots.length];
    var x = r.left + r.width * sp[0], y = r.top + r.height * sp[1];
    function pe(type) { try { c.dispatchEvent(new PointerEvent(type, { bubbles: true, cancelable: true, clientX: x, clientY: y, pointerId: 1, pointerType: 'mouse', isPrimary: true, button: 0, buttons: type === 'pointerup' ? 0 : 1 })); } catch (e) {} }
    function me(type) { try { c.dispatchEvent(new MouseEvent(type, { bubbles: true, cancelable: true, clientX: x, clientY: y, button: 0 })); } catch (e) {} }
    pe('pointerdown'); me('mousedown'); pe('pointerup'); me('mouseup'); me('click');
    ['keydown', 'keyup'].forEach(function (t) {
      try {
        var ev = new KeyboardEvent(t, { bubbles: true, cancelable: true, key: ' ', code: 'Space', keyCode: 32, which: 32 });
        window.dispatchEvent(ev); document.dispatchEvent(ev); c.dispatchEvent(ev);
      } catch (e) {}
    });
    if (window.__fbsend) window.__fbsend({ kind: 'drive', n: n });
  }, 2500);
})();"#;

// --------------------------------------------------------------- parent side

/// What the login window reports: the token in the page and whether the page
/// is logged in as a user.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Deserialize)]
pub struct LoginReport {
    #[serde(default)]
    pub token: String,
    #[serde(default, rename = "loggedIn")]
    pub logged_in: bool,
}

pub struct LoginSession {
    child: Child,
    rx: mpsc::Receiver<LoginReport>,
    /// The window's last stderr lines, to say why it closed if it crashed.
    errors: Arc<Mutex<VecDeque<String>>>,
    /// The last token we handed to the engine for verification.
    pub tried: Option<String>,
    /// Latest report from the window.
    pub last: LoginReport,
}

impl LoginSession {
    /// Opens the login window on `url`, in the account's own browser profile.
    pub fn spawn(id: CasinoId, url: &str, profile: &str, repaint: Arc<dyn Fn() + Send + Sync>) -> Result<Self, String> {
        let exe = std::env::current_exe().map_err(|e| e.to_string())?;
        let mut cmd = Command::new(exe);
        cmd.arg("--login").arg(id.key()).arg(url).arg(profile).stdout(Stdio::piped()).stderr(Stdio::piped()).stdin(Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            cmd.creation_flags(CREATE_NO_WINDOW);
        }
        let mut child = cmd.spawn().map_err(|e| format!("Could not open the login window: {e}"))?;
        let stdout = child.stdout.take().ok_or("no stdout")?;
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                let Some(body) = line.strip_prefix(TOKEN_PREFIX) else { continue };
                // JSON report, or (for a plain script) just the token
                let report = serde_json::from_str::<LoginReport>(body)
                    .unwrap_or_else(|_| LoginReport { token: body.trim().to_string(), logged_in: false });
                if tx.send(report).is_err() {
                    break;
                }
                repaint();
            }
        });
        // passed on to our stderr (seen when the app runs from a terminal), the tail kept
        let errors = Arc::new(Mutex::new(VecDeque::new()));
        if let Some(stderr) = child.stderr.take() {
            let errors = errors.clone();
            std::thread::spawn(move || {
                for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                    let _ = writeln!(std::io::stderr(), "[login window] {line}");
                    let mut e = errors.lock().unwrap();
                    if e.len() == 40 {
                        e.pop_front();
                    }
                    e.push_back(line);
                }
            });
        }
        Ok(Self { child, rx, errors, tried: None, last: LoginReport::default() })
    }

    /// Reports the login window sent since the last poll (latest last).
    pub fn poll(&mut self) -> Vec<LoginReport> {
        let v: Vec<LoginReport> = self.rx.try_iter().collect();
        if let Some(l) = v.last() {
            self.last = l.clone();
        }
        v
    }

    /// True once the user closed the login window.
    pub fn finished(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(Some(_)))
    }

    /// Once the window is gone: why, if it crashed or never opened (closing it by hand exits
    /// cleanly). The last thing it printed, e.g. a GTK or WebKit error.
    pub fn crash(&mut self) -> Option<String> {
        let status = self.child.try_wait().ok().flatten()?;
        if status.success() {
            return None;
        }
        // the reader thread may still be draining the pipe
        std::thread::sleep(std::time::Duration::from_millis(100));
        let errors = self.errors.lock().unwrap();
        let line = errors
            .iter()
            .rev()
            .map(|l| l.trim())
            .find(|l| !l.is_empty() && !l.starts_with("note: run with") && !l.starts_with("stack backtrace"))
            .map(|l| l.trim_start_matches("login window failed: ").to_string());
        Some(match line {
            Some(l) => l.chars().take(300).collect(),
            None => format!("it exited with {status}"),
        })
    }

    pub fn close(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Drop for LoginSession {
    fn drop(&mut self) {
        self.close();
    }
}
