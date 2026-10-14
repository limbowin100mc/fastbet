//! Session statistics and bet history, fed by engine events.

use std::collections::VecDeque;
use std::time::{Instant, SystemTime};

use crate::engine::BetRecord;
use crate::money::{to_f64, Units};
use crate::originals::Game;

pub const HISTORY_MAX: usize = 500;
const SERIES_MAX: usize = 4000;
/// Top multipliers and streaks kept per list (the pane's cards scroll through them).
const TOP_N: usize = 50;

/// One of the session's best wins, by the multiplier it paid.
#[derive(Clone, Copy, Debug)]
pub struct TopMulti {
    /// What the bet paid out at: Limbo's target, not the roll (a 3,211× roll on a 100× target is 100×).
    pub multi: f64,
    /// What that bet won or lost.
    pub profit: Units,
    /// The bet's number in the session.
    pub n: u64,
}

/// A run of wins, or of losses.
#[derive(Clone, Copy, Debug)]
pub struct Streak {
    pub len: u32,
    /// What its bets won or lost together.
    pub profit: Units,
}

#[derive(Clone, Debug)]
pub enum HistEntry {
    Bet { n: u64, rec: BetRecord },
    Seed { at: SystemTime, client_seed: String, reason: &'static str },
}

#[derive(Debug, Default)]
pub struct Stats {
    pub cur: String,
    pub decimals: u32,
    pub bets: u64,
    pub wins: u64,
    pub losses: u64,
    pub hits: u64,
    pub retries: u64,
    pub wagered: Units,
    pub profit: Units,
    pub peak: Units,
    pub low: Units,
    pub max_dd: Units,
    pub hi_win: Units,
    pub hi_loss: Units,
    pub hi_bet: Units,
    /// >0 wins in a row, <0 losses in a row
    pub streak: i64,
    /// What the current streak won or lost so far.
    streak_profit: Units,
    pub win_streaks: Vec<Streak>,
    pub loss_streaks: Vec<Streak>,
    pub top_multi: Vec<TopMulti>,
    /// Profit after every `step` bets, as coin value, for the chart.
    pub series: Vec<f64>,
    step: usize,
    skip: usize,
    pub times: VecDeque<Instant>,
    pub latency: f64,
    pub started: Option<Instant>,
    pub last_at: Option<Instant>,
}

/// Keeps `list` the best `TOP_N` by `key`, highest first (a tie goes after the older one).
fn push_top<T, K: PartialOrd>(list: &mut Vec<T>, v: T, key: impl Fn(&T) -> K) {
    let k = key(&v);
    let i = list.iter().position(|x| k > key(x)).unwrap_or(list.len());
    if i >= TOP_N {
        return;
    }
    list.insert(i, v);
    list.truncate(TOP_N);
}

impl Stats {
    pub fn fresh(cur: &str, decimals: u32) -> Self {
        Self { cur: cur.to_string(), decimals, series: vec![0.0], step: 1, ..Default::default() }
    }

    pub fn reset(&mut self, cur: &str, decimals: u32) {
        *self = Self::fresh(cur, decimals);
    }

    pub fn settle(&mut self, r: &BetRecord, decimals: u32) {
        if self.cur != r.currency {
            *self = Self::fresh(&r.currency, decimals);
        }
        self.decimals = decimals;
        let now = Instant::now();
        if self.started.is_none() {
            self.started = Some(now);
        }
        self.last_at = Some(now);
        self.bets += 1;
        if r.win {
            self.wins += 1;
            if self.streak < 0 {
                let ended = self.current_streak();
                push_top(&mut self.loss_streaks, ended, |s| s.len);
                self.streak_profit = 0;
            }
            self.streak = if self.streak > 0 { self.streak + 1 } else { 1 };
        } else {
            self.losses += 1;
            if self.streak > 0 {
                let ended = self.current_streak();
                push_top(&mut self.win_streaks, ended, |s| s.len);
                self.streak_profit = 0;
            }
            self.streak = if self.streak < 0 { self.streak - 1 } else { -1 };
        }
        self.streak_profit += r.profit;
        if r.hit {
            self.hits += 1;
        }
        self.wagered += r.amount;
        self.profit += r.profit;
        self.peak = self.peak.max(self.profit);
        self.low = self.low.min(self.profit);
        self.max_dd = self.max_dd.max(self.peak - self.profit);
        self.hi_win = self.hi_win.max(r.profit);
        self.hi_loss = self.hi_loss.max(-r.profit);
        self.hi_bet = self.hi_bet.max(r.amount);
        if r.win && r.target.is_finite() {
            push_top(&mut self.top_multi, TopMulti { multi: r.target, profit: r.profit, n: self.bets }, |m| m.multi);
        }
        self.latency = if self.latency > 0.0 { self.latency * 0.8 + r.ms * 0.2 } else { r.ms };
        self.times.push_back(now);
        self.trim_times(now);
        self.skip += 1;
        if self.skip >= self.step {
            self.skip = 0;
            self.series.push(to_f64(self.profit, decimals));
            if self.series.len() > SERIES_MAX {
                let mut i = 0;
                self.series.retain(|_| {
                    let keep = i % 2 == 0;
                    i += 1;
                    keep
                });
                self.step *= 2;
            }
        }
    }

