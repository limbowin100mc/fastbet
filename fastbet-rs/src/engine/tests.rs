//! Engine tests against a scripted mock casino.

use super::*;
use crate::casino::BetReply;
use async_trait::async_trait;
use std::collections::VecDeque;

/// A casino that answers from a scripted list of outcomes.
struct Mock {
    outcomes: Mutex<VecDeque<f64>>,
    calls: AtomicU32,
    rotations: AtomicU32,
    busy_once: AtomicBool,
    /// Simulated round-trip time per bet.
    delay_ms: AtomicU32,
    /// Extra round-trip time per call number, so bets finish staggered.
    step_ms: AtomicU32,
    /// Peak number of bets the mock saw in flight at once.
    inflight: AtomicU32,
    peak_inflight: AtomicU32,
}

impl Mock {
    fn new(outcomes: &[f64]) -> Arc<Self> {
        Arc::new(Self {
            outcomes: Mutex::new(outcomes.iter().copied().collect()),
            calls: AtomicU32::new(0),
            rotations: AtomicU32::new(0),
            busy_once: AtomicBool::new(false),
            delay_ms: AtomicU32::new(0),
            step_ms: AtomicU32::new(0),
            inflight: AtomicU32::new(0),
            peak_inflight: AtomicU32::new(0),
        })
    }
}

#[async_trait]
impl Casino for Mock {
    fn id(&self) -> CasinoId {
        CasinoId::ChipsGg
    }
    fn set_token(&self, _token: Option<String>) {}
    fn shutdown(&self) {}
    async fn bet_limbo(&self, req: &BetRequest) -> Result<BetReply, CasinoError> {
        if self.busy_once.swap(false, Ordering::SeqCst) {
            return Err(CasinoError::Server("Please wait and try again later".into()));
        }
        let n = self.calls.fetch_add(1, Ordering::SeqCst) + 1;
        let now = self.inflight.fetch_add(1, Ordering::SeqCst) + 1;
        self.peak_inflight.fetch_max(now, Ordering::SeqCst);
        let delay = self.delay_ms.load(Ordering::SeqCst) + self.step_ms.load(Ordering::SeqCst) * (n - 1);
        if delay > 0 {
            tokio::time::sleep(Duration::from_millis(delay as u64)).await;
        }
        self.inflight.fetch_sub(1, Ordering::SeqCst);
        let outcome = self.outcomes.lock().pop_front().unwrap_or(1.0);
        let win = outcome >= req.target;
        Ok(BetReply {
            outcome,
            amount: Some(req.amount),
            winnings: Some(if win { mul_f(req.amount, req.target) } else { 0 }),
            win: Some(win),
            nonce: Some(n as u64),
        })
    }
    async fn rotate_seed(&self, client_seed: &str) -> Result<ProvableState, CasinoError> {
        self.rotations.fetch_add(1, Ordering::SeqCst);
        Ok(ProvableState { client_seed: Some(client_seed.into()), nonce: Some(0), server_seed_hash: None })
    }
    async fn provable_state(&self) -> Result<ProvableState, CasinoError> {
        Ok(ProvableState::default())
    }
    async fn list_slots(&self, _f: crate::casino::SlotFilter) -> Result<Vec<crate::slots::SlotGame>, CasinoError> {
        Ok(vec![])
    }
    async fn launch_slot(&self, _id: &str, _demo: bool, _cur: &str) -> Result<String, CasinoError> {
        Err(CasinoError::NotConnected)
    }
}

fn engine_with(mock: Arc<Mock>) -> (Arc<Engine>, std_mpsc::Receiver<Event>) {
    let (tx, rx) = std_mpsc::channel();
    let e = Engine::new(tx, Arc::new(|| {}));
    *e.casino.lock() = Some(mock);
    (e, rx)
}

fn cfg(base: Units, target: f64) -> AutoConfig {
    AutoConfig {
        currency: "btc".into(),
        target,
        base,
        limit: 0,
        delay_ms: 0,
        on_win: None,
        on_loss: None,
        stop_profit: None,
        stop_loss: None,
        max_bet: None,
        hit_line: None,
    }
}

