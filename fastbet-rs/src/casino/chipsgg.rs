//! chips.gg over its public websocket API (`@chipsgg/openservice-ws-client`).
//!
//! Wire protocol:
//!   call:     [channel, id, action, [params]]
//!   response: [channel, id, result, err]        err = [message, stack]
//!   event:    [channel, null, [path, data]]     state patch
//! Frames arrive as a JSON array of messages.
//!
//! Originals (Keno, Mines, HiLo) are `private` calls too; their replies are turned
//! into Stake's state shapes so the Stake boards are reused.

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
use crate::originals::{mines_state, rank_value, Game, Round, Step, Tables, RANKS};

const WS_URL: &str = "wss://api.chips.gg/prod/socket";
const RPC_TIMEOUT: Duration = Duration::from_secs(20);
/// The originals in the public catalog are `chipsgg-<slug>`.
const ORIGINAL_PREFIX: &str = "chipsgg-";

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
    /// Rounds of Mines and HiLo still open, as last seen (HiLo's guesses depend on the card).
    open_rounds: Mutex<HashMap<Game, Round>>,
    /// The payout tables; Mines' come from chips.gg itself.
    tables: Mutex<Tables>,
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
            open_rounds: Mutex::new(HashMap::new()),
            tables: Mutex::new(Tables::builtin()),
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

impl ChipsGg {
    /// One request of an original, answered with the game. chips.gg's "finish your open game
    /// first" becomes [`CasinoError::RoundOpen`].
    async fn play(&self, game: Game, action: &str, params: Value) -> Result<Round, CasinoError> {
        let reply = self.call("private", action, vec![params]).await.map_err(|e| match e {
            CasinoError::Server(m) if m.starts_with("Already has active game") => CasinoError::RoundOpen,
            e => e,
        })?;
        let round = round_of(game, &reply, &self.tables.lock()).ok_or_else(|| CasinoError::BadReply(reply.to_string().chars().take(200).collect()))?;
        tracing::debug!("chipsgg: {} {action} -> open {} ×{} {}", game.name(), round.active, round.multiplier, round.state);
        let mut open = self.open_rounds.lock();
        if round.active {
            open.insert(game, round.clone());
        } else {
            open.remove(&game);
        }
        Ok(round)
    }

    /// An auto bet's Mines round: chips.gg has no one-request version, so the picked tiles are
    /// revealed one after another and the round is cashed out if no mine ended it.
    async fn mines_auto(&self, mut round: Round, tiles: &[i64]) -> Result<Round, CasinoError> {
        for tile in tiles {
            if !round.active {
                return Ok(round);
            }
            round = self.play(Game::Mines, "pickMinesGameTile", json!({ "tileid": tile })).await?;
        }
        if round.active {
            round = self.play(Game::Mines, "cashoutMinesGame", json!({})).await?;
        }
        Ok(round)
    }