    pub fn trim_times(&mut self, now: Instant) {
        while let Some(t) = self.times.front() {
            if now.duration_since(*t).as_secs_f64() > 60.0 {
                self.times.pop_front();
            } else {
                break;
            }
        }
    }

    /// Bets per second over the last 3 seconds, and bets in the last minute.
    pub fn speed(&mut self) -> (f64, usize) {
        let now = Instant::now();
        self.trim_times(now);
        let last3 = self.times.iter().rev().take_while(|t| now.duration_since(**t).as_secs_f64() <= 3.0).count();
        (last3 as f64 / 3.0, self.times.len())
    }

    pub fn win_pct(&self) -> Option<f64> {
        (self.bets > 0).then(|| self.wins as f64 / self.bets as f64 * 100.0)
    }

    /// Profit as a percentage of wagered.
    pub fn edge_pct(&self) -> Option<f64> {
        (self.wagered > 0).then(|| (self.profit * 1_000_000 / self.wagered) as f64 / 10_000.0)
    }

    pub fn rtp_pct(&self) -> Option<f64> {
        (self.wagered > 0).then(|| ((self.wagered + self.profit) * 1_000_000 / self.wagered) as f64 / 10_000.0)
    }

    fn current_streak(&self) -> Streak {
        Streak { len: self.streak.unsigned_abs() as u32, profit: self.streak_profit }
    }

    pub fn best_win_streak(&self) -> u32 {
        self.win_streaks_now().first().map_or(0, |s| s.len)
    }

    pub fn worst_loss_streak(&self) -> u32 {
        self.loss_streaks_now().first().map_or(0, |s| s.len)
    }

    /// Win streak list including the current one.
    pub fn win_streaks_now(&self) -> Vec<Streak> {
        let mut v = self.win_streaks.clone();
        if self.streak > 0 {
            push_top(&mut v, self.current_streak(), |s| s.len);
        }
        v
    }

    pub fn loss_streaks_now(&self) -> Vec<Streak> {
        let mut v = self.loss_streaks.clone();
        if self.streak < 0 {
            push_top(&mut v, self.current_streak(), |s| s.len);
        }
        v
    }

    /// Chart points: the recorded series plus the live profit.
    pub fn chart_points(&self) -> Vec<f64> {
        let mut pts = self.series.clone();
        pts.push(to_f64(self.profit, self.decimals));
        pts
    }
}

#[derive(Default)]
pub struct History {
    pub entries: VecDeque<HistEntry>,
    pub version: u64,
    pub bets: u64,
}

impl History {
    pub fn push_bet(&mut self, rec: BetRecord) -> u64 {
        self.bets += 1;
        self.push(HistEntry::Bet { n: self.bets, rec });
        self.bets
    }

    pub fn push_seed(&mut self, at: SystemTime, client_seed: String, reason: &'static str) {
        self.push(HistEntry::Seed { at, client_seed, reason });
    }

    fn push(&mut self, e: HistEntry) {
        self.entries.push_front(e);
        if self.entries.len() > HISTORY_MAX {
            self.entries.pop_back();
        }
        self.version += 1;
    }

    pub fn clear(&mut self) {
        self.entries.clear();
        self.version += 1;
    }

    pub fn recent_bets(&self, n: usize) -> impl Iterator<Item = &BetRecord> {
        self.entries
            .iter()
            .filter_map(|e| match e {
                HistEntry::Bet { rec, .. } => Some(rec),
                _ => None,
            })
            .take(n)
    }

