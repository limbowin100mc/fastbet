//! Code Mode: the user's strategy, in JavaScript. A script runs on its own thread (the JS engine
//! can't move between threads); an auto run asks it for the next bet after every settled one.
//!
//! What a script sees: `betSize` and `target` (it sets them for the next bet), `baseBet`,
//! `profit`, `wagered`, `bets`, `wins`, `losses`, `winStreak`, `lossStreak`, `game`, `unit`,
//! `currency`; `onBet(bet)` is called after every bet; `stop(reason)`, `changeSeed()` and
//! `log(...)` ask the run for things. Amounts are in the unit the panel shows.

use std::sync::mpsc as std_mpsc;

use boa_engine::{js_string, Context, JsValue, Source};
use tokio::sync::oneshot;

/// The run so far, before each call.
#[derive(Clone, Debug, Default)]
pub struct Stats {
    pub profit: f64,
    pub wagered: f64,
    pub bets: u64,
    pub wins: u64,
    pub losses: u64,
    pub win_streak: u32,
    pub loss_streak: u32,
}

/// A settled bet, amounts in the panel's unit.
#[derive(Clone, Debug, Default)]
pub struct Bet {
    pub win: bool,
    pub amount: f64,
    pub payout: f64,
    pub profit: f64,
    /// What it landed on (Limbo's roll, an original's payout multiplier).
    pub multiplier: f64,
    pub target: f64,
}

/// What the script wants next.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Decision {
    pub bet: f64,
    pub target: f64,
    /// `stop()` was called, with this reason (may be empty).
    pub stop: Option<String>,
    pub seed: bool,
    pub logs: Vec<String>,
}

/// What a script starts with.
#[derive(Clone, Debug)]
pub struct Setup {
    pub code: String,
    /// The base bet, in the panel's unit.
    pub bet: f64,
    pub target: f64,
    /// "limbo" or the original's slug.
    pub game: String,
    /// The panel's unit ("USD", "BTC" …).
    pub unit: String,
    pub currency: String,
}

type Reply = oneshot::Sender<Result<Decision, String>>;

/// A running script. Dropping it ends its thread.
pub struct Script {
    jobs: std_mpsc::Sender<(Bet, Stats, Reply)>,
}

/// The functions every script has, written in JS: they leave their requests in `__fb`.
const PRELUDE: &str = r#"
var __fb = { stop: null, seed: false, logs: [] };
function stop(reason) { __fb.stop = reason === undefined ? "" : String(reason); }
function changeSeed() { __fb.seed = true; }
function log() {
  var parts = [];
  for (var i = 0; i < arguments.length; i++) {
    var a = arguments[i];
    parts.push(typeof a === "object" && a !== null ? JSON.stringify(a) : String(a));
  }
  __fb.logs.push(parts.join(" "));
}
var console = { log: log, info: log, warn: log, error: log };
"#;

/// Longest loop a script may run in one call, and deepest recursion.
const LOOP_LIMIT: u64 = 2_000_000;
const RECURSION_LIMIT: usize = 400;

impl Script {
    /// Runs the script's top level on a new thread. Answers with its first bet and target.
    pub async fn start(setup: Setup) -> Result<(Script, Decision), String> {
        let (tx, rx) = std_mpsc::channel::<(Bet, Stats, Reply)>();
        let (first_tx, first_rx) = oneshot::channel();
        std::thread::Builder::new().name("fastbet-script".into()).spawn(move || serve(setup, rx, first_tx)).map_err(|e| e.to_string())?;
        let first = first_rx.await.map_err(|_| "the script stopped".to_string())??;
        Ok((Script { jobs: tx }, first))
    }

    /// Tells the script about a settled bet; answers with what it wants next.
    pub async fn on_bet(&self, bet: Bet, stats: Stats) -> Result<Decision, String> {
        let (tx, rx) = oneshot::channel();
        self.jobs.send((bet, stats, tx)).map_err(|_| "the script stopped".to_string())?;
        rx.await.map_err(|_| "the script stopped".to_string())?
    }

    /// Runs the top level once, on this thread: the Check button.
    pub fn check(setup: Setup) -> Result<Decision, String> {
        Runner::start(&setup).map(|(_, d)| d)
    }
}

/// A script run on the caller's thread (the simulator's).
pub struct Runner {
    ctx: Context,
}

impl Runner {
    /// Runs the top level; answers with the first bet and target.
    pub fn start(setup: &Setup) -> Result<(Runner, Decision), String> {
        let mut ctx = context();
        let first = begin(&mut ctx, setup)?;
        Ok((Runner { ctx }, first))
    }

    pub fn on_bet(&mut self, bet: &Bet, stats: &Stats) -> Result<Decision, String> {
        on_bet(&mut self.ctx, bet, stats)
    }
}

