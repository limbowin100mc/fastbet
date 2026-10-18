//! Condition Builder: rules that steer an auto run as it goes ("every 3 losses, raise the bet by
//! 50%"). The settings keep what the user typed ([`Rule`]); a run gets them compiled
//! ([`Compiled`]) and checks them in order after every settled bet ([`apply`]).

use serde::{Deserialize, Serialize};

use crate::money::{num, scale_pct, Units};

/// What a rule watches.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum When {
    /// Bets, wins or losses counted, or a streak of them.
    #[default]
    Bets,
    /// The run's profit or loss.
    Profit,
    /// The multiplier a bet landed on.
    Result,
}

/// How a bets rule counts.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Every {
    /// Every n-th bet, win or loss.
    #[default]
    Every,
    /// Every time a streak reaches a multiple of n.
    Streak,
    /// Once per streak, when it reaches n.
    First,
    /// Every bet of a streak longer than n.
    Above,
    /// Every bet of a streak shorter than n.
    Below,
}

impl Every {
    pub const ALL: [Every; 5] = [Every::Every, Every::Streak, Every::First, Every::Above, Every::Below];

    pub fn label(self) -> &'static str {
        match self {
            Every::Every => "Every",
            Every::Streak => "Every streak of",
            Every::First => "First streak of",
            Every::Above => "Streak above",
            Every::Below => "Streak below",
        }
    }
}

/// What a bets rule counts.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Of {
    Bets,
    Wins,
    #[default]
    Losses,
}

impl Of {
    fn word(self, n: u32) -> &'static str {
        match (self, n == 1) {
            (Of::Bets, true) => "bet",
            (Of::Bets, false) => "bets",
            (Of::Wins, true) => "win",
            (Of::Wins, false) => "wins",
            (Of::Losses, true) => "loss",
            (Of::Losses, false) => "losses",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Side {
    #[default]
    Profit,
    Loss,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Cmp {
    #[default]
    Ge,
    Gt,
    Le,
    Lt,
}

impl Cmp {
    pub const ALL: [Cmp; 4] = [Cmp::Ge, Cmp::Gt, Cmp::Le, Cmp::Lt];

    pub fn symbol(self) -> &'static str {
        match self {
            Cmp::Ge => "≥",
            Cmp::Gt => ">",
            Cmp::Le => "≤",
            Cmp::Lt => "<",
        }
    }

    fn test<T: PartialOrd>(self, v: T, line: T) -> bool {
        match self {
            Cmp::Ge => v >= line,
            Cmp::Gt => v > line,
            Cmp::Le => v <= line,
            Cmp::Lt => v < line,
        }
    }
}

/// What a rule does when it fires.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Act {
    #[default]
    BetUp,
    BetDown,
    BetAdd,
    BetSub,
    BetSet,
    BetReset,
    TargetUp,
    TargetDown,
    TargetAdd,
    TargetSub,
    TargetSet,
    TargetReset,
    ResetProfit,
    Seed,
    Stop,
}

/// What an action's number is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Arg {
    None,
    Pct,
    Amount,
    Multi,
}

impl Act {
    pub const ALL: [Act; 15] = [
        Act::BetUp,
        Act::BetDown,
        Act::BetAdd,
        Act::BetSub,
        Act::BetSet,
        Act::BetReset,
        Act::TargetUp,
        Act::TargetDown,
        Act::TargetAdd,
        Act::TargetSub,
        Act::TargetSet,
        Act::TargetReset,
        Act::ResetProfit,
        Act::Seed,
        Act::Stop,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Act::BetUp => "Increase bet by %",
            Act::BetDown => "Decrease bet by %",
            Act::BetAdd => "Add to bet",
            Act::BetSub => "Subtract from bet",
            Act::BetSet => "Set bet",
            Act::BetReset => "Reset bet",
            Act::TargetUp => "Increase target by %",
            Act::TargetDown => "Decrease target by %",
            Act::TargetAdd => "Add to target",
            Act::TargetSub => "Subtract from target",
            Act::TargetSet => "Set target",
            Act::TargetReset => "Reset target",
            Act::ResetProfit => "Reset profit",
            Act::Seed => "Change seed",
            Act::Stop => "Stop the run",
        }
    }

    pub fn arg(self) -> Arg {
        match self {
            Act::BetUp | Act::BetDown | Act::TargetUp | Act::TargetDown => Arg::Pct,
            Act::BetAdd | Act::BetSub | Act::BetSet => Arg::Amount,
            Act::TargetAdd | Act::TargetSub | Act::TargetSet => Arg::Multi,
            _ => Arg::None,
        }
    }

    /// Changes Limbo's target: only Limbo has one.
    pub fn on_target(self) -> bool {
        matches!(self, Act::TargetUp | Act::TargetDown | Act::TargetAdd | Act::TargetSub | Act::TargetSet | Act::TargetReset)
    }
}