    /// The latest bet on `game` (`None`: Limbo).
    pub fn latest_for(&self, game: Option<Game>) -> Option<(u64, &BetRecord)> {
        self.entries.iter().find_map(|e| match e {
            HistEntry::Bet { n, rec } if rec.game == game => Some((*n, rec)),
            _ => None,
        })
    }

    /// The latest `n` bets on `game` (`None`: Limbo), newest first.
    pub fn recent_for(&self, game: Option<Game>, n: usize) -> impl Iterator<Item = &BetRecord> {
        self.recent_bets(usize::MAX).filter(move |r| r.game == game).take(n)
    }
}

pub fn clock(at: SystemTime) -> String {
    let secs = at.duration_since(SystemTime::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let local = secs as i64 + local_offset_secs();
    let s = local.rem_euclid(86_400);
    format!("{:02}:{:02}:{:02}", s / 3600, (s / 60) % 60, s % 60)
}

/// Local UTC offset in seconds, read once from the OS.
fn local_offset_secs() -> i64 {
    use std::sync::OnceLock;
    static OFF: OnceLock<i64> = OnceLock::new();
    *OFF.get_or_init(|| {
        #[cfg(windows)]
        {
            // TIME_ZONE_INFORMATION.Bias is in minutes, west-positive. Use the registry-free API via libc-less FFI.
            #[repr(C)]
            struct SystemTime16 {
                _f: [u16; 8],
            }
            #[repr(C)]
            struct TimeZoneInformation {
                bias: i32,
                standard_name: [u16; 32],
                standard_date: SystemTime16,
                standard_bias: i32,
                daylight_name: [u16; 32],
                daylight_date: SystemTime16,
                daylight_bias: i32,
            }
            extern "system" {
                fn GetTimeZoneInformation(tz: *mut TimeZoneInformation) -> u32;
            }
            let mut tz = std::mem::MaybeUninit::<TimeZoneInformation>::zeroed();
            let r = unsafe { GetTimeZoneInformation(tz.as_mut_ptr()) };
            let tz = unsafe { tz.assume_init() };
            let extra = match r {
                2 => tz.daylight_bias,
                _ => tz.standard_bias,
            };
            return -((tz.bias + extra) as i64) * 60;
        }
        #[cfg(unix)]
        {
            let now = unsafe { libc::time(std::ptr::null_mut()) };
            let mut tm = std::mem::MaybeUninit::<libc::tm>::zeroed();
            if !unsafe { libc::localtime_r(&now, tm.as_mut_ptr()) }.is_null() {
                return unsafe { tm.assume_init() }.tm_gmtoff as i64;
            }
        }
        #[allow(unreachable_code)]
        0
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bet(target: f64, outcome: f64, win: bool, profit: Units) -> BetRecord {
        BetRecord {
            at: SystemTime::now(),
            nonce: None,
            currency: "btc".into(),
            target,
            outcome,
            win,
            hit: false,
            amount: 100,
            payout: 100 + profit,
            profit,
            ms: 0.0,
            game: None,
            round: None,
        }
    }

    #[test]
    fn streaks_and_multipliers_keep_what_they_won() {
        let mut s = Stats::fresh("btc", 8);
        // W2 (+200), L3 (-300), W1 (+100), then a loss run still going (L1, -100)
        let bets = [(2.0, 3211.0, true, 100), (3.0, 3.5, true, 200), (100.0, 99.0, false, -100), (2.0, 1.0, false, -100), (2.0, 1.5, false, -100), (2.5, 2.5, true, 150), (2.0, 1.1, false, -100)];
        for (target, roll, win, p) in bets {
            s.settle(&bet(target, roll, win, p), 8);
        }
        let wins: Vec<_> = s.win_streaks_now().iter().map(|st| (st.len, st.profit)).collect();
        assert_eq!(wins, vec![(2, 300), (1, 150)]);
        let losses: Vec<_> = s.loss_streaks_now().iter().map(|st| (st.len, st.profit)).collect();
        assert_eq!(losses, vec![(3, -300), (1, -100)], "the run still going counts too");
        assert_eq!((s.best_win_streak(), s.worst_loss_streak()), (2, 3));
        // the multiplier each win paid (not the roll), and losses not at all
        let top: Vec<_> = s.top_multi.iter().map(|m| (m.multi, m.profit, m.n)).collect();
        assert_eq!(top, vec![(3.0, 200, 2), (2.5, 150, 6), (2.0, 100, 1)]);
    }
}