fn context() -> Context {
    let mut ctx = Context::default();
    ctx.runtime_limits_mut().set_loop_iteration_limit(LOOP_LIMIT);
    ctx.runtime_limits_mut().set_recursion_limit(RECURSION_LIMIT);
    ctx
}

fn serve(setup: Setup, jobs: std_mpsc::Receiver<(Bet, Stats, Reply)>, first: Reply) {
    let mut runner = match Runner::start(&setup) {
        Ok((r, d)) => {
            let _ = first.send(Ok(d));
            r
        }
        Err(e) => {
            let _ = first.send(Err(e));
            return;
        }
    };
    while let Ok((bet, stats, reply)) = jobs.recv() {
        let _ = reply.send(runner.on_bet(&bet, &stats));
    }
}

/// A JS error as one or two readable lines.
fn err(e: impl std::fmt::Display) -> String {
    let s = e.to_string();
    let mut lines = s.lines().map(str::trim).filter(|l| !l.is_empty());
    match (lines.next(), lines.next()) {
        (Some(a), Some(b)) => format!("{a} ({})", b.trim_start_matches("at ")),
        (Some(a), None) => a.to_string(),
        _ => "unknown error".into(),
    }
}

fn set(ctx: &mut Context, name: &str, v: impl Into<JsValue>) -> Result<(), String> {
    ctx.global_object().set(js_string!(name), v.into(), false, ctx).map(|_| ()).map_err(err)
}

fn number(ctx: &mut Context, name: &str) -> Result<f64, String> {
    let v = ctx.global_object().get(js_string!(name), ctx).map_err(err)?;
    v.to_number(ctx).map_err(err)
}

fn set_stats(ctx: &mut Context, s: &Stats) -> Result<(), String> {
    set(ctx, "profit", s.profit)?;
    set(ctx, "wagered", s.wagered)?;
    set(ctx, "bets", s.bets as f64)?;
    set(ctx, "wins", s.wins as f64)?;
    set(ctx, "losses", s.losses as f64)?;
    set(ctx, "winStreak", s.win_streak)?;
    set(ctx, "lossStreak", s.loss_streak)?;
    Ok(())
}

/// The prelude, the run's values, then the script's top level.
fn begin(ctx: &mut Context, setup: &Setup) -> Result<Decision, String> {
    ctx.eval(Source::from_bytes(PRELUDE)).map_err(err)?;
    set(ctx, "betSize", setup.bet)?;
    set(ctx, "baseBet", setup.bet)?;
    set(ctx, "target", setup.target)?;
    set(ctx, "game", js_string!(setup.game.as_str()))?;
    set(ctx, "unit", js_string!(setup.unit.as_str()))?;
    set(ctx, "currency", js_string!(setup.currency.as_str()))?;
    set_stats(ctx, &Stats::default())?;
    ctx.eval(Source::from_bytes(setup.code.as_bytes())).map_err(err)?;
    decision(ctx)
}

fn on_bet(ctx: &mut Context, bet: &Bet, stats: &Stats) -> Result<Decision, String> {
    set_stats(ctx, stats)?;
    let f = ctx.global_object().get(js_string!("onBet"), ctx).map_err(err)?;
    if let Some(f) = f.as_callable() {
        let arg = JsValue::from_json(
            &serde_json::json!({
                "win": bet.win,
                "amount": bet.amount,
                "payout": bet.payout,
                "profit": bet.profit,
                "multiplier": bet.multiplier,
                "target": bet.target,
            }),
            ctx,
        )
        .map_err(err)?;
        f.call(&JsValue::undefined(), &[arg], ctx).map_err(err)?;
    }
    decision(ctx)
}

/// Reads what the script set, and clears its requests for the next call.
fn decision(ctx: &mut Context) -> Result<Decision, String> {
    let bet = number(ctx, "betSize")?;
    if !bet.is_finite() || bet < 0.0 {
        return Err(format!("betSize must be a number of 0 or more, not {bet}"));
    }
    let target = number(ctx, "target")?;
    if !target.is_finite() || target <= 0.0 {
        return Err(format!("target must be a number above 0, not {target}"));
    }
    let fb = ctx.global_object().get(js_string!("__fb"), ctx).map_err(err)?;
    let fb = fb.to_json(ctx).map_err(err)?.unwrap_or_default();
    let stop = fb["stop"].as_str().map(str::to_string);
    let seed = fb["seed"].as_bool().unwrap_or(false);
    let logs = fb["logs"].as_array().map(|a| a.iter().map(|l| l.as_str().unwrap_or_default().to_string()).collect()).unwrap_or_default();
    ctx.eval(Source::from_bytes("__fb.stop = null; __fb.seed = false; __fb.logs = [];")).map_err(err)?;
    Ok(Decision { bet, target, stop, seed, logs })
}

// ------------------------------------------------------------------ examples

