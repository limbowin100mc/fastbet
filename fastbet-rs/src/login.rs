//! Embedded casino login.
//!
//! The login page opens in a WebView2 window that runs as a *child process*
//! (`fastbet --login <casino>`), because the webview needs its own event loop
//! on a main thread and egui already owns ours. The child watches the page's
//! localStorage and prints every new `tokenid` it sees as `TOKEN <value>` on
//! stdout. The parent verifies each token against the casino API and kills
//! the child as soon as one logs in.

use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::{mpsc, Arc};

use crate::casino::CasinoId;

const TOKEN_PREFIX: &str = "TOKEN ";

/// Where the webview keeps cookies, so the next login is instant.
fn profile_dir(id: CasinoId) -> Option<PathBuf> {
    let dirs = directories::ProjectDirs::from("gg", "fastbet", "FastBet")?;
    Some(dirs.data_local_dir().join("webview").join(id.key()))
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

// ---------------------------------------------------------------- child side

/// Runs the login window. Never returns on success; the parent kills us.
pub fn run_window(id: CasinoId) -> Result<(), String> {
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
    let mut ctx = WebContext::new(profile_dir(id));
    let builder = WebViewBuilder::new_with_web_context(&mut ctx)
        .with_url(id.login_url())
        .with_initialization_script(script)
        .with_devtools(true)
        .with_ipc_handler(move |req| {
            let _ = proxy.send_event(UserEvent::Token(req.body().clone()));
        });
    let _webview = attach(builder, &window).map_err(|e| e.to_string())?;

    let mut last = String::new();
    event_loop.run(move |event, _, control_flow| {
        *control_flow = ControlFlow::Wait;
        match event {
            Event::WindowEvent { event: WindowEvent::CloseRequested, .. } => *control_flow = ControlFlow::Exit,
            Event::UserEvent(UserEvent::Token(t)) => {
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
    let mut ctx = WebContext::new(profile_dir(CasinoId::ChipsGg).map(|p| p.join("capture")));
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
  // websockets
  var NW = window.WebSocket;
  function W(u, p) {
    var ws = p === undefined ? new NW(u) : new NW(u, p);
    send({ kind: 'ws-open', url: String(u) });
    var os = ws.send;
    ws.send = function (d) { send({ kind: 'ws-send', url: String(u), req: cut(d) }); return os.apply(ws, arguments); };
    ws.addEventListener('message', function (ev) { send({ kind: 'ws-recv', url: String(u), res: cut(ev.data) }); });
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
    /// The last token we handed to the engine for verification.
    pub tried: Option<String>,
    /// Latest report from the window.
    pub last: LoginReport,
}

impl LoginSession {
    pub fn spawn(id: CasinoId, repaint: Arc<dyn Fn() + Send + Sync>) -> Result<Self, String> {
        let exe = std::env::current_exe().map_err(|e| e.to_string())?;
        let mut cmd = Command::new(exe);
        cmd.arg("--login").arg(id.key()).stdout(Stdio::piped()).stderr(Stdio::null()).stdin(Stdio::null());
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
        Ok(Self { child, rx, tried: None, last: LoginReport::default() })
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
