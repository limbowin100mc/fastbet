//! The betting engine. Runs on its own tokio runtime thread, owns the casino
//! connection, the single bet lane, seed rotation and the autobet loop.
//! The UI talks to it with [`Command`]s and listens for [`Event`]s.

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::{mpsc as std_mpsc, Arc};
use std::time::{Duration, Instant, SystemTime};

use parking_lot::Mutex;
use tokio::sync::{mpsc, Notify};
use tokio::task::JoinHandle;

use crate::casino::{self, BetRequest, Casino, CasinoError, CasinoEvent, CasinoId, ConnStatus, Market, ProvableState};
use crate::money::{fmt_multi, mul_f, scale_pct, Units};

const BUSY_RETRIES: u32 = 8;
pub const MAX_QUEUE: usize = 10;

#[derive(Debug)]
pub enum Command {
    SetCasino { id: CasinoId, setup: casino::Setup },
    SetToken(Option<String>),
    /// Reconnect through another of the casino's domains.
    SetMirror(String),
    /// Cloudflare cookie from the user's browser, or none.
    SetClearance(Option<crate::cloudflare::Clearance>),
    Bet { req: BetRequest, hit_line: Option<f64> },
    StartAuto(AutoConfig),
    StopAuto,
    /// Finish the run after the next winning bet.
    StopOnNextWin,
    TogglePause,
    RotateSeed,
    /// Bets between automatic seed changes, 0 = off.
    SeedPolicy { every: u32 },
    /// Async mode: bets allowed in flight at once (1 = sequential).
    SetConcurrency(u32),
    /// Slot catalog query; answered with `Event::Slots`.
    ListSlots { kind: SlotListKind, filter: crate::casino::SlotFilter },
    OpenSlot { game: crate::slots::SlotGame, mode: crate::slots::PlayMode, currency: String },
    CloseSlot,
    StartSlot(slots::SlotConfig),
    StopSlot,
    PauseSlot,
    SlotStopOnNextWin,
    Shutdown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SlotListKind {
    Featured,
    All,
    Search,
}

pub const MAX_CONCURRENCY: u32 = 8;

#[derive(Clone, Debug)]
pub enum Event {
    Status(ConnStatus, String),
    Market(Market),
    Provable(ProvableState),
    Bet(BetRecord),
    SeedRotated { at: SystemTime, client_seed: String, reason: &'static str },
    Retry,
    Queue(usize),
    Seed(SeedInfo),
    Auto(AutoState),
    Flash(String),
    Slots { kind: SlotListKind, skip: usize, games: Vec<crate::slots::SlotGame>, error: Option<String> },
    Slot(slots::SlotState),
    Spin(slots::SpinRecord),
}

#[derive(Clone, Copy, Debug, Default)]
pub struct SeedInfo {
    pub count: u32,
    pub rotations: u32,
    pub busy: bool,
}

#[derive(Clone, Debug)]
pub struct BetRecord {
    pub at: SystemTime,
    pub nonce: Option<u64>,
    pub currency: String,
    pub target: f64,
    pub outcome: f64,
    pub win: bool,
    pub hit: bool,
    pub amount: Units,
    pub payout: Units,
    pub profit: Units,
    pub ms: f64,
}

#[derive(Clone, Debug)]
pub struct AutoConfig {
    pub currency: String,
    pub target: f64,
    pub base: Units,
    /// 0 = no limit
    pub limit: u32,
    pub delay_ms: u64,
    /// Percent change of the stake after a win / loss. `None` resets to base.
    pub on_win: Option<f64>,
    pub on_loss: Option<f64>,
    pub stop_profit: Option<Units>,
    pub stop_loss: Option<Units>,
    pub max_bet: Option<Units>,
    pub hit_line: Option<f64>,
}

#[derive(Clone, Debug, Default)]
pub struct AutoState {
    pub running: bool,
    pub paused: bool,
    pub msg: String,
    pub done: u32,
    pub profit: Units,
    pub stake: Units,
    pub hits: u32,
    /// Outcome of the hit that ended the last run.
    pub last_hit: Option<f64>,
    /// Armed by "Stop on next win".
    pub stop_on_win: bool,
}

#[derive(Debug)]
enum BetError {
    Cancelled,
    Other(String),
}

impl From<CasinoError> for BetError {
    fn from(e: CasinoError) -> Self {
        BetError::Other(e.to_string())
    }
}

/// What the UI holds: a command sender and an event receiver.
pub struct EngineHandle {
    pub tx: mpsc::UnboundedSender<Command>,
    pub rx: std_mpsc::Receiver<Event>,
}

impl EngineHandle {
    pub fn send(&self, cmd: Command) {
        let _ = self.tx.send(cmd);
    }
}

type Repaint = Arc<dyn Fn() + Send + Sync>;
type KeepGoing = Arc<dyn Fn() -> bool + Send + Sync>;

struct Engine {
    casino: Mutex<Option<Arc<dyn Casino>>>,
    forwarder: Mutex<Option<JoinHandle<()>>>,
    events: std_mpsc::Sender<Event>,
    repaint: Repaint,