/// Ready-made scripts for the Examples menu: (name, code).
pub const EXAMPLES: [(&str, &str); 5] = [
    (
        "Martingale",
        r#"// Martingale: double the bet after a loss,
// back to the base bet after a win.

function onBet(bet) {
  if (bet.win) {
    betSize = baseBet;
  } else {
    betSize = betSize * 2;
  }
}
"#,
    ),
    (
        "Profit goal",
        r#"// Martingale that stops once the run is
// ten base bets up.
const goal = baseBet * 10;

function onBet(bet) {
  betSize = bet.win ? baseBet : betSize * 2;

  if (profit >= goal) {
    stop("Goal reached: +" + profit.toFixed(2) + " " + unit);
  }
}
"#,
    ),
    (
        "Paroli",
        r#"// Paroli: double after a win, back to the base bet
// after three wins in a row or a loss.

function onBet(bet) {
  if (bet.win && winStreak < 3) {
    betSize = betSize * 2;
  } else {
    betSize = baseBet;
  }
}
"#,
    ),
    (
        "Target ladder",
        r#"// Limbo: raise the target by 1x after every loss,
// start over from 2x after a win.
target = 2;

function onBet(bet) {
  if (bet.win) {
    log("Hit " + bet.multiplier.toFixed(2) + "x after " + (bet.target - 1) + " tries");
    target = 2;
  } else {
    target = target + 1;
  }
}
"#,
    ),
    (
        "Cold streak",
        r#"// Flat bets; a new seed after 15 losses in a row,
// and a stop if the run loses 50 base bets.

function onBet(bet) {
  if (lossStreak === 15) {
    log("15 losses in a row: new seed");
    changeSeed();
  }
  if (profit <= -50 * baseBet) {
    stop("Lost 50 base bets");
  }
}
"#,
    ),
];

#[cfg(test)]
mod tests {
    use super::*;

    fn setup(code: &str) -> Setup {
        Setup { code: code.into(), bet: 1.0, target: 2.0, game: "limbo".into(), unit: "USD".into(), currency: "btc".into() }
    }

    fn lost(amount: f64) -> Bet {
        Bet { win: false, amount, payout: 0.0, profit: -amount, multiplier: 1.2, target: 2.0 }
    }

    #[tokio::test]
    async fn martingale_doubles_and_resets() {
        let (s, first) = Script::start(setup(EXAMPLES[0].1)).await.unwrap();
        assert_eq!((first.bet, first.target), (1.0, 2.0));
        let d = s.on_bet(lost(1.0), Stats::default()).await.unwrap();
        assert_eq!(d.bet, 2.0);
        let d = s.on_bet(lost(2.0), Stats::default()).await.unwrap();
        assert_eq!(d.bet, 4.0);
        let won = Bet { win: true, amount: 4.0, payout: 8.0, profit: 4.0, multiplier: 3.0, target: 2.0 };
        assert_eq!(s.on_bet(won, Stats::default()).await.unwrap().bet, 1.0);
    }

    #[tokio::test]
    async fn stop_seed_and_logs_come_back_once() {
        let code = "function onBet(b) { if (profit < -2) { changeSeed(); log('down', profit, {x: 1}); stop('enough'); } }";
        let (s, _) = Script::start(setup(code)).await.unwrap();
        let stats = Stats { profit: -3.0, ..Stats::default() };
        let d = s.on_bet(lost(1.0), stats.clone()).await.unwrap();
        assert_eq!((d.stop.as_deref(), d.seed, d.logs.clone()), (Some("enough"), true, vec![r#"down -3 {"x":1}"#.to_string()]));
        let d = s.on_bet(lost(1.0), Stats::default()).await.unwrap();
        assert_eq!((d.stop, d.seed, d.logs.len()), (None, false, 0));
    }

    #[tokio::test]
    async fn errors_read_well() {
        assert!(Script::start(setup("function (")).await.err().unwrap().contains("SyntaxError"));
        let (s, _) = Script::start(setup("function onBet(b) { nope(); }")).await.unwrap();
        let e = s.on_bet(lost(1.0), Stats::default()).await.unwrap_err();
        assert!(e.contains("nope"), "{e}");
        assert!(Script::start(setup("betSize = 'x';")).await.err().unwrap().contains("betSize"));
    }

    #[test]
    fn endless_loops_are_cut_off() {
        let e = Script::check(setup("while (true) {}")).unwrap_err();
        assert!(e.to_lowercase().contains("limit"), "{e}");
        assert!(Script::check(setup("target = 3; betSize = baseBet * 2;")).is_ok_and(|d| d.target == 3.0 && d.bet == 2.0));
    }

    #[test]
    fn every_example_runs() {
        for (name, code) in EXAMPLES {
            assert!(Script::check(setup(code)).is_ok(), "{name}");
        }
    }
}