async fn wait_stopped(e: &Engine) -> AutoState {
    for _ in 0..400 {
        if !e.auto.lock().running {
            return e.auto.lock().clone();
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    panic!("auto bet never stopped");
}

fn bets(rx: &std_mpsc::Receiver<Event>) -> Vec<BetRecord> {
    rx.try_iter().filter_map(|e| if let Event::Bet(b) = e { Some(b) } else { None }).collect()
}

#[tokio::test]
async fn manual_bet_settles_a_win() {
    let (e, rx) = engine_with(Mock::new(&[2.5]));
    let req = BetRequest { currency: "btc".into(), amount: 1000, target: 2.0 };
    let r = e.place_bet(req, Some(2.4), Arc::new(|| true)).await.unwrap();
    assert!(r.win && r.hit);
    assert_eq!(r.payout, 2000);
    assert_eq!(r.profit, 1000);
    assert_eq!(r.nonce, Some(1));
    assert_eq!(bets(&rx).len(), 1);
}

#[tokio::test]
async fn busy_reply_is_retried() {
    let mock = Mock::new(&[1.0]);
    mock.busy_once.store(true, Ordering::SeqCst);
    let (e, rx) = engine_with(mock.clone());
    let req = BetRequest { currency: "btc".into(), amount: 1000, target: 2.0 };
    let r = e.place_bet(req, None, Arc::new(|| true)).await.unwrap();
    assert!(!r.win);
    assert_eq!(r.profit, -1000);
    let retries = rx.try_iter().filter(|e| matches!(e, Event::Retry)).count();
    assert_eq!(retries, 1);
    assert_eq!(mock.calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn martingale_hits_stop_loss() {
    let mock = Mock::new(&[1.0, 1.0, 1.0, 1.0]);
    let (e, rx) = engine_with(mock.clone());
    let mut c = cfg(100, 2.0);
    c.on_loss = Some(100.0);
    c.stop_loss = Some(700);
    e.start_auto(c);
    let a = wait_stopped(&e).await;
    assert_eq!(a.msg, "Stop loss reached");
    assert_eq!(a.done, 3);
    assert_eq!(a.profit, -700);
    let amounts: Vec<_> = bets(&rx).iter().map(|b| b.amount).collect();
    assert_eq!(amounts, vec![100, 200, 400]);
    assert_eq!(a.stake, 800, "stake keeps doubling for the next-bet display");
}

#[tokio::test]
async fn win_resets_to_base_and_stop_profit() {
    let (e, rx) = engine_with(Mock::new(&[1.0, 3.0, 3.0]));
    let mut c = cfg(100, 2.0);
    c.on_loss = Some(100.0);
    c.stop_profit = Some(200);
    e.start_auto(c);
    let a = wait_stopped(&e).await;
    assert_eq!(a.msg, "Stop profit reached");
    let amounts: Vec<_> = bets(&rx).iter().map(|b| b.amount).collect();
    assert_eq!(amounts, vec![100, 200, 100]);
    assert_eq!(a.profit, -100 + 200 + 100);
}

#[tokio::test]
async fn hit_stops_the_run() {
    let (e, _rx) = engine_with(Mock::new(&[1.5, 12.0, 1.0]));
    let mut c = cfg(100, 2.0);
    c.hit_line = Some(10.0);
    e.start_auto(c);
    let a = wait_stopped(&e).await;
    assert_eq!(a.msg, "HIT 12.00×");
    assert_eq!(a.done, 2);
    assert_eq!(a.hits, 1);
    assert_eq!(a.last_hit, Some(12.0));
}

#[tokio::test]
async fn bet_limit_and_max_bet() {
    let (e, _rx) = engine_with(Mock::new(&[1.0, 1.0, 1.0, 1.0, 1.0]));
    let mut c = cfg(100, 2.0);
    c.limit = 3;
    e.start_auto(c);
    assert_eq!(wait_stopped(&e).await.msg, "Done");

    let (e, _rx) = engine_with(Mock::new(&[1.0, 1.0, 1.0]));
    let mut c = cfg(100, 2.0);
    c.on_loss = Some(100.0);
    c.max_bet = Some(250);
    e.start_auto(c);
    let a = wait_stopped(&e).await;
    assert_eq!(a.msg, "Next bet over max bet");
    assert_eq!(a.done, 2);
}

#[tokio::test]
async fn seed_rotates_every_n_bets() {
    let mock = Mock::new(&[1.0; 5]);
    let (e, rx) = engine_with(mock.clone());
    e.seed_every.store(2, Ordering::SeqCst);
    let mut c = cfg(100, 2.0);
    c.limit = 5;
    e.start_auto(c);
    wait_stopped(&e).await;
    assert_eq!(mock.rotations.load(Ordering::SeqCst), 2, "rotated before bet 3 and bet 5");
    let rotated = rx.try_iter().filter(|e| matches!(e, Event::SeedRotated { reason: "auto", .. })).count();
    assert_eq!(rotated, 2);
}

#[tokio::test]
async fn async_mode_pipelines_flat_bets() {
    let mock = Mock::new(&[1.0; 12]);
    mock.delay_ms.store(40, Ordering::SeqCst);
    let (e, rx) = engine_with(mock.clone());
    e.concurrency.store(4, Ordering::SeqCst);
    *e.slots.lock() = Arc::new(tokio::sync::Semaphore::new(4));
    let mut c = cfg(100, 2.0);
    c.limit = 12;
    let t0 = std::time::Instant::now();
    e.start_auto(c);
    let a = wait_stopped(&e).await;
    let took = t0.elapsed();
    assert_eq!(a.msg, "Done");
    assert_eq!(a.done, 12);
    assert_eq!(bets(&rx).len(), 12);
    assert_eq!(mock.peak_inflight.load(Ordering::SeqCst), 4, "four bets in flight at once");
    assert!(took < Duration::from_millis(12 * 40), "12 bets of 40 ms each should overlap, took {took:?}");

    // a progressive stake must stay sequential even with async on
    let mock = Mock::new(&[1.0; 6]);
    mock.delay_ms.store(20, Ordering::SeqCst);
    let (e, _rx) = engine_with(mock.clone());
    e.concurrency.store(4, Ordering::SeqCst);
    *e.slots.lock() = Arc::new(tokio::sync::Semaphore::new(4));
    let mut c = cfg(100, 2.0);
    c.limit = 6;
    c.on_loss = Some(100.0);
    e.start_auto(c);
    wait_stopped(&e).await;
    assert_eq!(mock.peak_inflight.load(Ordering::SeqCst), 1, "martingale stays one at a time");
}

#[tokio::test]
async fn async_mode_rotates_seed_between_batches() {
    let mock = Mock::new(&[1.0; 8]);
    mock.delay_ms.store(20, Ordering::SeqCst);
    let (e, rx) = engine_with(mock.clone());
    e.concurrency.store(3, Ordering::SeqCst);
    *e.slots.lock() = Arc::new(tokio::sync::Semaphore::new(3));
    e.seed_every.store(4, Ordering::SeqCst);
    let mut c = cfg(100, 2.0);
    c.limit = 8;
    e.start_auto(c);
    let a = wait_stopped(&e).await;
    assert_eq!(a.done, 8);
    assert_eq!(mock.rotations.load(Ordering::SeqCst), 1, "one rotation after the first four bets");
    let rotated = rx.try_iter().filter(|e| matches!(e, Event::SeedRotated { .. })).count();
    assert_eq!(rotated, 1);
}

#[tokio::test]
async fn stop_and_pause() {
    let (e, _rx) = engine_with(Mock::new(&[1.0; 50]));
    let mut c = cfg(100, 2.0);
    c.delay_ms = 20;
    e.start_auto(c);
    tokio::time::sleep(Duration::from_millis(30)).await;
    e.toggle_pause();
    assert!(e.auto.lock().paused);
    let done = e.auto.lock().done;
    tokio::time::sleep(Duration::from_millis(80)).await;
    assert_eq!(e.auto.lock().done, done, "no bets while paused");
    e.toggle_pause();
    tokio::time::sleep(Duration::from_millis(60)).await;
    assert!(e.auto.lock().done > done, "bets resume");
    e.stop_auto("Stopped");
    let a = wait_stopped(&e).await;
    assert_eq!(a.msg, "Stopped");
}

#[tokio::test]
async fn stopping_mid_flight_empties_the_queue() {
    // a stopped async run drops its in-flight bets; they must still count out
    let mock = Mock::new(&[1.0; 50]);
    mock.delay_ms.store(40, Ordering::SeqCst);
    mock.step_ms.store(60, Ordering::SeqCst);
    let (e, rx) = engine_with(mock);
    e.concurrency.store(4, Ordering::SeqCst);
    *e.slots.lock() = Arc::new(tokio::sync::Semaphore::new(4));
    e.start_auto(cfg(100, 2.0));
    tokio::time::sleep(Duration::from_millis(20)).await;
    assert!(e.queued.load(Ordering::SeqCst) > 1, "bets in flight before the stop");
    e.stop_auto("Stopped");
    wait_stopped(&e).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(e.queued.load(Ordering::SeqCst), 0);
    let last = rx.try_iter().filter_map(|ev| if let Event::Queue(q) = ev { Some(q) } else { None }).last();
    assert_eq!(last, Some(0), "the UI hears the queue drain");
}