    /// Bets hold a read guard while in flight; a seed rotation takes the write
    /// guard so it only runs once the pipeline has drained.
    lane: tokio::sync::RwLock<()>,
    /// Caps how many bets are in flight at once (async mode).
    slots: Mutex<Arc<tokio::sync::Semaphore>>,
    concurrency: AtomicU32,
    queued: AtomicUsize,

    seed_every: AtomicU32,
    seed_count: AtomicU32,
    rotations: AtomicU32,
    seed_busy: AtomicBool,

    auto: Mutex<AutoState>,
    auto_cfg: Mutex<Option<AutoConfig>>,
    auto_run: AtomicU64,
    auto_wake: Notify,

    slot: Mutex<slots::SlotState>,
    slot_session: tokio::sync::Mutex<Option<Arc<dyn crate::slots::SlotSession>>>,
    slot_run: AtomicU64,
    slot_wake: Notify,
}

pub mod slots;

#[cfg(test)]
mod tests;

impl Engine {
    fn new(events: std_mpsc::Sender<Event>, repaint: Repaint) -> Arc<Self> {
        Arc::new(Engine {
            casino: Mutex::new(None),
            forwarder: Mutex::new(None),
            events,
            repaint,
            lane: tokio::sync::RwLock::new(()),
            slots: Mutex::new(Arc::new(tokio::sync::Semaphore::new(1))),
            concurrency: AtomicU32::new(1),
            queued: AtomicUsize::new(0),
            seed_every: AtomicU32::new(0),
            seed_count: AtomicU32::new(0),
            rotations: AtomicU32::new(0),
            seed_busy: AtomicBool::new(false),
            auto: Mutex::new(AutoState::default()),
            auto_cfg: Mutex::new(None),
            auto_run: AtomicU64::new(0),
            auto_wake: Notify::new(),
            slot: Mutex::new(slots::SlotState::default()),
            slot_session: tokio::sync::Mutex::new(None),
            slot_run: AtomicU64::new(0),
            slot_wake: Notify::new(),
        })
    }
}

/// Starts the engine thread. Call once.
pub fn start(repaint: Repaint) -> EngineHandle {
    let (cmd_tx, cmd_rx) = mpsc::unbounded_channel::<Command>();
    let (ev_tx, ev_rx) = std_mpsc::channel::<Event>();

    std::thread::Builder::new()
        .name("fastbet-engine".into())
        .spawn(move || {
            let rt = tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .enable_all()
                .build()
                .expect("tokio runtime");
            rt.block_on(async move {
                Engine::new(ev_tx, repaint).command_loop(cmd_rx).await;
            });
        })
        .expect("engine thread");

    EngineHandle { tx: cmd_tx, rx: ev_rx }
}

impl Engine {
    fn emit(&self, ev: Event) {
        let _ = self.events.send(ev);
        (self.repaint)();
    }

