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
use crate::originals::{Game, Round, Step, Tables};

const BUSY_RETRIES: u32 = 8;
pub const MAX_QUEUE: usize = 10;
/// Longest wait before force restart tries again after passes that settled nothing.
const RESTART_WAIT_MAX: Duration = Duration::from_secs(8);

#[derive(Debug)]
pub enum Command {
    SetCasino { id: CasinoId, setup: casino::Setup },
    SetToken(Option<String>),
    /// Reconnect through another of the casino's domains.
    SetMirror(String),
    /// Cloudflare cookie from the user's browser, or none.
    SetClearance(Option<crate::cloudflare::Clearance>),
    Bet { req: BetRequest, hit_line: Option<f64> },
    /// A round of an original: settles at once, or opens one that `OriginalStep`s carry on.
    Original { bet: OriginalBet, hit_line: Option<f64> },
    /// Next move in the open round of an original, or its cash out.
    OriginalStep { game: Game, step: Step, params: serde_json::Value },
    /// Picks up a round of `game` left open (answered with `Event::Round` when there is one).
    OriginalResume(Game),
    /// Payout tables and the casino's list of originals.
    OriginalInfo,
    StartAuto(AutoConfig),
    StopAuto,
    /// Force restart betting. `always`: a run that ends by itself starts again from the base bet.
    /// `on_error`: an error doesn't end the run, it bets on with the same bet size and counters.
    ForceRestart { always: bool, on_error: bool },
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
    /// Every answer about an original's round, open or settled (settled ones also come as `Bet`).
    Round(Round),
    /// A bet or step of an original failed (the engine flashes why); the board stops waiting.
    RoundFailed { game: Game },
    OriginalTables(Tables),
    Originals(Vec<crate::slots::SlotGame>),
}

/// A bet on an original, before the casino adds its own fields.
#[derive(Clone, Debug)]
pub struct OriginalBet {
    pub game: Game,
    pub currency: String,
    pub amount: Units,
    /// The game's own fields (tiles, risk, target …).
    pub params: serde_json::Value,
    /// Part of an auto run.
    pub auto: bool,
}

/// What a bet is placed on.
#[derive(Clone, Debug)]
enum Wager {
    Limbo(BetRequest),
    Original(OriginalBet),
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
    /// The original it was placed on; `None` for Limbo.
    pub game: Option<Game>,
    /// The original's settled round, for its board and history.
    pub round: Option<Arc<Round>>,
}

impl BetRecord {
    /// A settled round of an original. A round wins when it pays back at least the stake.
    pub fn from_round(r: &Round, ms: f64, hit_line: Option<f64>) -> Self {
        let stake = r.stake();
        Self {
            at: SystemTime::now(),
            nonce: None,
            currency: r.currency.clone(),
            target: r.multiplier,
            outcome: r.multiplier,
            win: r.multiplier >= 1.0,
            hit: hit_line.is_some_and(|l| r.multiplier >= l),
            amount: stake,
            payout: r.payout,
            profit: r.payout - stake,
            ms,
            game: Some(r.game),
            round: Some(Arc::new(r.clone())),
        }
    }
}

