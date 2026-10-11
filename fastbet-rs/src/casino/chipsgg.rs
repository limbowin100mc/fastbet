//! chips.gg over its public websocket API (`@chipsgg/openservice-ws-client`).
//!
//! Wire protocol:
//!   call:     [channel, id, action, [params]]
//!   response: [channel, id, result, err]        err = [message, stack]
//!   event:    [channel, null, [path, data]]     state patch
//! Frames arrive as a JSON array of messages.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use futures_util::{SinkExt, StreamExt};
use parking_lot::Mutex;
use serde_json::{json, Value};
use tokio::sync::{mpsc, oneshot, Notify};
use tokio_tungstenite::tungstenite::Message;

use super::*;
use crate::money::{f64_from_json, units_from_json};

const WS_URL: &str = "wss://api.chips.gg/prod/socket";
const RPC_TIMEOUT: Duration = Duration::from_secs(20);

type Reply = Result<Value, CasinoError>;

pub struct ChipsGg {
    seq: AtomicU64,
    out: Mutex<Option<mpsc::UnboundedSender<Message>>>,
    pending: Mutex<HashMap<u64, oneshot::Sender<Reply>>>,
    /// Per-channel state, merged from event patches like the site does.
    state: Mutex<HashMap<String, Value>>,
    token: Mutex<Option<String>>,
    token_changed: Notify,
    alive: AtomicBool,
    ready: AtomicBool,
    events: mpsc::UnboundedSender<CasinoEvent>,
    last_market: Mutex<Market>,
}

impl ChipsGg {
    pub fn spawn(token: Option<String>, events: mpsc::UnboundedSender<CasinoEvent>) -> Arc<dyn Casino> {
        let me = Arc::new(Self {
            seq: AtomicU64::new(0),
            out: Mutex::new(None),
            pending: Mutex::new(HashMap::new()),
            state: Mutex::new(HashMap::new()),
            token: Mutex::new(token),
            token_changed: Notify::new(),
            alive: AtomicBool::new(true),
            ready: AtomicBool::new(false),
            events,
            last_market: Mutex::new(Market::default()),
        });
        tokio::spawn(me.clone().run());
        me
    }

    fn status(&self, s: ConnStatus, detail: impl Into<String>) {
        self.ready.store(s.is_ready(), Ordering::SeqCst);
        let _ = self.events.send(CasinoEvent::Status(s, detail.into()));
    }

    async fn run(self: Arc<Self>) {
        let mut retries: u32 = 0;
        while self.alive.load(Ordering::SeqCst) {
            self.status(ConnStatus::Connecting, "");
            let ws = match tokio_tungstenite::connect_async(WS_URL).await {
                Ok((ws, _)) => ws,
                Err(e) => {
                    tracing::warn!("connect failed: {e}");
                    self.status(ConnStatus::Offline, e.to_string());
                    tokio::time::sleep(backoff(&mut retries)).await;
                    continue;
                }
            };
            retries = 0;
            let (mut sink, mut stream) = ws.split();
            let (otx, mut orx) = mpsc::unbounded_channel::<Message>();
            *self.out.lock() = Some(otx);

            let writer = tokio::spawn(async move {
                while let Some(m) = orx.recv().await {
                    if sink.send(m).await.is_err() {
                        break;
                    }
                }
                let _ = sink.close().await;
            });

            tokio::spawn(self.clone().authenticate());

            loop {
                tokio::select! {
                    msg = stream.next() => match msg {
                        Some(Ok(Message::Text(t))) => self.on_frame(&t),
                        Some(Ok(Message::Binary(b))) => {
                            if let Ok(t) = std::str::from_utf8(&b) { self.on_frame(t) }
                        }
                        Some(Ok(Message::Ping(_))) | Some(Ok(Message::Pong(_))) | Some(Ok(Message::Frame(_))) => {}
                        Some(Ok(Message::Close(_))) | None => break,
                        Some(Err(e)) => { tracing::warn!("socket error: {e}"); break; }
                    },
                    _ = self.token_changed.notified() => {
                        tokio::spawn(self.clone().authenticate());
                    }
                }
                if !self.alive.load(Ordering::SeqCst) {
                    break;
                }
            }

            *self.out.lock() = None;
            writer.abort();
            self.fail_pending(CasinoError::ConnectionLost);
            self.state.lock().clear();
            if !self.alive.load(Ordering::SeqCst) {
                break;
            }
            self.status(ConnStatus::Offline, "");
            tokio::time::sleep(backoff(&mut retries)).await;
        }
    }