    fn flash(&self, msg: impl Into<String>) {
        self.emit(Event::Flash(msg.into()));
    }

    fn casino(&self) -> Result<Arc<dyn Casino>, BetError> {
        self.casino.lock().clone().ok_or_else(|| BetError::Other("No casino connected".into()))
    }

    async fn command_loop(self: Arc<Self>, mut rx: mpsc::UnboundedReceiver<Command>) {
        while let Some(cmd) = rx.recv().await {
            match cmd {
                Command::SetCasino { id, setup } => self.set_casino(id, setup),
                Command::SetToken(token) => {
                    if let Some(c) = self.casino.lock().clone() {
                        c.set_token(token);
                    }
                }
                Command::SetMirror(mirror) => {
                    if let Some(c) = self.casino.lock().clone() {
                        c.set_mirror(&mirror);
                    }
                }
                Command::SetClearance(clearance) => {
                    if let Some(c) = self.casino.lock().clone() {
                        c.set_clearance(clearance);
                    }
                }
                Command::Bet { req, hit_line } => {
                    if self.auto.lock().running {
                        continue;
                    }
                    if self.queued.load(Ordering::SeqCst) >= MAX_QUEUE {
                        self.flash(format!("{MAX_QUEUE} bets already queued"));
                        continue;
                    }
                    let me = self.clone();
                    tokio::spawn(async move {
                        let keep: KeepGoing = Arc::new(|| true);
                        match me.place_bet(req, hit_line, keep).await {
                            Ok(_) | Err(BetError::Cancelled) => {}
                            Err(BetError::Other(e)) => {
                                tracing::warn!("bet failed: {e}");
                                me.flash(e);
                            }
                        }
                    });
                }
                Command::StartAuto(cfg) => self.start_auto(cfg),
                Command::StopAuto => self.stop_auto("Stopped"),
                Command::StopOnNextWin => {
                    let mut a = self.auto.lock();
                    if a.running {
                        a.stop_on_win = !a.stop_on_win;
                        drop(a);
                        self.emit_auto();
                    }
                }
                Command::TogglePause => self.toggle_pause(),
                Command::RotateSeed => {
                    let me = self.clone();
                    tokio::spawn(async move {
                        let _g = me.lane.write().await;
                        if let Err(e) = me.rotate_seed("manual").await {
                            me.flash(e.to_string());
                        }
                    });
                }
                Command::SeedPolicy { every } => {
                    let prev = self.seed_every.swap(every, Ordering::SeqCst);
                    if prev == 0 && every > 0 {
                        self.seed_count.store(0, Ordering::SeqCst);
                    }
                    self.emit_seed();
                }
                Command::SetConcurrency(n) => {
                    let n = n.clamp(1, MAX_CONCURRENCY);
                    if self.concurrency.swap(n, Ordering::SeqCst) != n {
                        *self.slots.lock() = Arc::new(tokio::sync::Semaphore::new(n as usize));
                    }
                }
                Command::ListSlots { kind, filter } => {
                    let me = self.clone();
                    tokio::spawn(async move {
                        let skip = filter.skip;
                        let casino = me.casino.lock().clone();
                        let res = match casino {
                            Some(c) => c.list_slots(filter).await.map_err(|e| e.to_string()),
                            None => Err("Not connected".into()),
                        };
                        let (games, error) = match res {
                            Ok(g) => (g, None),
                            Err(e) => (vec![], Some(e)),
                        };
                        me.emit(Event::Slots { kind, skip, games, error });
                    });
                }
                Command::OpenSlot { game, mode, currency } => {
                    tokio::spawn(self.clone().open_slot(game, mode, currency));
                }
                Command::CloseSlot => {
                    let me = self.clone();
                    tokio::spawn(async move { me.close_slot().await });
                }
                Command::StartSlot(cfg) => self.start_slot(cfg),
                Command::StopSlot => self.stop_slot("Stopped"),
                Command::PauseSlot => self.pause_slot(None, None),
                Command::SlotStopOnNextWin => {
                    let mut s = self.slot.lock();
                    if s.running {
                        s.stop_on_win = !s.stop_on_win;
                        drop(s);
                        self.emit_slot();
                    }
                }
                Command::Shutdown => break,
            }
        }
        self.close_slot().await;
        if let Some(c) = self.casino.lock().take() {
            c.shutdown();
        }
    }

