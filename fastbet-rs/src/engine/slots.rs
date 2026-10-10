//! Slot autoplay: opens a provider session through the browser bridge and
//! spins it without the game client. Sequential by nature (one round at a
//! time per session).

use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use super::{Engine, Event};
use crate::slots::hacksaw::{HacksawSession, Launch};
use crate::slots::{PlayMode, Provider, SessionInfo, SlotError, SlotGame, SlotSession};

#[derive(Clone, Debug)]
pub struct SlotConfig {
    /// Bet level in the session currency's minor units.
    pub bet: i64,
    pub feature: Option<String>,
    /// Pause when a round pays at least this many × the stake.
    pub stop_mult: Option<f64>,
    /// 0 = no limit
    pub limit: u32,
    pub stop_profit: Option<i64>,
    pub stop_loss: Option<i64>,
    pub delay_ms: u64,
    /// Wallet currency used to launch real-money sessions (for reopening).
    pub currency: String,
}

#[derive(Clone, Debug, Default)]
pub struct SlotState {
    pub game: Option<SlotGame>,
    pub mode: Option<PlayMode>,
    pub info: Option<SessionInfo>,
    pub opening: bool,
    pub running: bool,
    pub paused: bool,
    pub msg: String,
    pub done: u32,
    pub profit: i64,
    pub balance: Option<i64>,
    pub stop_on_win: bool,
    pub error: Option<String>,
}

#[derive(Clone, Debug)]
pub struct SpinRecord {
    pub at: SystemTime,
    pub round_id: String,
    pub feature: Option<String>,
    pub cost: i64,
    pub win: i64,
    pub multiplier: f64,
    pub balance: Option<i64>,
    pub free_spins: bool,
    pub ms: f64,
    pub currency: String,
    pub decimals: u32,
    /// Reached the stop-on-multiplier line.
    pub hit: bool,
}

impl Engine {
    pub(super) fn emit_slot(&self) {
        let st = self.slot.lock().clone();
        self.emit(Event::Slot(st));
    }

    pub(super) fn slot_fail(&self, msg: impl Into<String>) {
        let mut s = self.slot.lock();
        s.opening = false;
        s.running = false;
        s.paused = false;
        s.error = Some(msg.into());
        drop(s);
        self.emit_slot();
    }

    /// Opens a fresh session for `game`, resetting the run state.
    pub(super) async fn open_slot(self: Arc<Self>, game: SlotGame, mode: PlayMode, currency: String) {
        self.close_slot().await;
        {
            let mut s = self.slot.lock();
            *s = SlotState { game: Some(game.clone()), mode: Some(mode), opening: true, msg: "Opening session…".into(), ..Default::default() };
        }
        self.emit_slot();
        self.connect_slot(game, mode, currency).await;
    }

    /// Replaces the provider session but keeps the run counters (used when a session expires mid-run).
    async fn reopen_slot_session(&self, game: SlotGame, mode: PlayMode, currency: String) {
        if let Some(s) = self.slot_session.lock().await.take() {
            s.close();
        }
        {
            let mut s = self.slot.lock();
            s.opening = true;
            s.info = None;
            s.msg = "Session expired · reconnecting…".into();
        }
        self.emit_slot();
        self.connect_slot(game, mode, currency).await;
    }