    fn fail_pending(&self, err: CasinoError) {
        let pending: Vec<_> = self.pending.lock().drain().collect();
        for (_, tx) in pending {
            let _ = tx.send(Err(err.clone()));
        }
    }

    async fn authenticate(self: Arc<Self>) {
        let token = self.token.lock().clone();
        let Some(token) = token else {
            self.status(ConnStatus::NoToken, "");
            return;
        };
        self.status(ConnStatus::Authenticating, "");
        match self.call("auth", "authenticate", vec![json!(token)]).await {
            // guest tokens authenticate fine but come back without a userid
            Ok(Value::Null) | Ok(Value::Bool(false)) => self.status(ConnStatus::NoToken, "token has no user"),
            Ok(Value::String(s)) if s.is_empty() => self.status(ConnStatus::NoToken, "token has no user"),
            Ok(_) => {
                self.status(ConnStatus::Ready, "");
                let me = self.clone();
                tokio::spawn(async move {
                    if let Ok(v) = me.call("private", "me", vec![]).await {
                        if v.is_object() {
                            me.state.lock().entry("private".into()).or_insert_with(|| json!({}))["me"] = v;
                            me.publish_market();
                        }
                    }
                    if let Ok(st) = me.provable_state().await {
                        let _ = me.events.send(CasinoEvent::Provable(st));
                    }
                });
            }
            Err(e) => self.status(ConnStatus::AuthFailed, e.to_string()),
        }
    }

    async fn call(&self, channel: &str, action: &str, args: Vec<Value>) -> Reply {
        let out = self.out.lock().clone().ok_or(CasinoError::NotConnected)?;
        let id = self.seq.fetch_add(1, Ordering::SeqCst) + 1;
        let (tx, rx) = oneshot::channel();
        self.pending.lock().insert(id, tx);
        let frame = Value::Array(vec![json!(channel), json!(id), json!(action), Value::Array(args)]);
        if out.send(Message::Text(frame.to_string().into())).is_err() {
            self.pending.lock().remove(&id);
            return Err(CasinoError::NotConnected);
        }
        match tokio::time::timeout(RPC_TIMEOUT, rx).await {
            Ok(Ok(reply)) => reply,
            Ok(Err(_)) => Err(CasinoError::ConnectionLost),
            Err(_) => {
                self.pending.lock().remove(&id);
                Err(CasinoError::Timeout)
            }
        }
    }

    fn on_frame(&self, raw: &str) {
        let Ok(Value::Array(msgs)) = serde_json::from_str::<Value>(raw) else { return };
        let mut market_dirty = false;
        for m in msgs {
            let Value::Array(m) = m else { continue };
            if m.len() < 3 {
                continue;
            }
            if m[1].is_null() {
                let channel = m[0].as_str().unwrap_or("").to_string();
                market_dirty |= self.apply_event(&channel, &m[2]);
                continue;
            }
            let Some(id) = m[1].as_u64() else { continue };
            let Some(tx) = self.pending.lock().remove(&id) else { continue };
            let reply = match m.get(3) {
                Some(err) if !err.is_null() => {
                    let msg = match err {
                        Value::Array(a) => a.first().map(value_to_string).unwrap_or_else(|| "Unknown error".into()),
                        other => value_to_string(other),
                    };
                    Err(CasinoError::Server(msg))
                }
                _ => Ok(m[2].clone()),
            };
            let _ = tx.send(reply);
        }
        if market_dirty {
            self.publish_market();
        }
    }