    fn set_casino(self: &Arc<Self>, id: CasinoId, setup: casino::Setup) {
        if let Some(old) = self.casino.lock().take() {
            old.shutdown();
        }
        if let Some(f) = self.forwarder.lock().take() {
            f.abort();
        }
        self.stop_auto("Stopped");
        self.seed_count.store(0, Ordering::SeqCst);
        self.rotations.store(0, Ordering::SeqCst);
        self.emit_seed();

        let (tx, mut rx) = mpsc::unbounded_channel::<CasinoEvent>();
        let casino = casino::connect(id, setup, tx);
        tracing::info!("connecting to {}", casino.id().name());
        *self.casino.lock() = Some(casino);

        let me = self.clone();
        let fwd = tokio::spawn(async move {
            while let Some(ev) = rx.recv().await {
                me.emit(match ev {
                    CasinoEvent::Status(s, d) => Event::Status(s, d),
                    CasinoEvent::Market(m) => Event::Market(m),
                    CasinoEvent::Provable(p) => Event::Provable(p),
                });
            }
        });
        *self.forwarder.lock() = Some(fwd);
    }

    // ------------------------------------------------------------- seed

    fn emit_seed(&self) {
        self.emit(Event::Seed(SeedInfo {
            count: self.seed_count.load(Ordering::SeqCst),
            rotations: self.rotations.load(Ordering::SeqCst),
            busy: self.seed_busy.load(Ordering::SeqCst),
        }));
    }

    /// Only call while holding the lane.
    async fn rotate_seed(&self, reason: &'static str) -> Result<(), CasinoError> {
        let casino = self.casino.lock().clone().ok_or(CasinoError::NotConnected)?;
        self.seed_busy.store(true, Ordering::SeqCst);
        self.emit_seed();
        let client_seed = random_seed();
        let res = casino.rotate_seed(&client_seed).await;
        self.seed_busy.store(false, Ordering::SeqCst);
        match res {
            Ok(st) => {
                self.seed_count.store(0, Ordering::SeqCst);
                self.rotations.fetch_add(1, Ordering::SeqCst);
                let cs = st.client_seed.clone().unwrap_or(client_seed);
                self.emit(Event::Provable(st));
                self.emit(Event::SeedRotated { at: SystemTime::now(), client_seed: cs, reason });
                self.emit_seed();
                Ok(())
            }
            Err(e) => {
                self.emit_seed();
                Err(CasinoError::Server(format!("Seed change failed: {e}")))
            }
        }
    }

    // ------------------------------------------------------------- bets

    async fn place_bet(&self, req: BetRequest, hit_line: Option<f64>, keep_going: KeepGoing) -> Result<BetRecord, BetError> {
        /// Counts the bet out even when the future is dropped mid-flight
        /// (a stopped auto run aborts its JoinSet), so the queue never sticks.
        struct Queued<'a>(&'a Engine);
        impl Drop for Queued<'_> {
            fn drop(&mut self) {
                let left = self.0.queued.fetch_sub(1, Ordering::SeqCst).saturating_sub(1);
                self.0.emit(Event::Queue(left));
            }
        }
        let n = self.queued.fetch_add(1, Ordering::SeqCst) + 1;
        self.emit(Event::Queue(n));
        let _queued = Queued(self);
        self.place_bet_inner(req, hit_line, keep_going).await
    }