/// One rule as the user set it up. Every field is kept whatever the rule watches, so
/// switching between kinds doesn't lose what was typed.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Rule {
    pub on: bool,
    pub when: When,
    pub every: Every,
    pub n: String,
    pub of: Of,
    pub side: Side,
    pub cmp: Cmp,
    /// Profit rules: an amount in the panel's unit. Result rules: a multiplier.
    pub value: String,
    pub then: Act,
    /// The action's number: a percentage, an amount or a multiplier.
    pub by: String,
}

impl Default for Rule {
    fn default() -> Self {
        Self { on: true, when: When::Bets, every: Every::Every, n: "1".into(), of: Of::Losses, side: Side::Profit, cmp: Cmp::Ge, value: String::new(), then: Act::BetUp, by: "100".into() }
    }
}

impl Rule {
    fn bets(every: Every, n: u32, of: Of, then: Act, by: &str) -> Self {
        Self { every, n: n.to_string(), of, then, by: by.into(), ..Self::default() }
    }

    fn profit(side: Side, amount: &str, then: Act, by: &str) -> Self {
        Self { when: When::Profit, side, value: amount.into(), then, by: by.into(), ..Self::default() }
    }

    /// "Every 3 losses", "First streak of 5 wins", "Profit ≥ 5 USD", "Result ≥ 10×".
    pub fn when_text(&self, unit: &str) -> String {
        match self.when {
            When::Bets => {
                let n: u32 = self.n.trim().parse().unwrap_or(0);
                let of = if self.every == Every::Every { self.of } else { self.streak_of() };
                match (self.every, n) {
                    (Every::Every, 1) => format!("Every {}", of.word(1)),
                    (Every::Every, _) => format!("Every {} {}", show(&self.n), of.word(n)),
                    (e, _) => format!("{} {} {}", e.label(), show(&self.n), of.word(n)),
                }
            }
            When::Profit => {
                let side = if self.side == Side::Profit { "Profit" } else { "Loss" };
                format!("{side} {} {} {unit}", self.cmp.symbol(), show(&self.value))
            }
            When::Result => format!("Result {} {}×", self.cmp.symbol(), show(&self.value)),
        }
    }

    /// "Increase bet by 100%", "Add 0.1 USD to bet", "Set target to 2×", "Stop the run".
    pub fn then_text(&self, unit: &str) -> String {
        let by = show(&self.by);
        match self.then {
            Act::BetUp => format!("Increase bet by {by}%"),
            Act::BetDown => format!("Decrease bet by {by}%"),
            Act::BetAdd => format!("Add {by} {unit} to bet"),
            Act::BetSub => format!("Subtract {by} {unit} from bet"),
            Act::BetSet => format!("Set bet to {by} {unit}"),
            Act::TargetUp => format!("Increase target by {by}%"),
            Act::TargetDown => format!("Decrease target by {by}%"),
            Act::TargetAdd => format!("Add {by}× to target"),
            Act::TargetSub => format!("Subtract {by}× from target"),
            Act::TargetSet => format!("Set target to {by}×"),
            a => a.label().to_string(),
        }
    }

    /// Streaks are of wins or of losses; a streak of bets reads as losses.
    pub fn streak_of(&self) -> Of {
        if self.of == Of::Bets {
            Of::Losses
        } else {
            self.of
        }
    }
}

fn show(s: &str) -> String {
    let t = s.trim();
    if t.is_empty() {
        "…".into()
    } else {
        t.to_string()
    }
}