    /// Applies one `[path, data]` patch. Returns true if a key the UI cares about changed.
    fn apply_event(&self, channel: &str, patch: &Value) -> bool {
        const WATCHED: &[&str] = &["currencies", "myWallets", "me"];
        let (path, data) = match patch {
            Value::Array(a) if a.len() >= 2 => (&a[0], &a[1]),
            Value::Array(a) if a.len() == 1 => (&a[0], &Value::Null),
            _ => return false,
        };
        let path: Vec<String> = match path {
            Value::Array(p) => p.iter().map(value_to_string).collect(),
            Value::Null => vec![],
            other => vec![value_to_string(other)],
        };
        let mut state = self.state.lock();
        if path.is_empty() {
            state.insert(channel.to_string(), data.clone());
            return true;
        }
        let root = state.entry(channel.to_string()).or_insert_with(|| json!({}));
        if !root.is_object() {
            *root = json!({});
        }
        let mut o = root;
        for key in &path[..path.len() - 1] {
            let obj = o.as_object_mut().unwrap();
            let next = obj.entry(key.clone()).or_insert_with(|| json!({}));
            if !next.is_object() {
                *next = json!({});
            }
            o = next;
        }
        let last = &path[path.len() - 1];
        let obj = o.as_object_mut().unwrap();
        if data.is_null() {
            obj.remove(last);
        } else {
            obj.insert(last.clone(), data.clone());
        }
        WATCHED.contains(&path[0].as_str())
    }

    /// The site merges every channel's state into one object; look keys up the same way.
    fn pick(state: &HashMap<String, Value>, key: &str) -> Option<Value> {
        let mut order: Vec<&String> = state.keys().collect();
        order.sort_by_key(|k| match k.as_str() {
            "private" => 0,
            "public" => 1,
            _ => 2,
        });
        for ch in order {
            if let Some(v) = state.get(ch).and_then(|s| s.get(key)) {
                if !v.is_null() {
                    return Some(v.clone());
                }
            }
        }
        None
    }

    fn publish_market(&self) {
        let market = {
            let state = self.state.lock();
            let mut m = Market::default();
            if let Some(Value::Object(cs)) = Self::pick(&state, "currencies") {
                for (k, c) in cs {
                    let Some(dec) = c.get("decimals").and_then(|d| d.as_u64()) else { continue };
                    m.currencies.insert(
                        k,
                        CurrencyInfo {
                            decimals: dec as u32,
                            price: c.get("price").and_then(f64_from_json).unwrap_or(0.0),
                            hidden: c.get("hidden").and_then(|h| h.as_bool()).unwrap_or(false),
                        },
                    );
                }
            }
            if let Some(Value::Object(ws)) = Self::pick(&state, "myWallets") {
                for (k, w) in ws {
                    if let Some(b) = w.get("balance").and_then(units_from_json) {
                        m.wallets.insert(k, b);
                    }
                }
            }
            if let Some(me) = Self::pick(&state, "me") {
                m.site_currency = me.get("currency").and_then(|c| c.as_str()).map(str::to_string);
                m.username = me.get("username").and_then(|c| c.as_str()).map(str::to_string);
            }
            m
        };
        let mut last = self.last_market.lock();
        if *last != market {
            *last = market.clone();
            let _ = self.events.send(CasinoEvent::Market(market));
        }
    }
}

#[async_trait]
impl Casino for ChipsGg {
    fn id(&self) -> CasinoId {
        CasinoId::ChipsGg
    }

    fn set_token(&self, token: Option<String>) {
        // Always re-authenticate, even with the same token: chips.gg upgrades a
        // guest token to a user token in place when the user logs in.
        *self.token.lock() = token;
        self.token_changed.notify_one();
    }

    fn shutdown(&self) {
        self.alive.store(false, Ordering::SeqCst);
        *self.out.lock() = None;
        self.fail_pending(CasinoError::ConnectionLost);
    }