    async fn place_bet_inner(&self, req: BetRequest, hit_line: Option<f64>, keep_going: KeepGoing) -> Result<BetRecord, BetError> {
        if !keep_going() {
            return Err(BetError::Cancelled);
        }
        let every = self.seed_every.load(Ordering::SeqCst);
        if every > 0 && self.seed_count.load(Ordering::SeqCst) >= every {
            // exclusive: waits for every in-flight bet, and re-checks because
            // another bet may have rotated while we waited
            let _w = self.lane.write().await;
            if self.seed_count.load(Ordering::SeqCst) >= every {
                self.rotate_seed("auto").await?;
            }
        }
        let sem = self.slots.lock().clone();
        let _permit = sem.acquire_owned().await.map_err(|_| BetError::Other("engine closed".into()))?;
        let _lane = self.lane.read().await;
        if !keep_going() {
            return Err(BetError::Cancelled);
        }
        self.seed_count.fetch_add(1, Ordering::SeqCst);
        self.emit_seed();

        let casino = self.casino()?;
        let mut attempt = 0u32;
        let (reply, ms) = loop {
            let t0 = Instant::now();
            match casino.bet_limbo(&req).await {
                Ok(r) => break (r, t0.elapsed().as_secs_f64() * 1000.0),
                Err(e) if e.is_busy() && attempt < BUSY_RETRIES => {
                    self.emit(Event::Retry);
                    tokio::time::sleep(Duration::from_millis((50u64 << attempt).min(1000))).await;
                    attempt += 1;
                    if !keep_going() {
                        self.seed_count.fetch_sub(1, Ordering::SeqCst);
                        return Err(BetError::Cancelled);
                    }
                }
                Err(e) => {
                    self.seed_count.fetch_sub(1, Ordering::SeqCst);
                    self.emit_seed();
                    return Err(e.into());
                }
            }
        };

        // The reply's `multiplier` is the payout multiplier (0 on a loss), so the
        // target is always the autoCashout we sent.
        let amount = reply.amount.unwrap_or(req.amount);
        let win = reply.win.unwrap_or_else(|| match reply.winnings {
            Some(p) => p > 0,
            None => reply.outcome >= req.target,
        });
        let payout = reply.winnings.unwrap_or(if win { mul_f(amount, req.target) } else { 0 });
        let record = BetRecord {
            at: SystemTime::now(),
            nonce: reply.nonce,
            currency: req.currency,
            target: req.target,
            outcome: reply.outcome,
            win,
            hit: hit_line.map(|l| reply.outcome >= l).unwrap_or(false),
            amount,
            payout,
            profit: payout - amount,
            ms,
        };
        self.emit(Event::Bet(record.clone()));
        Ok(record)
    }

    // ------------------------------------------------------------- autobet

    fn emit_auto(&self) {
        let st = self.auto.lock().clone();
        self.emit(Event::Auto(st));
    }

    fn start_auto(self: &Arc<Self>, cfg: AutoConfig) {
        if self.auto.lock().running {
            return;
        }
        let run = self.auto_run.fetch_add(1, Ordering::SeqCst) + 1;
        {
            let mut a = self.auto.lock();
            *a = AutoState {
                running: true,
                paused: false,
                msg: "Running".into(),
                done: 0,
                profit: 0,
                stake: cfg.base,
                hits: 0,
                last_hit: None,
                stop_on_win: false,
            };
        }
        *self.auto_cfg.lock() = Some(cfg.clone());
        self.emit_auto();
        tokio::spawn(self.clone().worker(run, cfg));
    }

    fn stop_auto(&self, msg: impl Into<String>) {
        let mut a = self.auto.lock();
        if !a.running {
            return;
        }
        a.running = false;
        a.paused = false;
        a.msg = msg.into();
        drop(a);
        self.auto_wake.notify_one();
        self.emit_auto();
    }

    fn toggle_pause(&self) {
        let mut a = self.auto.lock();
        if !a.running {
            return;
        }
        a.paused = !a.paused;
        a.msg = if a.paused { "Paused" } else { "Running" }.into();
        let resumed = !a.paused;
        drop(a);
        if resumed {
            self.auto_wake.notify_one();
        }
        self.emit_auto();
    }

    fn auto_live(&self, run: u64) -> bool {
        self.auto_run.load(Ordering::SeqCst) == run && self.auto.lock().running
    }