#[derive(Clone, Debug)]
pub struct AutoConfig {
    pub currency: String,
    /// Limbo's target; unused when `original` is set.
    pub target: f64,
    /// Auto bets on an original instead of Limbo: the game and its fields for every bet.
    pub original: Option<(Game, serde_json::Value)>,
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
    /// Times force restart started the run again.
    pub restarts: u32,
    /// Errors the run bet on through (force restart on errors).
    pub errors: u32,
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
type BetSet = tokio::task::JoinSet<Result<Option<BetRecord>, BetError>>;

/// How one pass of an auto run ended.
enum Ending {
    /// Stop, Stop on next win, or another run took over: nothing more to do.
    Stopped,
    /// The run ended by itself (a stop, its number of bets, an error): force restart starts it again.
    Over(String),
    /// The casino kept a round open: only the user can finish it, on the board.
    Stuck(String),
}

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
    force_restart: AtomicBool,
    restart_on_error: AtomicBool,

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
            force_restart: AtomicBool::new(false),
            restart_on_error: AtomicBool::new(false),
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
                        match me.place_bet(Wager::Limbo(req), hit_line, keep).await {
                            Ok(_) | Err(BetError::Cancelled) => {}
                            Err(BetError::Other(e)) => {
                                tracing::warn!("bet failed: {e}");
                                me.flash(e);
                            }
                        }
                    });
                }
                Command::Original { bet, hit_line } => {
                    if self.auto.lock().running {
                        continue;
                    }
                    if self.queued.load(Ordering::SeqCst) >= MAX_QUEUE {
                        self.flash(format!("{MAX_QUEUE} bets already queued"));
                        continue;
                    }
                    let me = self.clone();
                    tokio::spawn(async move {
                        let game = bet.game;
                        let keep: KeepGoing = Arc::new(|| true);
                        match me.place_bet(Wager::Original(bet), hit_line, keep).await {
                            Ok(_) => {}
                            Err(BetError::Cancelled) => me.emit(Event::RoundFailed { game }),
                            Err(BetError::Other(e)) => me.original_failed(game, e).await,
                        }
                    });
                }
                Command::OriginalStep { game, step, params } => {
                    tokio::spawn(self.clone().original_step(game, step, params));
                }
                Command::OriginalResume(game) => {
                    let me = self.clone();
                    tokio::spawn(async move {
                        let Ok(casino) = me.casino() else { return };
                        match casino.original_active(game).await {
                            Ok(Some(round)) => me.emit(Event::Round(round)),
                            Ok(None) => {}
                            Err(e) => tracing::info!("no open {} round: {e}", game.name()),
                        }
                    });
                }
                Command::OriginalInfo => {
                    let me = self.clone();
                    tokio::spawn(async move {
                        let Ok(casino) = me.casino() else { return };
                        match casino.original_tables().await {
                            Ok(t) => me.emit(Event::OriginalTables(t)),
                            Err(e) => tracing::info!("payout tables: {e}"),
                        }
                        match casino.list_originals().await {
                            Ok(list) if !list.is_empty() => me.emit(Event::Originals(list)),
                            Ok(_) => {}
                            Err(e) => tracing::info!("originals list: {e}"),
                        }
                    });
                }
                Command::StartAuto(cfg) => self.start_auto(cfg),
                Command::StopAuto => self.stop_auto("Stopped"),
                Command::ForceRestart { always, on_error } => {
                    self.force_restart.store(always, Ordering::SeqCst);
                    self.restart_on_error.store(on_error, Ordering::SeqCst);
                }
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

    /// Places one bet through the lane. `Ok(None)`: an original's round opened and waits for steps.
    async fn place_bet(&self, wager: Wager, hit_line: Option<f64>, keep_going: KeepGoing) -> Result<Option<BetRecord>, BetError> {
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
        self.place_bet_inner(wager, hit_line, keep_going).await
    }

    async fn place_bet_inner(&self, wager: Wager, hit_line: Option<f64>, keep_going: KeepGoing) -> Result<Option<BetRecord>, BetError> {
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
            let res = match &wager {
                Wager::Limbo(req) => casino.bet_limbo(req).await.map(Reply::Limbo),
                Wager::Original(b) => casino.original_bet(b.game, &b.currency, b.amount, b.params.clone(), b.auto).await.map(Reply::Original),
            };
            match res {
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

        let (req, reply) = match (wager, reply) {
            (Wager::Limbo(req), Reply::Limbo(reply)) => (req, reply),
            (_, Reply::Original(mut round)) => {
                self.emit(Event::Round(round.clone()));
                // bonus spins need no decisions: play them out
                let mut spins = 0;
                while round.active && round.game.plays_itself() && spins < 500 {
                    spins += 1;
                    round = casino.original_step(round.game, Step::Next, serde_json::json!({})).await.map_err(BetError::from)?;
                    self.emit(Event::Round(round.clone()));
                }
                if round.active {
                    return Ok(None);
                }
                let record = BetRecord::from_round(&round, ms, hit_line);
                self.emit(Event::Bet(record.clone()));
                return Ok(Some(record));
            }
            (Wager::Original(_), Reply::Limbo(_)) => unreachable!("a Limbo reply to an original's bet"),
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
            game: None,
            round: None,
        };
        self.emit(Event::Bet(record.clone()));
        Ok(Some(record))
    }

    /// A manual bet on an original failed. An open round is the usual cause: show it so it can be finished.
    async fn original_failed(&self, game: Game, error: String) {
        tracing::warn!("{} bet failed: {error}", game.name());
        self.emit(Event::RoundFailed { game });
        if error == CasinoError::RoundOpen.to_string() {
            if let Ok(casino) = self.casino() {
                if let Ok(Some(r)) = casino.original_active(game).await {
                    self.emit(Event::Round(r));
                }
            }
            self.flash("Finish the open round first");
        } else {
            self.flash(error);
        }
    }

    /// Reveal, guess or cash out in the open round of an original. Goes through the lane so a
    /// seed change never lands in the middle of it.
    async fn original_step(self: Arc<Self>, game: Game, step: Step, params: serde_json::Value) {
        let _lane = self.lane.read().await;
        let res = async {
            let casino = self.casino().map_err(|_| "No casino connected".to_string())?;
            let mut attempt = 0u32;
            loop {
                let t0 = Instant::now();
                match casino.original_step(game, step, params.clone()).await {
                    Ok(r) => return Ok((r, t0.elapsed().as_secs_f64() * 1000.0)),
                    Err(e) if e.is_busy() && attempt < BUSY_RETRIES => {
                        self.emit(Event::Retry);
                        tokio::time::sleep(Duration::from_millis((50u64 << attempt).min(1000))).await;
                        attempt += 1;
                    }
                    Err(e) => return Err(e.to_string()),
                }
            }
        }
        .await;
        match res {
            Ok((round, ms)) => {
                self.emit(Event::Round(round.clone()));
                if !round.active {
                    self.emit(Event::Bet(BetRecord::from_round(&round, ms, None)));
                }
            }
            Err(e) => {
                tracing::warn!("{} step failed: {e}", game.name());
                self.emit(Event::RoundFailed { game });
                self.flash(e);
            }
        }
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
                restarts: 0,
                errors: 0,
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

    /// Runs `cfg` until it ends; with force restart on, starts it again each time it ends by itself.
    async fn worker(self: Arc<Self>, mut run: u64, cfg: AutoConfig) {
        // passes in a row that settled nothing (the casino may be down): wait longer before each try
        let mut idle: u32 = 0;
        loop {
            let mut set = BetSet::new();
            let (ending, settled) = self.auto_pass(run, &cfg, &mut set).await;
            let msg = match ending {
                Ending::Stopped => return,
                Ending::Stuck(m) => return self.stop_auto(m),
                Ending::Over(m) => m,
            };
            if !self.force_restart.load(Ordering::SeqCst) || !self.auto_live(run) {
                return self.stop_auto(msg);
            }
            // a new run id cancels this pass's bets not sent yet; the ones sent still settle
            run = self.auto_run.fetch_add(1, Ordering::SeqCst) + 1;
            while set.join_next().await.is_some() {}
            idle = if settled == 0 { idle + 1 } else { 0 };
            // the run's own delay between bets, longer after passes that settled nothing
            let wait = if idle > 0 {
                let wait = Duration::from_secs(1 << (idle - 1).min(3)).min(RESTART_WAIT_MAX);
                self.auto.lock().msg = format!("{msg} · next try in {}s", wait.as_secs());
                self.emit_auto();
                wait
            } else {
                Duration::from_millis(cfg.delay_ms)
            };
            if !wait.is_zero() {
                // Stop wakes it
                tokio::select! {
                    _ = tokio::time::sleep(wait) => {}
                    _ = self.auto_wake.notified() => {}
                }
            }
            if !self.auto_live(run) {
                return;
            }
            tracing::info!("auto: force restart after \"{msg}\"");
            {
                let mut a = self.auto.lock();
                a.done = 0;
                a.profit = 0;
                a.stake = cfg.base;
                a.hits = 0;
                a.restarts += 1;
                a.msg = format!("Restarted: {msg}");
            }
            self.emit_auto();
        }
    }

    /// One pass of an auto run: from the base bet until something ends it. Returns how it
    /// ended and how many bets it settled.
    async fn auto_pass(self: &Arc<Self>, run: u64, cfg: &AutoConfig, set: &mut BetSet) -> (Ending, u32) {
        let keep: KeepGoing = {
            let me = self.clone();
            Arc::new(move || me.auto_live(run))
        };
        // A progressive stake depends on the previous result, so it stays sequential. So does an
        // original with steps: the casino holds one round of it at a time.
        let progressive = cfg.on_win.is_some() || cfg.on_loss.is_some() || cfg.original.as_ref().is_some_and(|(g, _)| g.has_steps());
        let mut inflight: u32 = 0;
        let mut launched: u32 = 0;
        let mut settled: u32 = 0;
        // errors since the last settled bet (force restart on errors)
        let mut errors_in_row: u32 = 0;
        let ending = loop {
            if !self.auto_live(run) {
                break Ending::Stopped;
            }
            let paused = self.auto.lock().paused;
            let width = if progressive { 1 } else { self.concurrency.load(Ordering::SeqCst).max(1) };
            let under_limit = cfg.limit == 0 || launched < cfg.limit;
            if !paused && inflight < width && under_limit {
                let amount = self.auto.lock().stake;
                // a run that starts at 0 plays for free on purpose; one that shrank to 0 is over
                if amount <= 0 && cfg.base > 0 {
                    break Ending::Over("Bet size hit 0".into());
                }
                if let Some(max) = cfg.max_bet {
                    if amount > max {
                        break Ending::Over("Next bet over max bet".into());
                    }
                }
                let wager = match &cfg.original {
                    Some((game, params)) => Wager::Original(OriginalBet { game: *game, currency: cfg.currency.clone(), amount, params: params.clone(), auto: true }),
                    None => Wager::Limbo(BetRequest { currency: cfg.currency.clone(), amount, target: cfg.target }),
                };
                let me = self.clone();
                let k = keep.clone();
                let hit_line = cfg.hit_line;
                set.spawn(async move { me.place_bet(wager, hit_line, k).await });
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
                break Ending::Over("Done".into());
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
                Ok(Some(r)) => r,
                Ok(None) => {
                    // the casino kept the round open (it wanted more picks than were given)
                    if self.auto_live(run) {
                        break Ending::Stuck("The round stayed open; finish it on the board".into());
                    }
                    break Ending::Stopped;
                }
                Err(BetError::Cancelled) => continue,
                Err(BetError::Other(e)) => {
                    tracing::warn!("auto bet failed: {e}");
                    if !self.auto_live(run) {
                        break Ending::Stopped;
                    }
                    let msg = if e.starts_with("Seed") { e } else { format!("Error: {e}") };
                    if !self.restart_on_error.load(Ordering::SeqCst) {
                        break Ending::Over(msg);
                    }
                    // bet on through it: the failed bet is placed again, with the same bet size and
                    // counters; errors in a row wait longer each time (Stop ends the wait)
                    launched = launched.saturating_sub(1);
                    errors_in_row += 1;
                    let wait = if errors_in_row > 1 { Duration::from_secs(1 << (errors_in_row - 2).min(3)).min(RESTART_WAIT_MAX) } else { Duration::ZERO };
                    {
                        let mut a = self.auto.lock();
                        a.errors += 1;
                        a.msg = if wait.is_zero() { format!("{msg} · betting on") } else { format!("{msg} · next try in {}s", wait.as_secs()) };
                    }
                    self.emit_auto();
                    if !wait.is_zero() {
                        tokio::select! {
                            _ = tokio::time::sleep(wait) => {}
                            _ = self.auto_wake.notified() => {}
                        }
                    }
                    continue;
                }
            };
            settled += 1;
            errors_in_row = 0;
            if self.auto_run.load(Ordering::SeqCst) != run {
                break Ending::Stopped;
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
                break Ending::Over(format!("HIT {}", fmt_multi(r.outcome)));
            }
            // asked for, like Stop: force restart leaves it stopped
            if stop_on_win && r.win && self.auto_live(run) {
                self.stop_auto("Stopped on win");
                break Ending::Stopped;
            }
            if !self.auto_live(run) {
                break Ending::Stopped;
            }
            if let Some(sp) = cfg.stop_profit {
                if profit >= sp {
                    break Ending::Over("Stop profit reached".into());
                }
            }
            if let Some(sl) = cfg.stop_loss {
                if -profit >= sl {
                    break Ending::Over("Stop loss reached".into());
                }
            }
            if cfg.limit > 0 && done >= cfg.limit {
                break Ending::Over("Done".into());
            }
            self.emit_auto();
            if cfg.delay_ms > 0 {
                tokio::time::sleep(Duration::from_millis(cfg.delay_ms)).await;
            }
        };
        (ending, settled)
    }
}

/// The casino's answer to a bet.
enum Reply {
    Limbo(casino::BetReply),
    Original(Round),
}

fn random_seed() -> String {
    use rand::RngCore;
    let mut bytes = [0u8; 12];
    rand::rng().fill_bytes(&mut bytes);
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