// ------------------------------------------------------------------ compiled

#[derive(Clone, Debug, PartialEq)]
pub enum Trigger {
    Count { n: u64, of: Of },
    Streak { kind: Every, n: u32, win: bool },
    Profit { loss: bool, cmp: Cmp, amount: Units },
    Result { cmp: Cmp, multi: f64 },
}

#[derive(Clone, Debug, PartialEq)]
pub enum Action {
    BetPct(f64),
    BetAdd(Units),
    BetSet(Units),
    BetReset,
    TargetPct(f64),
    TargetAdd(f64),
    TargetSet(f64),
    TargetReset,
    ResetProfit,
    Seed,
    Stop,
}

impl Action {
    /// Changes the next bet, so the run can't send it before this one settles.
    pub fn progressive(&self) -> bool {
        !matches!(self, Action::ResetProfit | Action::Seed | Action::Stop)
    }
}

/// A rule ready to run. `index` is its place in the user's list, for the fired counts.
#[derive(Clone, Debug, PartialEq)]
pub struct Compiled {
    pub index: usize,
    pub trigger: Trigger,
    pub action: Action,
}

/// The rules switched on, checked and converted. `amount` turns a typed amount into the
/// currency's units. Target rules are left out where there is no target (`limbo` false).
pub fn compile(rules: &[Rule], amount: impl Fn(&str) -> Option<Units>, limbo: bool) -> Result<Vec<Compiled>, String> {
    let mut out = vec![];
    for (i, r) in rules.iter().enumerate() {
        if !r.on || (r.then.on_target() && !limbo) {
            continue;
        }
        let k = i + 1;
        let trigger = match r.when {
            When::Bets => {
                let n: u32 = r.n.trim().parse().ok().filter(|n| *n >= 1).ok_or_else(|| format!("Condition {k}: the count must be 1 or more"))?;
                match r.every {
                    Every::Every => Trigger::Count { n: n as u64, of: r.of },
                    kind => Trigger::Streak { kind, n, win: r.streak_of() == Of::Wins },
                }
            }
            When::Profit => {
                let amount = amount(&r.value).filter(|a| *a >= 0).ok_or_else(|| format!("Condition {k}: enter the {} amount", if r.side == Side::Profit { "profit" } else { "loss" }))?;
                Trigger::Profit { loss: r.side == Side::Loss, cmp: r.cmp, amount }
            }
            When::Result => {
                let multi = num(&r.value).filter(|m| *m >= 0.0).ok_or_else(|| format!("Condition {k}: enter the multiplier"))?;
                Trigger::Result { cmp: r.cmp, multi }
            }
        };
        let pct = || num(&r.by).filter(|p| *p >= 0.0).ok_or_else(|| format!("Condition {k}: enter the percentage"));
        let units = || amount(&r.by).filter(|a| *a >= 0).ok_or_else(|| format!("Condition {k}: enter the amount"));
        let multi = || num(&r.by).filter(|m| *m >= 0.0).ok_or_else(|| format!("Condition {k}: enter the multiplier"));
        let action = match r.then {
            Act::BetUp => Action::BetPct(pct()?),
            Act::BetDown => Action::BetPct(-pct()?.min(100.0)),
            Act::BetAdd => Action::BetAdd(units()?),
            Act::BetSub => Action::BetAdd(-units()?),
            Act::BetSet => Action::BetSet(units()?),
            Act::BetReset => Action::BetReset,
            Act::TargetUp => Action::TargetPct(pct()?),
            Act::TargetDown => Action::TargetPct(-pct()?.min(100.0)),
            Act::TargetAdd => Action::TargetAdd(multi()?),
            Act::TargetSub => Action::TargetAdd(-multi()?),
            Act::TargetSet => Action::TargetSet(multi()?),
            Act::TargetReset => Action::TargetReset,
            Act::ResetProfit => Action::ResetProfit,
            Act::Seed => Action::Seed,
            Act::Stop => Action::Stop,
        };
        out.push(Compiled { index: i, trigger, action });
    }
    Ok(out)
}