    async fn worker(self: Arc<Self>, run: u64, cfg: AutoConfig) {
        let keep: KeepGoing = {
            let me = self.clone();
            Arc::new(move || me.auto_live(run))
        };
        // A progressive stake depends on the previous result, so it stays sequential.
        let progressive = cfg.on_win.is_some() || cfg.on_loss.is_some();
        let mut set: tokio::task::JoinSet<Result<BetRecord, BetError>> = tokio::task::JoinSet::new();
        let mut inflight: u32 = 0;
        let mut launched: u32 = 0;
        loop {
            if !self.auto_live(run) {
                return;
            }
            let paused = self.auto.lock().paused;
            let width = if progressive { 1 } else { self.concurrency.load(Ordering::SeqCst).max(1) };
            let under_limit = cfg.limit == 0 || launched < cfg.limit;
            if !paused && inflight < width && under_limit {
                let amount = self.auto.lock().stake;
                if amount <= 0 {
                    return self.stop_auto("Bet size hit 0");
                }
                if let Some(max) = cfg.max_bet {
                    if amount > max {
                        return self.stop_auto("Next bet over max bet");
                    }
                }
                let req = BetRequest { currency: cfg.currency.clone(), amount, target: cfg.target };
                let me = self.clone();
                let k = keep.clone();
                let hit_line = cfg.hit_line;
                set.spawn(async move { me.place_bet(req, hit_line, k).await });
                inflight += 1;
                launched += 1;
                continue;
            }
            if inflight == 0 {
                if paused {
                    self.auto_wake.notified().await;
                    continue;
                }
                // nothing left to launch and nothing in flight
                return self.stop_auto("Done");
            }
            let res = match set.join_next().await {
                Some(Ok(r)) => r,
                Some(Err(_)) | None => {
                    inflight = inflight.saturating_sub(1);
                    continue;
                }
            };
            inflight -= 1;
            let r = match res {
                Ok(r) => r,
                Err(BetError::Cancelled) => continue,
                Err(BetError::Other(e)) => {
                    tracing::warn!("auto bet failed: {e}");
                    if self.auto_live(run) {
                        let msg = if e.starts_with("Seed") { e } else { format!("Error: {e}") };
                        self.stop_auto(msg);
                    }
                    return;
                }
            };
            if self.auto_run.load(Ordering::SeqCst) != run {
                return;
            }
            let (profit, done, hit, stop_on_win) = {
                let mut a = self.auto.lock();
                a.done += 1;
                a.profit += r.profit;
                let pct = if r.win { cfg.on_win } else { cfg.on_loss };
                a.stake = match pct {
                    Some(p) => scale_pct(a.stake, p),
                    None => cfg.base,
                };
                if r.hit {
                    a.hits += 1;
                }
                (a.profit, a.done, r.hit, a.stop_on_win)
            };
            if hit && self.auto_live(run) {
                self.auto.lock().last_hit = Some(r.outcome);
                return self.stop_auto(format!("HIT {}", fmt_multi(r.outcome)));
            }
            if stop_on_win && r.win && self.auto_live(run) {
                return self.stop_auto("Stopped on win");
            }
            if !self.auto_live(run) {
                return;
            }
            if let Some(sp) = cfg.stop_profit {
                if profit >= sp {
                    return self.stop_auto("Stop profit reached");
                }
            }
            if let Some(sl) = cfg.stop_loss {
                if -profit >= sl {
                    return self.stop_auto("Stop loss reached");
                }
            }
            if cfg.limit > 0 && done >= cfg.limit {
                return self.stop_auto("Done");
            }
            self.emit_auto();
            if cfg.delay_ms > 0 {
                tokio::time::sleep(Duration::from_millis(cfg.delay_ms)).await;
            }
        }
    }
}

fn random_seed() -> String {
    use rand::RngCore;
    let mut bytes = [0u8; 12];
    rand::rng().fill_bytes(&mut bytes);
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