    async fn connect_slot(&self, game: SlotGame, mode: PlayMode, currency: String) {

        let Some(provider) = Provider::for_producer(&game.producer) else {
            return self.slot_fail(format!("{} games are not supported yet", game.producer));
        };
        let result: Result<Arc<dyn SlotSession>, String> = async {
            // the casino's launcher → final game URL, for everything except Hacksaw demo
            let launched = |demo: bool| {
                let game = game.clone();
                let currency = currency.clone();
                async move {
                    // the casino socket may still be coming up (e.g. right after start)
                    let mut last = String::new();
                    for attempt in 0..12 {
                        let casino = self.casino.lock().clone();
                        let res = match casino {
                            Some(c) => c.launch_slot(&game.id, demo, &currency).await.map_err(|e| e.to_string()),
                            None => Err("Not connected".to_string()),
                        };
                        match res {
                            Ok(url) => return resolve_launcher(&url).await,
                            Err(e) if e.contains("Not connected") && attempt < 11 => {
                                last = e;
                                tokio::time::sleep(Duration::from_millis(1000)).await;
                            }
                            Err(e) => return Err(e),
                        }
                    }
                    Err(last)
                }
            };
            match provider {
                Provider::Hacksaw => {
                    let launch = if mode == PlayMode::Demo {
                        let gid = game.provider_game_id.as_deref().and_then(|s| s.parse().ok()).ok_or("No provider game id for this game")?;
                        Launch::demo(gid)
                    } else {
                        let final_url = launched(false).await?;
                        let mut l = Launch::from_url(&final_url, "USD").ok_or_else(|| format!("Could not read the launch parameters from {final_url}"))?;
                        l.env = crate::slots::hacksaw::live_env(&l.partner).await;
                        l
                    };
                    let s = HacksawSession::open(launch).await.map_err(|e| e.to_string())?;
                    Ok(Arc::new(s) as Arc<dyn SlotSession>)
                }
                Provider::Avatarux => {
                    let final_url = launched(mode == PlayMode::Demo).await?;
                    let l = crate::slots::avatarux::Launch::from_url(&final_url).ok_or_else(|| format!("Could not read the launch parameters from {final_url}"))?;
                    let s = crate::slots::avatarux::AvataruxSession::open(l).await.map_err(|e| e.to_string())?;
                    Ok(Arc::new(s) as Arc<dyn SlotSession>)
                }
                Provider::Bgaming => {
                    let final_url = launched(mode == PlayMode::Demo).await?;
                    let l = crate::slots::bgaming::Launch::from_url(&final_url).ok_or_else(|| format!("Could not read the launch parameters from {final_url}"))?;
                    let s = crate::slots::bgaming::BgamingSession::open(l).await.map_err(|e| e.to_string())?;
                    Ok(Arc::new(s) as Arc<dyn SlotSession>)
                }
            }
        }
        .await;
        match result {
            Ok(session) => {
                let info = session.info().clone();
                *self.slot_session.lock().await = Some(session);
                let mut s = self.slot.lock();
                s.opening = false;
                if info.balance.is_some() {
                    s.balance = info.balance;
                }
                s.info = Some(info);
                s.msg = if s.running { "Running".into() } else { "Ready".into() };
                drop(s);
                self.emit_slot();
            }
            Err(e) => self.slot_fail(e),
        }
    }

    pub(super) async fn close_slot(&self) {
        self.slot_run.fetch_add(1, Ordering::SeqCst);
        if let Some(s) = self.slot_session.lock().await.take() {
            s.close();
        }
        let mut st = self.slot.lock();
        st.running = false;
        st.paused = false;
        st.info = None;
        st.opening = false;
        drop(st);
        self.emit_slot();
    }

    pub(super) fn start_slot(self: &Arc<Self>, cfg: SlotConfig) {
        {
            let mut s = self.slot.lock();
            if s.running || s.info.is_none() {
                return;
            }
            s.running = true;
            s.paused = false;
            s.done = 0;
            s.profit = 0;
            s.stop_on_win = false;
            s.error = None;
            s.msg = "Running".into();
        }
        let run = self.slot_run.fetch_add(1, Ordering::SeqCst) + 1;
        self.emit_slot();
        tokio::spawn(self.clone().slot_worker(run, cfg));
    }

    pub(super) fn stop_slot(&self, msg: impl Into<String>) {
        let mut s = self.slot.lock();
        if !s.running {
            return;
        }
        s.running = false;
        s.paused = false;
        s.msg = msg.into();
        drop(s);
        self.slot_wake.notify_one();
        self.emit_slot();
    }

    pub(super) fn pause_slot(&self, paused: Option<bool>, msg: Option<String>) {
        let mut s = self.slot.lock();
        if !s.running {
            return;
        }
        let next = paused.unwrap_or(!s.paused);
        s.paused = next;
        s.msg = msg.unwrap_or_else(|| if next { "Paused" } else { "Running" }.into());
        drop(s);
        if !next {
            self.slot_wake.notify_one();
        }
        self.emit_slot();
    }

    fn slot_live(&self, run: u64) -> bool {
        self.slot_run.load(Ordering::SeqCst) == run && self.slot.lock().running
    }