    async fn bet_limbo(&self, req: &BetRequest) -> Result<BetReply, CasinoError> {
        if !self.ready.load(Ordering::SeqCst) {
            return Err(CasinoError::NotConnected);
        }
        let params = json!({
            "currency": req.currency,
            "amount": req.amount.to_string(),
            "autoCashout": req.target,
        });
        let r = self.call("private", "betLimbo", vec![params]).await?;
        let outcome = r.get("outcome").and_then(f64_from_json).ok_or_else(|| CasinoError::BadReply(r.to_string()))?;
        Ok(BetReply {
            outcome,
            amount: r.get("amount").and_then(units_from_json),
            winnings: r.get("winnings").and_then(units_from_json),
            win: r.get("win").and_then(|w| w.as_bool()),
            nonce: r.get("nonce").and_then(|n| n.as_u64()),
        })
    }

    async fn rotate_seed(&self, client_seed: &str) -> Result<ProvableState, CasinoError> {
        let r = self.call("private", "rotateProvableSeed", vec![json!({ "clientSeed": client_seed })]).await?;
        let mut st = ProvableState::from_json(&r);
        if st.client_seed.is_none() {
            st.client_seed = Some(client_seed.to_string());
        }
        if st.nonce.is_none() {
            st.nonce = Some(0);
        }
        Ok(st)
    }

    async fn provable_state(&self) -> Result<ProvableState, CasinoError> {
        let r = self.call("private", "getMyProvableState", vec![]).await?;
        Ok(ProvableState::from_json(&r))
    }

    async fn list_slots(&self, f: SlotFilter) -> Result<Vec<crate::slots::SlotGame>, CasinoError> {
        let limit = f.limit.clamp(1, 100);
        let r = if let Some(q) = f.query.filter(|q| !q.trim().is_empty()) {
            self.call("public", "searchGames", vec![json!({ "term": q.trim(), "skip": f.skip, "limit": limit })]).await?
        } else if let Some(p) = f.producer {
            self.call("public", "listGamesByProducer", vec![json!({ "producer": p, "skip": f.skip, "limit": limit })]).await?
        } else {
            self.call("public", "listGamesByCategory", vec![json!({ "category": "Video Slots", "skip": f.skip, "limit": limit })]).await?
        };
        let list = match r {
            Value::Array(a) => a,
            Value::Object(ref o) => o.get("results").and_then(|v| v.as_array()).cloned().unwrap_or_default(),
            _ => vec![],
        };
        Ok(list
            .iter()
            .filter_map(|g| {
                let id = g.get("id").or(g.get("_id"))?.as_str()?.to_string();
                let category = g.get("category").and_then(|c| c.as_str()).unwrap_or("");
                if !category.to_ascii_lowercase().contains("slot") && !category.is_empty() {
                    return None;
                }
                Some(crate::slots::SlotGame {
                    id,
                    name: g.get("name").and_then(|n| n.as_str()).unwrap_or("?").to_string(),
                    producer: g.get("producer").or(g.get("provider")).and_then(|p| p.as_str()).unwrap_or("").to_string(),
                    image: g.get("images").and_then(|i| i.get("s1").or(i.get("s2")).or(i.get("bg"))).and_then(|s| s.as_str()).unwrap_or("").to_string(),
                    demo: g.get("demo_game_support").and_then(|d| d.as_bool()).unwrap_or(false),
                    rtp: g.get("rtp").and_then(f64_from_json).unwrap_or(0.0),
                    provider_game_id: g
                        .get("supplier_identifier")
                        .and_then(|s| s.get("game_id"))
                        .and_then(|v| v.as_u64().map(|n| n.to_string()).or_else(|| v.as_str().map(str::to_string))),
                    studio: None,
                })
            })
            .collect())
    }

    async fn launch_slot(&self, catalog_id: &str, demo: bool, currency: &str) -> Result<String, CasinoError> {
        let channel = if demo { "public" } else { "private" };
        let r = self
            .call(channel, "initializeCasinoGame", vec![json!({ "catalogid": catalog_id, "isMobile": false, "demo": demo, "currency": currency })])
            .await?;
        match r {
            Value::String(s) => Ok(s),
            Value::Object(ref o) => o.get("url").and_then(|u| u.as_str()).map(str::to_string).ok_or_else(|| CasinoError::BadReply(r.to_string())),
            other => Err(CasinoError::BadReply(other.to_string())),
        }
    }
}

fn value_to_string(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}