/// What the rules count during a pass of a run.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Counters {
    pub bets: u64,
    pub wins: u64,
    pub losses: u64,
    pub win_streak: u32,
    pub loss_streak: u32,
    /// Profit since the run started, or since a rule reset it.
    pub profit: Units,
}

impl Counters {
    pub fn record(&mut self, win: bool, profit: Units) {
        self.bets += 1;
        self.profit += profit;
        if win {
            self.wins += 1;
            self.win_streak += 1;
            self.loss_streak = 0;
        } else {
            self.losses += 1;
            self.loss_streak += 1;
            self.win_streak = 0;
        }
    }
}

/// The run's starting point, and the casino's target range.
#[derive(Clone, Copy, Debug)]
pub struct Limits {
    pub base: Units,
    pub base_target: f64,
    pub min_target: f64,
    pub max_target: f64,
}

/// What fired rules ask of the engine, beyond the bet and target.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Effects {
    pub seed: bool,
    /// The first rule that stopped the run (its place in the user's list).
    pub stop: Option<usize>,
}

fn fires(t: &Trigger, c: &Counters, win: bool, multi: f64) -> bool {
    match *t {
        Trigger::Count { n, of } => match of {
            Of::Bets => c.bets % n == 0,
            Of::Wins => win && c.wins % n == 0,
            Of::Losses => !win && c.losses % n == 0,
        },
        Trigger::Streak { kind, n, win: of_wins } => {
            // the bet has to be part of that streak
            if win != of_wins {
                return false;
            }
            let s = if of_wins { c.win_streak } else { c.loss_streak };
            match kind {
                Every::Streak => s % n == 0,
                Every::First => s == n,
                Every::Above => s > n,
                Every::Below => s < n,
                Every::Every => false,
            }
        }
        Trigger::Profit { loss, cmp, amount } => cmp.test(if loss { -c.profit } else { c.profit }, amount),
        Trigger::Result { cmp, multi: line } => cmp.test(multi, line),
    }
}

/// Checks the rules, in order, after a settled bet already counted in `c`, and applies the ones
/// that fire to the next bet and target. `fired` counts firings per rule of the user's list.
/// A decrease never takes the bet below the base bet; targets stay in the casino's range.
pub fn apply(rules: &[Compiled], c: &mut Counters, win: bool, multi: f64, bet: &mut Units, target: &mut f64, lim: &Limits, fired: &mut [u32]) -> Effects {
    let mut fx = Effects::default();
    let clamp_target = |t: f64| ((t * 100.0).round() / 100.0).clamp(lim.min_target, lim.max_target);
    for r in rules {
        if !fires(&r.trigger, c, win, multi) {
            continue;
        }
        if let Some(f) = fired.get_mut(r.index) {
            *f += 1;
        }
        let floor = if *bet >= lim.base { lim.base } else { 0 };
        match r.action {
            Action::BetPct(p) => {
                let v = scale_pct(*bet, p);
                *bet = if p < 0.0 { v.max(floor) } else { v };
            }
            Action::BetAdd(a) => *bet = if a < 0 { (*bet + a).max(floor) } else { *bet + a },
            Action::BetSet(a) => *bet = a,
            Action::BetReset => *bet = lim.base,
            Action::TargetPct(p) => *target = clamp_target(*target * (1.0 + p / 100.0)),
            Action::TargetAdd(x) => *target = clamp_target(*target + x),
            Action::TargetSet(x) => *target = clamp_target(x),
            Action::TargetReset => *target = lim.base_target,
            Action::ResetProfit => c.profit = 0,
            Action::Seed => fx.seed = true,
            Action::Stop => {
                fx.stop.get_or_insert(r.index);
            }
        }
    }
    fx
}

// ------------------------------------------------------------------ presets

/// A ready-made set of rules.
pub struct Preset {
    pub name: &'static str,
    pub about: &'static str,
    pub limbo_only: bool,
    pub rules: Vec<Rule>,
}