    /// The card HiLo's next guess is about: the open round's latest one.
    async fn hilo_card(&self) -> Result<String, CasinoError> {
        let cached = self.open_rounds.lock().get(&Game::Hilo).cloned();
        let round = match cached {
            Some(r) => r,
            None => self.original_active(Game::Hilo).await?.ok_or_else(|| CasinoError::Server("No open round".into()))?,
        };
        let last = round.state["rounds"].as_array().and_then(|a| a.last()).map(|x| &x["card"]).unwrap_or(&round.state["startCard"]);
        Ok(last["rank"].as_str().unwrap_or("7").to_string())
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
        Ok(catalog_entries(r)
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

    async fn original_bet(&self, game: Game, currency: &str, amount: Units, params: Value, auto: bool) -> Result<Round, CasinoError> {
        if !self.ready.load(Ordering::SeqCst) {
            return Err(CasinoError::NotConnected);
        }
        let amount = amount.to_string();
        match game {
            // forceDone settles the bet at once (the site settles it after its animation)
            Game::Keno => {
                let picks = one_based(&params["numbers"]);
                if picks.is_empty() {
                    return Err(CasinoError::Server("Pick some numbers".into()));
                }
                let risk = params["risk"].as_str().unwrap_or("classic");
                let bet = json!({ "currency": currency, "amount": amount, "ruleset": risk, "selection": picks, "forceDone": true });
                self.play(game, "betKeno", bet).await
            }
            Game::Mines => {
                let mines = params["minesCount"].as_u64().unwrap_or(3);
                let round = self.play(game, "betMinesGame", json!({ "currency": currency, "amount": amount, "mines": mines })).await?;
                if auto {
                    self.mines_auto(round, &one_based(&params["fields"])).await
                } else {
                    Ok(round)
                }
            }
            // the player brings the first card
            Game::Hilo => {
                let card = &params["startCard"];
                let start = site_card(card["rank"].as_str().unwrap_or("7"), card["suit"].as_str().unwrap_or("H"));
                self.play(game, "betHiloGame", json!({ "currency": currency, "amount": amount, "initialCard": start })).await
            }
            _ => Err(CasinoError::Server(format!("{} isn't on chips.gg", game.name()))),
        }
    }

    async fn original_step(&self, game: Game, step: Step, params: Value) -> Result<Round, CasinoError> {
        match (game, step) {
            (Game::Mines, Step::Next) => {
                let tile = one_based(&params["fields"]).first().copied().ok_or_else(|| CasinoError::Server("Pick a tile".into()))?;
                self.play(game, "pickMinesGameTile", json!({ "tileid": tile })).await
            }
            (Game::Mines, Step::Cashout) => self.play(game, "cashoutMinesGame", json!({})).await,
            (Game::Hilo, Step::Next) => {
                let guess = params["guess"].as_str().unwrap_or("");
                let rank = if guess == "equal" { self.hilo_card().await? } else { String::new() };
                let choice = hilo_choice(guess, &rank).ok_or_else(|| CasinoError::Server(format!("No such guess: {guess}")))?;
                self.play(game, "chooseHighOrLow", json!({ "choice": choice })).await
            }
            (Game::Hilo, Step::Cashout) => self.play(game, "cashoutHiloGame", json!({})).await,
            _ => Err(CasinoError::Server(format!("{} has no steps", game.name()))),
        }
    }

    async fn original_active(&self, game: Game) -> Result<Option<Round>, CasinoError> {
        let action = match game {
            Game::Mines => "getActiveMinesGame",
            Game::Hilo => "getActiveHiloGame",
            _ => return Ok(None),
        };
        match self.play(game, action, json!({})).await {
            Ok(r) => Ok(Some(r)),
            Err(CasinoError::Server(m)) if m == "no active game" => {
                self.open_rounds.lock().remove(&game);
                Ok(None)
            }
            Err(e) => Err(e),
        }
    }

    /// Keno pays exactly like Stake's (the built-in table); Mines' payouts are chips.gg's own,
    /// one table per number of mines.
    async fn original_tables(&self) -> Result<Tables, CasinoError> {
        let configs = (1..=24u32).map(|m| async move { (m, self.call("public", "getGameConfig", vec![json!({ "gamename": "mines", "ruleset": m.to_string() })]).await) });
        let mut tables = Tables::builtin();
        for (m, reply) in futures_util::future::join_all(configs).await {
            let pays: Vec<f64> = reply?["paytable"].as_array().map(|a| a.iter().filter_map(f64_from_json).collect()).unwrap_or_default();
            if !pays.is_empty() {
                tables.mines.insert(m.to_string(), pays);
            }
        }
        *self.tables.lock() = tables.clone();
        Ok(tables)
    }

    async fn list_originals(&self) -> Result<Vec<crate::slots::SlotGame>, CasinoError> {
        let r = self.call("public", "listGamesByProducer", vec![json!({ "producer": "chipsgg", "skip": 0, "limit": 100 })]).await?;
        Ok(catalog_entries(r)
            .iter()
            .filter_map(|g| {
                let slug = g.get("id")?.as_str()?.strip_prefix(ORIGINAL_PREFIX)?;
                let name = match Game::from_slug(slug).filter(|g| CasinoId::ChipsGg.originals().contains(g)) {
                    Some(game) => game.name(),
                    None if slug == "limbo" => "Limbo",
                    None => return None,
                };
                let image = g.get("images").and_then(|i| i.get("s2")).and_then(Value::as_str).filter(|u| u.starts_with("https://"))?;
                Some(crate::slots::SlotGame {
                    id: slug.to_string(),
                    name: name.to_string(),
                    producer: "chipsgg".into(),
                    image: image.to_string(),
                    demo: false,
                    rtp: g.get("rtp").and_then(f64_from_json).unwrap_or(99.0),
                    provider_game_id: None,
                    studio: None,
                })
            })
            .collect())
    }
}

/// A catalog reply: a list, or `{results: [...]}`.
fn catalog_entries(r: Value) -> Vec<Value> {
    match r {
        Value::Array(a) => a,
        Value::Object(ref o) => o.get("results").and_then(|v| v.as_array()).cloned().unwrap_or_default(),
        _ => vec![],
    }
}

/// The boards number keno and mines tiles from 0, chips.gg from 1.
fn one_based(v: &Value) -> Vec<i64> {
    v.as_array().map(|a| a.iter().filter_map(Value::as_i64).map(|n| n + 1).collect()).unwrap_or_default()
}

fn zero_based(v: &Value) -> Vec<i64> {
    v.as_array().map(|a| a.iter().filter_map(Value::as_i64).map(|n| n - 1).collect()).unwrap_or_default()
}

/// The boards' card (`rank` "A"…"K", `suit` letter) -> chips.gg's `{hit: 1–13, color, suit}`.
fn site_card(rank: &str, suit: &str) -> Value {
    let (suit, color) = match suit {
        "H" => ("hearts", "red"),
        "D" => ("diamonds", "red"),
        "C" => ("clubs", "black"),
        _ => ("spades", "black"),
    };
    json!({ "hit": rank_value(rank), "color": color, "suit": suit })
}

/// chips.gg's card -> the boards' `{rank, suit}`.
fn board_card(c: &Value) -> Value {
    let hit = c["hit"].as_u64().unwrap_or(7).clamp(1, 13) as usize;
    let suit = match c["suit"].as_str() {
        Some("hearts") => "H",
        Some("diamonds") => "D",
        Some("clubs") => "C",
        _ => "S",
    };
    json!({ "rank": RANKS[hit - 1], "suit": suit })
}

/// A board's HiLo guess -> chips.gg's choice. The boards name what wins on this card; chips.gg
/// has "high" (higher or same, but strictly higher on an ace) and "low" (the mirror image),
/// so "same" is the high guess on a king and the low one on an ace.
fn hilo_choice(guess: &str, rank: &str) -> Option<&'static str> {
    Some(match guess {
        "higher" | "higherEqual" => "high",
        "lower" | "lowerEqual" => "low",
        "equal" if rank == "K" => "high",
        "equal" => "low",
        "skip" => "skip",
        "red" => "red",
        "black" => "black",
        "number" => "number",
        "face" => "face",
        "ace" => "ace",
        _ => return None,
    })
}

/// A chips.gg game -> a round in the state shape of Stake's board for the game.
fn round_of(game: Game, g: &Value, tables: &Tables) -> Option<Round> {
    let done = g.get("done").and_then(Value::as_bool).unwrap_or(false);
    let state = match game {
        // `outcome.hits` are the numbers drawn
        Game::Keno => json!({ "risk": g["ruleset"], "selectedNumbers": zero_based(&g["selection"]), "drawnNumbers": zero_based(&g["outcome"]["hits"]) }),
        // where the mines were (`outcome`) comes with the end of the round
        Game::Mines => {
            let count = g.get("mines").and_then(Value::as_u64).unwrap_or(0) as u32;
            let mines = if done { zero_based(&g["outcome"]) } else { vec![] };
            let mut state = mines_state(count, &zero_based(&g["selection"]), &mines);
            // what chips.gg pays, not the formula's exact value
            for (gems, r) in state["rounds"].as_array_mut().into_iter().flatten().enumerate() {
                if r["payoutMultiplier"].as_f64().unwrap_or(0.0) > 0.0 {
                    r["payoutMultiplier"] = json!(tables.mines(count, gems as u32 + 1));
                }
            }
            state
        }
        // the start card, then one entry per guess or skip with what the round pays after it
        Game::Hilo => {
            let cards = g.get("selection")?.as_array()?;
            let rounds: Vec<Value> = cards
                .iter()
                .skip(1)
                .map(|c| json!({ "card": board_card(&c["card"]), "guess": c["choice"], "payoutMultiplier": c["multiplier"].as_f64().unwrap_or(0.0) }))
                .collect();
            json!({ "startCard": board_card(&cards.first()?["card"]), "rounds": rounds })
        }
        _ => return None,
    };
    // a lost Mines or HiLo round has no multiplier; Keno's is what the hits paid (also below 1)
    let pays = done && (game == Game::Keno || g.get("win").and_then(Value::as_bool) == Some(true));
    Some(Round {
        game,
        id: g.get("id")?.as_str()?.to_string(),
        active: !done,
        currency: g.get("currency").and_then(Value::as_str).unwrap_or("").to_ascii_lowercase(),
        amount: g.get("amount").and_then(units_from_json).unwrap_or(0),
        amount_mult: 1.0,
        payout: g.get("winnings").and_then(units_from_json).unwrap_or(0),
        multiplier: if pays { g.get("multiplier").and_then(f64_from_json).unwrap_or(0.0) } else { 0.0 },
        state,
    })
}

fn value_to_string(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn originals_replies() {
        let tables = Tables::builtin();
        // replies of 0-stake rounds on chips.gg
        let keno = json!({"id":"keno:1","done":true,"outcome":{"hits":[32,20,3,31,11,2,1,26,16,19],"matches":[1,2,3]},"win":true,"ruleset":"classic","selection":[1,2,3,4,5],"amount":"0","currency":"usdt","multiplier":4.1,"winnings":"0"});
        let r = round_of(Game::Keno, &keno, &tables).unwrap();
        assert!(!r.active && r.multiplier == 4.1);
        assert_eq!(r.state["selectedNumbers"], json!([0, 1, 2, 3, 4]));
        assert_eq!(r.state["drawnNumbers"][0], 31);

        let open = json!({"id":"mines:1","win":false,"done":false,"amount":"0","mines":3,"selection":[1,25],"currency":"usdt"});
        let r = round_of(Game::Mines, &open, &tables).unwrap();
        assert!(r.active && r.multiplier == 0.0);
        assert_eq!(r.state["rounds"][1]["field"], 24);
        let mut chips = Tables::builtin();
        chips.mines.insert("3".into(), vec![0.99, 1.12, 1.28]);
        assert_eq!(round_of(Game::Mines, &open, &chips).unwrap().state["rounds"][1]["payoutMultiplier"], 1.28);
        let cashed = json!({"id":"mines:1","win":true,"done":true,"amount":"0","mines":3,"selection":[1,25],"outcome":[2,6,9],"currency":"usdt","multiplier":1.28,"winnings":"0"});
        let r = round_of(Game::Mines, &cashed, &chips).unwrap();
        assert!(!r.active && r.multiplier == 1.28);
        assert_eq!(r.state["mines"], json!([1, 5, 8]));
        let lost = json!({"id":"mines:2","win":false,"done":true,"amount":"0","mines":24,"selection":[13],"outcome":[24,10,13],"currency":"usdt"});
        let r = round_of(Game::Mines, &lost, &tables).unwrap();
        assert!(!r.active && r.multiplier == 0.0);
        assert_eq!(r.state["rounds"][0]["payoutMultiplier"], 0.0);

        let hilo = json!({"id":"hilo:1","win":true,"done":true,"amount":"0","currency":"usdt","multiplier":1.2675,"winnings":"0","selection":[
            {"card":{"hit":13,"color":"black","suit":"clubs"},"choice":"startCard","multiplier":1},
            {"card":{"hit":9,"suit":"clubs","color":"black"},"multiplier":1.0725,"choice":"low","win":true},
            {"card":{"hit":3,"suit":"diamonds","color":"red"},"multiplier":1.0725,"choice":"skip","win":true},
            {"card":{"hit":9,"suit":"diamonds","color":"red"},"multiplier":1.2675,"choice":"high","win":true}]});
        let r = round_of(Game::Hilo, &hilo, &tables).unwrap();
        assert!(!r.active && r.multiplier == 1.2675);
        assert_eq!(r.state["startCard"], json!({"rank": "K", "suit": "C"}));
        assert_eq!(r.state["rounds"][1], json!({"card": {"rank": "3", "suit": "D"}, "guess": "skip", "payoutMultiplier": 1.0725}));
        let lost = json!({"id":"hilo:2","win":false,"done":true,"amount":"0","currency":"usdt","selection":[
            {"card":{"hit":12,"color":"black","suit":"spades"},"choice":"startCard","multiplier":1},
            {"card":{"hit":9,"suit":"hearts","color":"red"},"multiplier":0,"choice":"face","win":false}]});
        assert_eq!(round_of(Game::Hilo, &lost, &tables).unwrap().multiplier, 0.0);
    }

    #[test]
    fn hilo_guesses_and_cards() {
        // chips.gg's high on an ace is "higher", its low the ace again; a king mirrors that
        assert_eq!(hilo_choice("higher", "A"), Some("high"));
        assert_eq!(hilo_choice("equal", "A"), Some("low"));
        assert_eq!(hilo_choice("equal", "K"), Some("high"));
        assert_eq!(hilo_choice("lowerEqual", "7"), Some("low"));
        assert_eq!(hilo_choice("face", "Q"), Some("face"));
        assert_eq!(hilo_choice("bogus", "7"), None);
        assert_eq!(site_card("10", "D"), json!({"hit": 10, "color": "red", "suit": "diamonds"}));
        assert_eq!(board_card(&site_card("Q", "S")), json!({"rank": "Q", "suit": "S"}));
        assert_eq!(one_based(&json!([0, 39])), vec![1, 40]);
    }
}