    async fn slot_worker(self: Arc<Self>, run: u64, cfg: SlotConfig) {
        // adaptive throttle: back off after a rate limit, relax again after a quiet stretch
        let mut throttle_ms: u64 = 0;
        let mut quiet: u32 = 0;
        let mut reopened: u32 = 0;
        loop {
            if !self.slot_live(run) {
                return;
            }
            if self.slot.lock().paused {
                self.slot_wake.notified().await;
                continue;
            }
            let session = match self.slot_session.lock().await.clone() {
                Some(s) => s,
                None => return self.stop_slot("Session closed"),
            };
            let info = session.info().clone();
            let mut res = session.spin(cfg.bet, cfg.feature.as_deref()).await;
            // transient trouble gets a few retries with backoff: a network hiccup in
            // the bridge, or the provider rate-limiting us for a moment
            let mut attempt = 0u32;
            while matches!(&res, Err(SlotError::Bridge(_)) | Err(SlotError::RateLimited)) && attempt < 5 && self.slot_live(run) {
                attempt += 1;
                let limited = matches!(&res, Err(SlotError::RateLimited));
                if limited || attempt >= 2 {
                    throttle_ms = (throttle_ms + 150).min(1500);
                    quiet = 0;
                }
                // network trouble: 1s, 2s, 4s, 8s, 16s; rate limit: 3s, 6s, 9s …
                let wait = if limited { 3000 * attempt as u64 } else { 500 * (1u64 << attempt) };
                self.pause_slot(Some(true), Some(if wait >= 3000 { "Provider rate limit · waiting…".into() } else { "Reconnecting…".into() }));
                tokio::time::sleep(Duration::from_millis(wait)).await;
                if !self.slot_live(run) {
                    return;
                }
                self.pause_slot(Some(false), Some("Running".into()));
                res = session.spin(cfg.bet, cfg.feature.as_deref()).await;
            }
            if !self.slot_live(run) {
                return;
            }
            let r = match res {
                Ok(r) => r,
                Err(SlotError::Balance) => return self.stop_slot("Insufficient balance"),
                Err(SlotError::SessionExpired) => {
                    // open a new session and carry on with the same run
                    let (game, mode) = {
                        let s = self.slot.lock();
                        (s.game.clone(), s.mode)
                    };
                    let (Some(game), Some(mode)) = (game, mode) else { return self.stop_slot("Session expired") };
                    reopened += 1;
                    if reopened > 3 {
                        return self.stop_slot("Session keeps expiring");
                    }
                    self.reopen_slot_session(game, mode, cfg.currency.clone()).await;
                    if self.slot.lock().error.is_some() {
                        return self.stop_slot("Could not reopen the session");
                    }
                    continue;
                }
                Err(e) => {
                    tracing::warn!("spin failed: {e}");
                    return self.stop_slot(format!("Error: {e}"));
                }
            };
            reopened = 0;
            let hit = cfg.stop_mult.map(|m| r.multiplier >= m).unwrap_or(false);
            let record = SpinRecord {
                at: SystemTime::now(),
                round_id: r.round_id.clone(),
                feature: cfg.feature.clone(),
                cost: r.cost,
                win: r.win,
                multiplier: r.multiplier,
                balance: r.balance,
                free_spins: r.free_spins,
                ms: r.ms,
                currency: info.currency.clone(),
                decimals: info.decimals,
                hit,
            };
            self.emit(Event::Spin(record));
            let (profit, done, stop_on_win) = {
                let mut s = self.slot.lock();
                s.done += 1;
                s.profit += r.win - r.cost;
                if r.balance.is_some() {
                    s.balance = r.balance;
                }
                (s.profit, s.done, s.stop_on_win)
            };
            if hit {
                self.pause_slot(Some(true), Some(format!("Hit {:.2}× · paused", r.multiplier)));
                continue;
            }
            if stop_on_win && r.win > 0 {
                return self.stop_slot("Stopped on win");
            }
            if let Some(sp) = cfg.stop_profit {
                if profit >= sp {
                    return self.stop_slot("Stop profit reached");
                }
            }
            if let Some(sl) = cfg.stop_loss {
                if -profit >= sl {
                    return self.stop_slot("Stop loss reached");
                }
            }
            if cfg.limit > 0 && done >= cfg.limit {
                return self.stop_slot("Done");
            }
            self.emit_slot();
            quiet += 1;
            if quiet >= 60 && throttle_ms > 0 {
                throttle_ms = throttle_ms.saturating_sub(50);
                quiet = 0;
            }
            let delay = cfg.delay_ms.max(throttle_ms);
            if delay > 0 {
                tokio::time::sleep(Duration::from_millis(delay)).await;
            }
        }
    }
}

/// Follows the casino's launcher redirect (plain HTTP, no browser needed) and
/// returns the final URL whose query string carries the launch parameters.
async fn resolve_launcher(url: &str) -> Result<String, String> {
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/154.0.0.0 Safari/537.36")
        .build()
        .map_err(|e| e.to_string())?;
    let mut current = url.to_string();
    for _ in 0..6 {
        let res = client.get(&current).send().await.map_err(|e| format!("launcher: {e}"))?;
        if res.status().is_redirection() {
            if let Some(loc) = res.headers().get("location").and_then(|v| v.to_str().ok()) {
                current = if loc.starts_with("http") { loc.to_string() } else { format!("{}{}", current.split('/').take(3).collect::<Vec<_>>().join("/"), loc) };
                continue;
            }
        }
        return Ok(current);
    }
    Ok(current)
}