/// The presets, with amounts made from the bet size typed in the panel (`base`).
pub fn presets(base: &str) -> Vec<Preset> {
    let b = num(base).filter(|v| *v > 0.0).unwrap_or(1.0);
    let fmt = |v: f64| {
        let s = format!("{v:.8}");
        s.trim_end_matches('0').trim_end_matches('.').to_string()
    };
    let unit = fmt(b);
    let goal = fmt(b * 10.0);
    vec![
        Preset {
            name: "Martingale",
            about: "Doubles the bet after every loss and goes back to the base bet on a win.",
            limbo_only: false,
            rules: vec![Rule::bets(Every::Every, 1, Of::Losses, Act::BetUp, "100"), Rule::bets(Every::Every, 1, Of::Wins, Act::BetReset, "")],
        },
        Preset {
            name: "Paroli",
            about: "Doubles the bet after a win; back to the base bet after three wins in a row or a loss.",
            limbo_only: false,
            rules: vec![
                Rule::bets(Every::Every, 1, Of::Wins, Act::BetUp, "100"),
                Rule::bets(Every::Streak, 3, Of::Wins, Act::BetReset, ""),
                Rule::bets(Every::Every, 1, Of::Losses, Act::BetReset, ""),
            ],
        },
        Preset {
            name: "D'Alembert",
            about: "One base bet more after a loss, one less after a win.",
            limbo_only: false,
            rules: vec![Rule::bets(Every::Every, 1, Of::Losses, Act::BetAdd, &unit), Rule::bets(Every::Every, 1, Of::Wins, Act::BetSub, &unit)],
        },
        Preset {
            name: "Profit cycles",
            about: "Martingale that banks every 10 base bets of profit: the bet and the profit count start over.",
            limbo_only: false,
            rules: vec![
                Rule::bets(Every::Every, 1, Of::Losses, Act::BetUp, "100"),
                Rule::bets(Every::Every, 1, Of::Wins, Act::BetReset, ""),
                Rule::profit(Side::Profit, &goal, Act::BetReset, ""),
                Rule::profit(Side::Profit, &goal, Act::ResetProfit, ""),
            ],
        },
        Preset {
            name: "Target ladder",
            about: "Raises the target by 1× after every loss and resets it on a win.",
            limbo_only: true,
            rules: vec![Rule::bets(Every::Every, 1, Of::Losses, Act::TargetAdd, "1"), Rule::bets(Every::Every, 1, Of::Wins, Act::TargetReset, "")],
        },
        Preset {
            name: "Fresh seed on a cold streak",
            about: "Changes the seed after 10 losses in a row.",
            limbo_only: false,
            rules: vec![Rule::bets(Every::First, 10, Of::Losses, Act::Seed, "")],
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn units(s: &str) -> Option<Units> {
        crate::money::parse_units(s, 2)
    }

    fn lim() -> Limits {
        Limits { base: 100, base_target: 2.0, min_target: 1.01, max_target: 1_000_000.0 }
    }

    /// Plays `results` (true = win) through the rules; returns the bet before each one.
    fn play(rules: &[Rule], results: &[bool]) -> Vec<Units> {
        let compiled = compile(rules, units, true).unwrap();
        let mut c = Counters::default();
        let (mut bet, mut target) = (100, 2.0);
        let mut fired = vec![0; rules.len()];
        let mut bets = vec![];
        for &w in results {
            bets.push(bet);
            let profit = if w { bet } else { -bet };
            c.record(w, profit);
            apply(&compiled, &mut c, w, if w { 2.0 } else { 1.2 }, &mut bet, &mut target, &lim(), &mut fired);
        }
        bets
    }

    #[test]
    fn martingale() {
        let rules = &presets("1")[0].rules;
        assert_eq!(play(rules, &[false, false, false, true, false]), vec![100, 200, 400, 800, 100]);
    }

    #[test]
    fn paroli_resets_after_three_wins() {
        let rules = &presets("1")[1].rules;
        assert_eq!(play(rules, &[true, true, true, true, false, true]), vec![100, 200, 400, 100, 200, 100]);
    }

    #[test]
    fn dalembert_never_below_base() {
        let rules = &presets("1")[2].rules;
        assert_eq!(play(rules, &[true, false, false, true, true, true]), vec![100, 100, 200, 300, 200, 100]);
    }

    #[test]
    fn counts_and_streaks() {
        let every_2nd_loss = Rule::bets(Every::Every, 2, Of::Losses, Act::BetUp, "100");
        assert_eq!(play(&[every_2nd_loss], &[false, true, false, false, false]), vec![100, 100, 100, 200, 200]);
        let first_streak = Rule::bets(Every::First, 2, Of::Losses, Act::BetSet, "5");
        assert_eq!(play(&[first_streak], &[false, false, false, true, false, false]), vec![100, 100, 500, 500, 500, 500]);
        let above = Rule::bets(Every::Above, 1, Of::Wins, Act::BetAdd, "1");
        assert_eq!(play(&[above], &[true, true, true, false, true]), vec![100, 100, 200, 300, 300]);
    }

    #[test]
    fn profit_cycle_resets_its_count() {
        let rules = &presets("1")[3].rules;
        let compiled = compile(rules, units, true).unwrap();
        let mut c = Counters::default();
        let (mut bet, mut target) = (100, 2.0);
        let mut fired = vec![0; rules.len()];
        for _ in 0..10 {
            c.record(true, 100);
            apply(&compiled, &mut c, true, 2.0, &mut bet, &mut target, &lim(), &mut fired);
        }
        // ten wins of 1.00 reach the 10.00 goal once: profit count back to 0
        assert_eq!(c.profit, 0);
        assert_eq!(fired, vec![0, 10, 1, 1]);
    }

    #[test]
    fn targets_stay_in_range_and_need_limbo() {
        let ladder = &presets("1")[4].rules;
        assert!(compile(ladder, units, false).unwrap().is_empty());
        let compiled = compile(ladder, units, true).unwrap();
        let mut c = Counters::default();
        let (mut bet, mut target) = (100, 2.0);
        let mut fired = vec![0; 2];
        for _ in 0..3 {
            c.record(false, -100);
            apply(&compiled, &mut c, false, 1.0, &mut bet, &mut target, &lim(), &mut fired);
        }
        assert_eq!(target, 5.0);
        c.record(true, 400);
        apply(&compiled, &mut c, true, 5.0, &mut bet, &mut target, &lim(), &mut fired);
        assert_eq!(target, 2.0);
        let low = Rule::bets(Every::Every, 1, Of::Bets, Act::TargetSet, "0.5");
        let compiled = compile(&[low], units, true).unwrap();
        apply(&compiled, &mut c, true, 1.0, &mut bet, &mut target, &lim(), &mut fired);
        assert_eq!(target, 1.01);
    }

    #[test]
    fn stop_seed_and_errors() {
        let rules = vec![Rule { when: When::Result, cmp: Cmp::Ge, value: "10".into(), then: Act::Stop, ..Rule::default() }, Rule::bets(Every::Every, 5, Of::Bets, Act::Seed, "")];
        let compiled = compile(&rules, units, true).unwrap();
        let mut c = Counters::default();
        let (mut bet, mut target) = (100, 2.0);
        let mut fired = vec![0; 2];
        c.record(true, 100);
        let fx = apply(&compiled, &mut c, true, 12.0, &mut bet, &mut target, &lim(), &mut fired);
        assert_eq!(fx, Effects { seed: false, stop: Some(0) });
        assert!(!compiled[0].action.progressive());
        let bad = Rule { n: "0".into(), ..Rule::default() };
        assert_eq!(compile(&[bad], units, true).unwrap_err(), "Condition 1: the count must be 1 or more");
        let off = Rule { on: false, n: "x".into(), ..Rule::default() };
        assert!(compile(&[off], units, true).unwrap().is_empty());
    }

    #[test]
    fn sentences() {
        let r = Rule::bets(Every::Every, 3, Of::Losses, Act::BetUp, "50");
        assert_eq!((r.when_text("USD"), r.then_text("USD")), ("Every 3 losses".into(), "Increase bet by 50%".into()));
        let r = Rule::bets(Every::First, 1, Of::Wins, Act::BetAdd, "0.1");
        assert_eq!((r.when_text("USD"), r.then_text("USD")), ("First streak of 1 win".into(), "Add 0.1 USD to bet".into()));
        let r = Rule::profit(Side::Loss, "5", Act::Stop, "");
        assert_eq!(r.when_text("USD"), "Loss ≥ 5 USD");
    }
}
