//! Casino originals besides Limbo (Stake's and Shuffle's in-house games): which
//! games there are, what a round looks like once the casino has answered, and
//! the math the boards show (payout tables, next multipliers). The casino module
//! turns a [`Game`] plus its params into requests; the UI draws each game from
//! [`Round::state`], which is Stake's JSON for that game (the other casinos'
//! replies are turned into the same shape, Blitz being Shuffle's own).

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::money::{mul_f, Units};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Game {
    Dice,
    Keno,
    Mines,
    Plinko,
    Wheel,
    Diamonds,
    DragonTower,
    Pump,
    Hilo,
    Flip,
    Blackjack,
    VideoPoker,
    Baccarat,
    Roulette,
    Cases,
    Darts,
    Tarot,
    Packs,
    Bars,
    Drill,
    Primedice,
    Chicken,
    Snakes,
    Moles,
    RockPaperScissors,
    ScarabSpin,
    TomeOfLife,
    BlueSamurai,
    /// Shuffle only.
    Blitz,
}

/// How a round left open is found again.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Resume {
    /// `active-bet/<key>`.
    ActiveBet,
    /// `<slug>/<path>`, answered with the open bet or null.
    GetState(&'static str),
    /// Only through GraphQL (`user.activeCasinoBet`).
    Graphql,
}

/// Everything the app knows about a game, in one row.
struct Meta {
    game: Game,
    /// The casino's slug (`/casino/games/<slug>` on Stake).
    slug: &'static str,
    name: &'static str,
    /// The game's key in replies and in `active-bet/<key>`.
    key: &'static str,
    /// Request that starts a round; the auto version when that differs.
    bet: &'static str,
    auto_bet: Option<&'static str>,
    /// Requests that carry an open round on, and cash it out.
    next: Option<&'static str>,
    cashout: Option<&'static str>,
    /// Auto betting works: one request settles a round when the picks come up front, or the
    /// engine plays the round out by a plan (HiLo, see [`auto_step`]).
    auto: bool,
    resume: Resume,
}

const fn meta(game: Game, slug: &'static str, name: &'static str, key: &'static str, bet: &'static str) -> Meta {
    Meta { game, slug, name, key, bet, auto_bet: None, next: None, cashout: None, auto: true, resume: Resume::ActiveBet }
}

const fn steps(m: Meta, next: &'static str, cashout: Option<&'static str>, resume: Resume) -> Meta {
    Meta { next: Some(next), cashout, resume, ..m }
}

const fn manual(m: Meta) -> Meta {
    Meta { auto: false, ..m }
}

/// Lobby order.
const GAMES: &[Meta] = &[
    meta(Game::Dice, "dice", "Dice", "dice", "dice/roll"),
    steps(meta(Game::Mines, "mines", "Mines", "mines", "mines/bet"), "mines/next", Some("mines/cashout"), Resume::ActiveBet),
    meta(Game::Plinko, "plinko", "Plinko", "plinko", "plinko/bet"),
    meta(Game::Keno, "keno", "Keno", "keno", "keno/bet"),
    steps(meta(Game::Hilo, "hilo", "HiLo", "hilo", "hilo/bet"), "hilo/next", Some("hilo/cashout"), Resume::ActiveBet),
    manual(steps(meta(Game::Blackjack, "blackjack", "Blackjack", "blackjack", "blackjack/bet"), "blackjack/next", None, Resume::ActiveBet)),
    meta(Game::Roulette, "roulette", "Roulette", "roulette", "roulette/bet"),
    meta(Game::Baccarat, "baccarat", "Baccarat", "baccarat", "baccarat/bet"),
    manual(steps(meta(Game::VideoPoker, "video-poker", "Video Poker", "videoPoker", "video-poker/bet"), "video-poker/next", None, Resume::ActiveBet)),
    meta(Game::Wheel, "wheel", "Wheel", "wheel", "wheel/spin"),
    steps(meta(Game::DragonTower, "dragon-tower", "Dragon Tower", "dragonTower", "dragon-tower/bet"), "dragon-tower/next", Some("dragon-tower/cashout"), Resume::ActiveBet),
    steps(meta(Game::Pump, "pump", "Pump", "pump", "pump/bet"), "pump/next", Some("pump/cashout"), Resume::ActiveBet),
    steps(meta(Game::Chicken, "chicken", "Chicken", "chicken", "chicken/bet"), "chicken/next", Some("chicken/cashout"), Resume::GetState("getstate")),
    steps(meta(Game::Snakes, "snakes", "Snakes", "snakes", "snakes/bet"), "snakes/next", Some("snakes/cashout"), Resume::GetState("getstate")),
    Meta { auto_bet: Some("moles/autobet"), ..steps(meta(Game::Moles, "moles", "Moles", "moles", "moles/bet"), "moles/next", Some("moles/cashout"), Resume::GetState("getState")) },
    steps(
        meta(Game::RockPaperScissors, "rock-paper-scissors", "Rock Paper Scissors", "rockPaperScissors", "rock-paper-scissors/bet"),
        "rock-paper-scissors/next",
        Some("rock-paper-scissors/cashout"),
        Resume::GetState("getstate"),
    ),
    steps(meta(Game::Flip, "flip", "Flip", "flip", "flip/bet"), "flip/next", Some("flip/cashout"), Resume::Graphql),
    meta(Game::Diamonds, "diamonds", "Diamonds", "diamonds", "diamonds/bet"),
    meta(Game::Cases, "cases", "Cases", "cases", "cases/bet"),
    meta(Game::Darts, "darts", "Darts", "darts", "darts/bet"),
    meta(Game::Drill, "drill", "Drill", "drill", "drill/bet"),
    meta(Game::Bars, "bars", "Bars", "bars", "bars/bet"),
    meta(Game::Tarot, "tarot", "Tarot", "tarot", "tarot/bet"),
    meta(Game::Packs, "packs", "Packs", "packs", "packs/bet"),
    meta(Game::Primedice, "primedice", "Primedice", "primediceX", "primedice-x/bet"),
    meta(Game::ScarabSpin, "slots", "Scarab Spin", "slots", "slots/bet"),
    steps(meta(Game::TomeOfLife, "tome-of-life", "Tome of Life", "slotsTomeOfLife", "slots-tome-of-life/bet"), "slots-tome-of-life/bonus", None, Resume::ActiveBet),
    steps(meta(Game::BlueSamurai, "slots-samurai", "Blue Samurai", "slotsSamurai", "slots-samurai/bet"), "slots-samurai/next", None, Resume::ActiveBet),
    // Shuffle's own; the path only names the action
    meta(Game::Blitz, "blitz", "Blitz", "blitz", "blitz/play"),
];

impl Game {
    /// Every game, in lobby order.
    pub fn all() -> impl Iterator<Item = Game> {
        GAMES.iter().map(|m| m.game)
    }

    /// The games Stake has.
    pub fn stake() -> impl Iterator<Item = Game> {
        Game::all().filter(|g| *g != Game::Blitz)
    }

    fn meta(self) -> &'static Meta {
        GAMES.iter().find(|m| m.game == self).expect("every game has a row")
    }

    pub fn slug(self) -> &'static str {
        self.meta().slug
    }

    pub fn from_slug(slug: &str) -> Option<Game> {
        GAMES.iter().find(|m| m.slug == slug).map(|m| m.game)
    }

    pub fn name(self) -> &'static str {
        self.meta().name
    }

    /// The game's key in replies and in `active-bet/<key>` ("dragonTower").
    pub fn key(self) -> &'static str {
        self.meta().key
    }

    /// Request that starts a round (by hand, or for an auto run).
    pub fn bet_path(self, auto: bool) -> &'static str {
        let m = self.meta();
        if auto {
            m.auto_bet.unwrap_or(m.bet)
        } else {
            m.bet
        }
    }

    /// Request that carries an open round on (`Next`) or cashes it out.
    pub fn step_path(self, step: Step) -> Option<&'static str> {
        match step {
            Step::Next => self.meta().next,
            Step::Cashout => self.meta().cashout,
        }
    }

    pub fn resume(self) -> Resume {
        self.meta().resume
    }

    /// Rounds go on over several requests (reveal a tile, guess, cash out).
    pub fn has_steps(self) -> bool {
        self.meta().next.is_some()
    }

    /// Auto betting works: one request settles a round (tiles, path or guesses picked up front),
    /// or the engine plays it out by a plan (HiLo).
    pub fn auto(self) -> bool {
        self.meta().auto
    }

    /// An open round that needs no decision (bonus spins): the engine plays it to the end.
    pub fn plays_itself(self) -> bool {
        matches!(self, Game::TomeOfLife | Game::BlueSamurai)
    }

    /// The next-step request carries an idempotency key, like the cash out does.
    pub fn next_takes_identifier(self) -> bool {
        matches!(self, Game::Blackjack | Game::VideoPoker | Game::BlueSamurai | Game::TomeOfLife)
    }
}

/// What to do next in an open round.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    Next,
    Cashout,
}

/// How an auto bet plays on a round that stays open after the bet (HiLo).
#[derive(Clone, Debug, PartialEq)]
pub struct Plan {
    /// Take the winnings once the round pays this much.
    pub cashout: f64,
    /// HiLo: what to do on each card, in [`HILO_CODES`] (empty: the likeliest guess).
    pub pattern: Vec<u8>,
}

/// Splits an original's bet params into what the casino is sent and the auto bet's plan
/// (`"plan": {"cashout": 100, "pattern": [2, 2, 2, 5, 5]}`), and draws the HiLo start card where
/// the bet asks for a random one (`"startCard": "random"`), new for every bet.
pub fn prepare(game: Game, mut params: Value) -> (Value, Option<Plan>) {
    let plan = params.as_object_mut().and_then(|o| o.remove("plan")).and_then(|p| {
        let pattern = p["pattern"].as_array().map(|a| a.iter().filter_map(Value::as_u64).map(|c| c as u8).collect()).unwrap_or_default();
        Some(Plan { cashout: p["cashout"].as_f64()?, pattern })
    });
    if game == Game::Hilo && params["startCard"].as_str() == Some("random") {
        let (rank, suit) = random_card();
        params["startCard"] = json!({ "rank": rank, "suit": suit });
    }
    (params, plan)
}

/// What an auto bet does next in its open round: bonus spins play themselves, HiLo plays the
/// plan's pattern card by card until the round pays the plan's target, then takes the winnings.
/// `None` leaves the round open for the board.
pub fn auto_step(round: &Round, plan: Option<&Plan>) -> Option<(Step, Value)> {
    if round.game.plays_itself() {
        return Some((Step::Next, json!({})));
    }
    match (round.game, plan) {
        (Game::Hilo, Some(plan)) => {
            let last = round.state["rounds"].as_array().and_then(|a| a.last());
            if last.and_then(|x| x["payoutMultiplier"].as_f64()).unwrap_or(0.0) >= plan.cashout {
                return Some((Step::Cashout, json!({})));
            }
            Some((Step::Next, json!({ "guess": hilo_pattern_guess(&round.state, &plan.pattern) })))
        }
        _ => None,
    }
}

/// HiLo pattern codes, Antebot's: what each number in a pattern does on a card, as
/// (code, name, what it does).
pub const HILO_CODES: [(u8, &str, &str); 8] = [
    (0, "Low", "Lower or same (on an ace: same)"),
    (1, "High", "Higher or same (on a king: same)"),
    (2, "Equal", "The same rank again, 12.87×. Aces and kings only: other cards are skipped until one comes"),
    (3, "Random", "High, low or equal"),
    (4, "Least likely", "The side with the smaller chance, which pays more"),
    (5, "Likeliest", "The side with the bigger chance, which pays less"),
    (6, "Random", "High or low"),
    (7, "Skip", "Draw another card"),
];

/// Skips a HiLo round allows (chips.gg's limit); past it a skip becomes the likeliest guess.
pub const HILO_SKIPS: usize = 52;

/// A HiLo pattern typed as "2,2,2,5,5" (commas or spaces between the codes).
pub fn parse_hilo_pattern(text: &str) -> Result<Vec<u8>, String> {
    let codes = text
        .split([',', ' ', ';'])
        .map(str::trim)
        .filter(|c| !c.is_empty())
        .map(|c| c.parse::<u8>().ok().filter(|n| *n <= 7).ok_or_else(|| format!("\"{c}\" isn't a pattern code (0 to 7)")))
        .collect::<Result<Vec<u8>, String>>()?;
    if codes.is_empty() {
        return Err("Type a pattern, like 5 or 2,2,2,5,5".into());
    }
    if codes.iter().all(|c| *c == 7) {
        return Err("The pattern only skips: it needs at least one guess".into());
    }
    Ok(codes)
}

/// HiLo: what pattern `code` guesses on a card of `rank`, in the boards' names. "High" is
/// higher or same ("higher" on an ace, "same" on a king), "low" the mirror image; "equal" only
/// exists on an ace or a king, so elsewhere the card is skipped.
pub fn hilo_code_guess(code: u8, rank: &str) -> &'static str {
    use rand::seq::IndexedRandom;
    let high = match rank {
        "A" => "higher",
        "K" => "equal",
        _ => "higherEqual",
    };
    let low = match rank {
        "A" => "equal",
        "K" => "lower",
        _ => "lowerEqual",
    };
    let [(a, pa), (b, pb)] = hilo_guesses(rank);
    match code {
        0 => low,
        1 => high,
        2 if matches!(rank, "A" | "K") => "equal",
        2 => "skip",
        // equal is the low guess on an ace and the high one on a king, so both are high or low
        3 | 6 => [high, low].choose(&mut rand::rng()).unwrap_or(&high),
        4 if pa < pb => a,
        4 if pb < pa => b,
        4 => low,
        7 => "skip",
        _ if pb > pa => b,
        _ => a,
    }
}

/// HiLo: the guess the pattern makes on the round's latest card. The round so far says how far
/// the pattern has got: every card used one code, except the cards skipped while an "equal"
/// waited for an ace or a king. The pattern starts over when it runs out.
pub fn hilo_pattern_guess(state: &Value, pattern: &[u8]) -> &'static str {
    let pattern: &[u8] = if pattern.is_empty() { &[5] } else { pattern };
    let rounds = state["rounds"].as_array().map(Vec::as_slice).unwrap_or(&[]);
    let mut card = state["startCard"]["rank"].as_str().unwrap_or("7");
    let mut used = 0;
    for x in rounds {
        let waited = pattern[used % pattern.len()] == 2 && !matches!(card, "A" | "K");
        if !waited {
            used += 1;
        }
        card = x["card"]["rank"].as_str().unwrap_or("7");
    }
    let skips = rounds.iter().filter(|x| x["guess"].as_str() == Some("skip")).count();
    match hilo_code_guess(pattern[used % pattern.len()], card) {
        "skip" if skips >= HILO_SKIPS => hilo_code_guess(5, card),
        g => g,
    }
}

/// A round as the casino reported it, after any request (bet, next, cash out).
#[derive(Clone, Debug)]
pub struct Round {
    pub game: Game,
    pub id: String,
    /// Still open: more steps or a cash out to come.
    pub active: bool,
    pub currency: String,
    /// Base stake.
    pub amount: Units,
    /// Stake multiplier (a doubled blackjack hand is 2).
    pub amount_mult: f64,
    pub payout: Units,
    /// Payout multiplier so far (0 while nothing is won, or on a loss).
    pub multiplier: f64,
    /// The game's own state JSON.
    pub state: Value,
}

impl Round {
    /// Total staked.
    pub fn stake(&self) -> Units {
        if self.amount_mult == 1.0 {
            self.amount
        } else {
            mul_f(self.amount, self.amount_mult)
        }
    }
}

// ------------------------------------------------------------------ math

/// Product of (n-i)/(n-k-i) for i in 0..picks: the inverse odds of drawing `picks` safe tiles
/// out of `n` with `k` bad ones.
fn inverse_odds(n: u32, bad: u32, picks: u32) -> f64 {
    (0..picks).map(|i| (n - i) as f64 / (n - bad - i) as f64).product()
}

/// Mines: multiplier after `gems` safe tiles with `mines` mines on the 25-tile board (1% edge).
pub fn mines_multiplier(mines: u32, gems: u32) -> f64 {
    if mines == 0 || mines >= 25 || gems + mines > 25 {
        return 0.0;
    }
    0.99 * inverse_odds(25, mines, gems)
}

/// Mines state in Stake's shape, for casinos that report the picked tiles and (once the round
/// is over) where the mines were: what each tile paid, nothing from the first mine on.
pub fn mines_state(count: u32, picks: &[i64], mines: &[i64]) -> Value {
    let (mut gems, mut hit) = (0, false);
    let rounds: Vec<Value> = picks
        .iter()
        .map(|f| {
            hit |= mines.contains(f);
            if !hit {
                gems += 1;
            }
            serde_json::json!({ "field": f, "payoutMultiplier": if hit { 0.0 } else { mines_multiplier(count, gems) } })
        })
        .collect();
    serde_json::json!({ "minesCount": count, "mines": mines, "rounds": rounds })
}

/// Pump: balloons that pop among the 25 steps, per difficulty.
pub fn pump_pops(difficulty: &str) -> u32 {
    match difficulty {
        "medium" => 3,
        "hard" => 5,
        "expert" => 10,
        _ => 1,
    }
}

/// Pump: multiplier after `pumps` safe pumps (2% edge).
pub fn pump_multiplier(difficulty: &str, pumps: u32) -> f64 {
    let pops = pump_pops(difficulty);
    if pumps + pops > 25 {
        return 0.0;
    }
    if pumps == 0 {
        return 1.0;
    }
    0.98 * inverse_odds(25, pops, pumps)
}

/// Dragon Tower: (columns, eggs) per row.
pub fn dragon_layout(difficulty: &str) -> (u32, u32) {
    match difficulty {
        "medium" => (3, 2),
        "hard" => (2, 1),
        "expert" => (3, 1),
        "master" => (4, 1),
        _ => (4, 3),
    }
}

pub const DRAGON_ROWS: u32 = 9;

/// Dragon Tower: multiplier after climbing `rows` rows (2% edge).
pub fn dragon_multiplier(difficulty: &str, rows: u32) -> f64 {
    if rows == 0 {
        return 1.0;
    }
    let (cols, eggs) = dragon_layout(difficulty);
    0.98 * (cols as f64 / eggs as f64).powi(rows as i32)
}

/// Flip: each right guess pays ×1.96 (2% edge).
pub fn flip_multiplier(flips: u32) -> f64 {
    if flips == 0 {
        1.0
    } else {
        0.98 * 2f64.powi(flips as i32)
    }
}

/// Dice: chance to win in percent.
pub fn dice_chance(target: f64, over: bool) -> f64 {
    if over {
        100.0 - target
    } else {
        target
    }
}

/// Dice: payout for a win chance (1% edge).
pub fn dice_multiplier(chance: f64) -> f64 {
    if chance <= 0.0 {
        0.0
    } else {
        99.0 / chance
    }
}

/// Dice target for a win chance.
pub fn dice_target(chance: f64, over: bool) -> f64 {
    if over {
        100.0 - chance
    } else {
        chance
    }
}

/// Blitz: cards drawn per bet.
pub const BLITZ_PICKS: (u32, u32) = (5, 36);

/// Blitz: pays when `picks` cards from an endless 52-card deck are all different (2% edge).
pub fn blitz_multiplier(picks: u32) -> f64 {
    let all_different: f64 = (0..picks.min(52)).map(|i| (52 - i) as f64 / 52.0).product();
    if all_different <= 0.0 {
        0.0
    } else {
        0.98 / all_different
    }
}

/// Blitz card index (0–51) -> (rank, suit) as the cards are drawn: twos first, aces last,
/// suits in diamonds, hearts, spades, clubs order.
pub fn blitz_card(index: u32) -> (&'static str, &'static str) {
    const RANKS: [&str; 13] = ["2", "3", "4", "5", "6", "7", "8", "9", "10", "J", "Q", "K", "A"];
    const SUITS: [&str; 4] = ["D", "H", "S", "C"];
    (RANKS[(index / 4).min(12) as usize], SUITS[(index % 4) as usize])
}


/// HiLo card ranks, low to high, as the casino spells them.
pub const RANKS: [&str; 13] = ["A", "2", "3", "4", "5", "6", "7", "8", "9", "10", "J", "Q", "K"];
pub const SUITS: [&str; 4] = ["C", "D", "H", "S"];

/// A random HiLo card (rank, suit).
pub fn random_card() -> (String, String) {
    use rand::seq::IndexedRandom;
    let mut rng = rand::rng();
    (RANKS.choose(&mut rng).unwrap_or(&"7").to_string(), SUITS.choose(&mut rng).unwrap_or(&"H").to_string())
}

/// 1 (ace) to 13 (king).
pub fn rank_value(rank: &str) -> u32 {
    RANKS.iter().position(|r| *r == rank).map(|i| i as u32 + 1).unwrap_or(7)
}

/// The two guesses HiLo offers on a card, with their chances (0–1): ace and king only
/// allow strictly higher / lower, everything else "or same".
pub fn hilo_guesses(rank: &str) -> [(&'static str, f64); 2] {
    let v = rank_value(rank) as f64;
    match rank {
        "A" => [("higher", 12.0 / 13.0), ("equal", 1.0 / 13.0)],
        "K" => [("lower", 12.0 / 13.0), ("equal", 1.0 / 13.0)],
        _ => [("higherEqual", (14.0 - v) / 13.0), ("lowerEqual", v / 13.0)],
    }
}

/// HiLo payout for a guess with chance `p` (1% edge).
pub fn hilo_multiplier(p: f64) -> f64 {
    if p <= 0.0 {
        0.0
    } else {
        0.99 / p
    }
}

/// The side guesses chips.gg's HiLo offers on a card, with their chances: the next card's
/// colour, and whether it is the same kind of card (a number 2–10, a face J–K, or an ace).
pub fn hilo_side_guesses(rank: &str) -> [(&'static str, f64); 3] {
    let kind = match rank_value(rank) {
        1 => ("ace", 1.0 / 13.0),
        2..=10 => ("number", 9.0 / 13.0),
        _ => ("face", 3.0 / 13.0),
    };
    [("red", 0.5), ("black", 0.5), kind]
}

/// HiLo on chips.gg: what the round pays after one more right guess with chance `p`, from the
/// latest card's multiplier (`None` before the first card). chips.gg takes its 1% edge once per
/// round (0.99 × 1/p × 1/p …), not on every guess like Stake.
pub fn chips_hilo_next(so_far: Option<f64>, p: f64) -> f64 {
    if p <= 0.0 {
        0.0
    } else {
        so_far.unwrap_or(0.99) / p
    }
}

/// Diamonds: the hand a draw of five gem colours makes, and its payout.
pub fn diamonds_hand(colors: &[String]) -> (&'static str, f64) {
    let mut counts: HashMap<&str, u32> = HashMap::new();
    for c in colors {
        *counts.entry(c.as_str()).or_default() += 1;
    }
    let mut v: Vec<u32> = counts.into_values().collect();
    v.sort_unstable_by(|a, b| b.cmp(a));
    match v.as_slice() {
        [5] => ("5 of a kind", 50.0),
        [4, ..] => ("4 of a kind", 5.0),
        [3, 2] => ("Full house", 4.0),
        [3, ..] => ("3 of a kind", 3.0),
        [2, 2, ..] => ("Two pair", 2.0),
        [2, ..] => ("One pair", 0.1),
        _ => ("Nothing", 0.0),
    }
}

/// Diamonds paytable, best hand first.
pub const DIAMONDS_PAYTABLE: &[(&str, f64)] =
    &[("5 of a kind", 50.0), ("4 of a kind", 5.0), ("Full house", 4.0), ("3 of a kind", 3.0), ("Two pair", 2.0), ("One pair", 0.1), ("Nothing", 0.0)];

/// Difficulty levels most of the newer originals share.
pub const LEVELS: [&str; 4] = ["easy", "medium", "hard", "expert"];
pub const LEVEL_NAMES: [&str; 4] = ["Easy", "Medium", "Hard", "Expert"];

/// Cases: what a case can hold per difficulty, (multiplier, chance), from the site.
pub fn cases_table(difficulty: &str) -> &'static [(f64, f64)] {
    match difficulty {
        "medium" => &[(0.3, 0.35), (0.6, 0.35), (1.2, 0.15), (1.4, 0.1), (3.0, 0.03), (6.0, 0.015), (33.0, 0.005)],
        "hard" => &[(0.1, 0.47), (0.3, 0.31), (1.2, 0.12), (2.4, 0.06), (6.0, 0.03), (12.0, 0.008), (75.0, 0.0018), (705.0, 0.0002)],
        "expert" => &[(0.0, 0.5), (0.2, 0.4), (1.5, 0.06), (6.0, 0.025), (9.0, 0.01), (30.0, 0.004), (150.0, 0.0008), (1200.0, 0.00015), (3000.0, 0.00005)],
        _ => &[(0.4, 0.35), (0.6, 0.35), (1.2, 0.15), (1.5, 0.08), (3.0, 0.05), (9.0, 0.02)],
    }
}

/// Darts: the board's rings from the outside in, (multiplier, chance), from the site.
pub fn darts_table(difficulty: &str) -> &'static [(f64, f64)] {
    match difficulty {
        "medium" => &[(0.4, 0.5525), (0.6, 0.2875), (1.3, 0.075), (3.1, 0.041667), (6.0, 0.03333), (16.0, 0.01)],
        "hard" => &[(0.2, 0.5939), (0.5, 0.2756), (2.5, 0.06345), (3.6, 0.0423), (8.8, 0.02115), (63.0, 0.0036)],
        "expert" => &[(0.1, 0.6871), (0.5, 0.2541), (4.8, 0.038933), (9.6, 0.012978), (42.0, 0.006489), (500.0, 0.0004)],
        _ => &[(0.5, 0.476875), (0.8, 0.26), (1.2, 0.12375), (1.5, 0.06875), (2.7, 0.055), (8.5, 0.015625)],
    }
}

/// Chicken: safe lanes out of 20 per difficulty.
pub fn chicken_lanes(difficulty: &str) -> u32 {
    match difficulty {
        "medium" => 17,
        "hard" => 15,
        "expert" => 10,
        _ => 19,
    }
}

/// Chicken: multiplier after crossing `lanes` lanes (2% edge, never below 1).
pub fn chicken_multiplier(difficulty: &str, lanes: u32) -> f64 {
    let safe = chicken_lanes(difficulty);
    if lanes > safe {
        return 0.0;
    }
    let mut m = 0.98;
    for r in 0..lanes {
        m *= (20 - r) as f64 / (safe - r) as f64;
    }
    m.max(1.0)
}

/// Snakes: what each dice total pays per difficulty (0: a snake ends the round).
pub fn snakes_payout(difficulty: &str, total: u32) -> f64 {
    let row: [f64; 11] = match difficulty {
        "medium" => [4.0, 2.5, 1.4, 1.2, 0.0, 0.0, 0.0, 1.2, 1.4, 2.5, 4.0],
        "hard" => [7.5, 3.0, 1.5, 0.0, 0.0, 0.0, 0.0, 0.0, 1.5, 3.0, 7.5],
        "expert" => [10.0, 4.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 4.0, 10.0],
        "master" => [18.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 18.0],
        _ => [2.0, 1.3, 1.2, 1.1, 1.08, 0.0, 1.08, 1.1, 1.2, 1.3, 2.0],
    };
    if (2..=12).contains(&total) {
        row[(total - 2) as usize]
    } else {
        0.0
    }
}

pub const SNAKES_LEVELS: [&str; 5] = ["easy", "medium", "hard", "expert", "master"];

/// Moles: rounds per game for a number of moles (7 holes).
pub fn moles_rounds(moles: u32) -> u32 {
    if moles <= 1 {
        8
    } else {
        9
    }
}

/// Moles: multiplier after `hits` hits with `moles` moles among 7 holes (2% edge).
pub fn moles_multiplier(moles: u32, hits: u32) -> f64 {
    if moles == 0 || hits == 0 {
        return 1.0;
    }
    0.98 * (7.0 / moles as f64).powi(hits as i32)
}

/// Video poker (jacks or better) paytable as the site lists it, best hand first.
pub const VIDEO_POKER_PAYTABLE: &[(&str, &str, f64)] = &[
    ("royalFlush", "Royal flush", 800.0),
    ("straightFlush", "Straight flush", 60.0),
    ("fourOfAKind", "4 of a kind", 22.0),
    ("fullHouse", "Full house", 9.0),
    ("flush", "Flush", 6.0),
    ("straight", "Straight", 4.0),
    ("threeOfAKind", "3 of a kind", 3.0),
    ("twoPair", "Two pair", 2.0),
    ("onePair", "Jacks or better", 1.0),
];

/// Roulette: the red numbers.
pub const ROULETTE_REDS: [u32; 18] = [1, 3, 5, 7, 9, 12, 14, 16, 18, 19, 21, 23, 25, 27, 30, 32, 34, 36];

/// Roulette: does a chip on `key` win when the ball lands on `n`?
pub fn roulette_wins(key: &str, n: u32) -> bool {
    if let Some(num) = key.strip_prefix("number") {
        return num.parse::<u32>().ok() == Some(n);
    }
    if n == 0 {
        return false;
    }
    let red = ROULETTE_REDS.contains(&n);
    match key {
        "colorRed" => red,
        "colorBlack" => !red,
        "parityEven" => n.is_multiple_of(2),
        "parityOdd" => n % 2 == 1,
        "range0118" => n <= 18,
        "range1936" => n >= 19,
        "range0112" => n <= 12,
        "range1324" => (13..=24).contains(&n),
        "range2536" => n >= 25,
        // rows of the table: 3, 6 … 36 is the top row
        "row1" => n % 3 == 1,
        "row2" => n % 3 == 2,
        "row3" => n.is_multiple_of(3),
        _ => false,
    }
}

// ------------------------------------------------------------------ payout tables

/// Payout tables the casino publishes (Stake: `info { kenoPayouts wheelPayouts plinkoPayouts }`).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Tables {
    /// risk -> [picks - 1] -> payout per number of hits
    #[serde(default)]
    pub keno: HashMap<String, Vec<Vec<f64>>>,
    /// risk -> segments -> payout per segment
    #[serde(default)]
    pub wheel: HashMap<String, HashMap<String, Vec<f64>>>,
    /// risk -> rows -> payout per bucket
    #[serde(default)]
    pub plinko: HashMap<String, HashMap<String, Vec<f64>>>,
    /// mines -> payout per number of gems, where the casino publishes its own (chips.gg)
    #[serde(default)]
    pub mines: HashMap<String, Vec<f64>>,
}

impl Tables {
    /// The copy shipped with the app, used until the casino's own arrives.
    pub fn builtin() -> Self {
        serde_json::from_str(include_str!("../../assets/stake-originals.json")).unwrap_or_default()
    }

    pub fn keno(&self, risk: &str, picks: usize) -> &[f64] {
        picks.checked_sub(1).and_then(|i| self.keno.get(risk)?.get(i)).map(Vec::as_slice).unwrap_or(&[])
    }

    pub fn wheel(&self, risk: &str, segments: u32) -> &[f64] {
        self.wheel.get(risk).and_then(|m| m.get(&segments.to_string())).map(Vec::as_slice).unwrap_or(&[])
    }

    pub fn plinko(&self, risk: &str, rows: u32) -> &[f64] {
        self.plinko.get(risk).and_then(|m| m.get(&rows.to_string())).map(Vec::as_slice).unwrap_or(&[])
    }

    /// Mines: what `gems` safe tiles pay with `mines` mines, from the casino's table when it has
    /// one (chips.gg cuts its payouts to cents, a cent lower now and then), else the formula.
    pub fn mines(&self, mines: u32, gems: u32) -> f64 {
        self.mines.get(&mines.to_string()).and_then(|t| t.get(gems as usize)).copied().unwrap_or_else(|| mines_multiplier(mines, gems))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn multipliers_match_the_server() {
        // values the casino returned for these rounds
        assert!((mines_multiplier(3, 1) - 1.125).abs() < 1e-9);
        assert!((pump_multiplier("easy", 1) - 1.0208333333333335).abs() < 1e-9);
        assert!((pump_multiplier("hard", 1) - 1.225).abs() < 1e-9);
        assert!((dragon_multiplier("easy", 1) - 1.31).abs() < 0.005);
        assert!((dice_multiplier(dice_chance(50.5, true)) - 2.0).abs() < 1e-9);
        assert_eq!(diamonds_hand(&["yellow", "purple", "green", "purple", "orange"].map(String::from)), ("One pair", 0.1));
        assert_eq!(diamonds_hand(&["red"; 5].map(String::from)).1, 50.0);
        assert_eq!(diamonds_hand(&["a", "a", "a", "b", "b"].map(String::from)).0, "Full house");
    }

    #[test]
    fn games_round_trip() {
        for g in Game::all() {
            assert_eq!(Game::from_slug(g.slug()), Some(g));
        }
        assert_eq!(Game::stake().count(), 28);
        assert_eq!(Game::Moles.bet_path(true), "moles/autobet");
        assert_eq!(Game::Moles.bet_path(false), "moles/bet");
        assert_eq!(Game::Blackjack.step_path(Step::Cashout), None);
        assert!(Game::VideoPoker.has_steps() && !Game::VideoPoker.auto());
        assert_eq!(hilo_guesses("A")[0].0, "higher");
        assert_eq!(hilo_guesses("7")[1], ("lowerEqual", 7.0 / 13.0));
    }

    #[test]
    fn newer_games_match_the_server() {
        // the casino returned 1.0315789… after one lane on easy
        assert!((chicken_multiplier("easy", 1) - 1.031578947368421).abs() < 1e-9);
        assert_eq!(snakes_payout("easy", 2), 2.0);
        assert_eq!(snakes_payout("easy", 7), 0.0);
        assert!(cases_table("easy").iter().any(|(m, _)| *m == 0.4));
        assert!(darts_table("easy").iter().any(|(m, _)| *m == 0.8));
        for d in LEVELS {
            let sum: f64 = cases_table(d).iter().map(|(_, c)| c).sum();
            assert!((sum - 1.0).abs() < 0.01, "cases {d} chances add up to {sum}");
        }
        assert!(roulette_wins("colorRed", 1) && !roulette_wins("colorRed", 2) && !roulette_wins("colorBlack", 0));
        assert!(roulette_wins("row3", 36) && roulette_wins("range1324", 13) && roulette_wins("number0", 0));
    }

    #[test]
    fn blitz_matches_the_site() {
        // the site's paytable, rounded to 2 decimals
        for (picks, shown) in [(5, 1.19), (10, 2.47), (14, 6.72), (20, 66.81), (25, 1051.10)] {
            assert!((blitz_multiplier(picks) - shown).abs() < 0.006, "{picks} picks: {}", blitz_multiplier(picks));
        }
        assert_eq!(blitz_card(0), ("2", "D"));
        assert_eq!(blitz_card(51), ("A", "C"));
        assert_eq!(blitz_card(37), ("J", "H"));
    }

    #[test]
    fn chips_matches_the_server() {
        // the casino's own Mines table wins over the formula (chips.gg cuts to cents)
        let mut t = Tables::builtin();
        assert!((t.mines(3, 1) - 1.125).abs() < 1e-9);
        t.mines.insert("3".into(), vec![0.99, 1.12, 1.28]);
        assert_eq!((t.mines(3, 1), t.mines(3, 2)), (1.12, 1.28));
        assert!((t.mines(3, 3) - mines_multiplier(3, 3)).abs() < 1e-9);
        // HiLo rounds played on chips.gg: K low, a skip, then 3 high
        let first = chips_hilo_next(None, 12.0 / 13.0);
        assert!((first - 1.0725).abs() < 1e-9);
        assert!((chips_hilo_next(Some(first), 11.0 / 13.0) - 1.2675).abs() < 1e-9);
        assert!((chips_hilo_next(None, 0.5) - 1.98).abs() < 1e-9);
        assert_eq!(hilo_side_guesses("A")[2].0, "ace");
        assert_eq!(hilo_side_guesses("10")[2], ("number", 9.0 / 13.0));
        assert_eq!(hilo_side_guesses("J")[2].0, "face");
    }

    #[test]
    fn hilo_auto_plays_by_the_plan() {
        let plan = Plan { cashout: 2.0, pattern: vec![] };
        let round = |state: Value| Round { game: Game::Hilo, id: "1".into(), active: true, currency: "usdt".into(), amount: 0, amount_mult: 1.0, payout: 0, multiplier: 0.0, state };
        // an ace goes higher, a king lower, a 7 higher (a tie), a 3 higher, a 10 lower
        for (card, guess) in [("A", "higher"), ("K", "lower"), ("7", "higherEqual"), ("3", "higherEqual"), ("10", "lowerEqual")] {
            let r = round(json!({ "startCard": { "rank": card, "suit": "S" }, "rounds": [] }));
            assert_eq!(auto_step(&r, Some(&plan)), Some((Step::Next, json!({ "guess": guess }))));
        }
        let on = |m: f64| round(json!({ "startCard": { "rank": "A", "suit": "S" }, "rounds": [{ "card": { "rank": "Q", "suit": "D" }, "guess": "higher", "payoutMultiplier": m }] }));
        assert_eq!(auto_step(&on(1.0725), Some(&plan)), Some((Step::Next, json!({ "guess": "lowerEqual" }))));
        assert_eq!(auto_step(&on(2.5), Some(&plan)), Some((Step::Cashout, json!({}))));
        // no plan: a manual round stays with the board
        assert_eq!(auto_step(&on(2.5), None), None);

        let (sent, plan) = prepare(Game::Hilo, json!({ "startCard": "random", "plan": { "cashout": 100.0 } }));
        assert_eq!(plan, Some(Plan { cashout: 100.0, pattern: vec![] }));
        assert!(sent.get("plan").is_none());
        assert!(RANKS.contains(&sent["startCard"]["rank"].as_str().unwrap()));
        let (sent, plan) = prepare(Game::Hilo, json!({ "startCard": { "rank": "A", "suit": "S" } }));
        assert_eq!((sent["startCard"]["rank"].as_str(), plan), (Some("A"), None));
    }

    #[test]
    fn hilo_patterns() {
        let state = |start: &str, cards: &[(&str, &str)]| {
            let rounds: Vec<Value> = cards.iter().map(|(rank, guess)| json!({ "card": { "rank": rank, "suit": "S" }, "guess": guess, "payoutMultiplier": 2.0 })).collect();
            json!({ "startCard": { "rank": start, "suit": "S" }, "rounds": rounds })
        };
        // three aces in a row from an ace, then the likeliest side
        let p = parse_hilo_pattern("2, 2,2 ,5,5").unwrap();
        assert_eq!(p, vec![2, 2, 2, 5, 5]);
        assert_eq!(hilo_pattern_guess(&state("A", &[]), &p), "equal");
        assert_eq!(hilo_pattern_guess(&state("A", &[("A", "equal"), ("A", "equal")]), &p), "equal");
        assert_eq!(hilo_pattern_guess(&state("A", &[("A", "equal"), ("A", "equal"), ("A", "equal")]), &p), "higher");
        assert_eq!(hilo_pattern_guess(&state("A", &[("A", "equal"), ("A", "equal"), ("A", "equal"), ("9", "higher")]), &p), "lowerEqual");
        // equal waits for an ace or king without using up its code
        let p = parse_hilo_pattern("2,1").unwrap();
        assert_eq!(hilo_pattern_guess(&state("7", &[]), &p), "skip");
        assert_eq!(hilo_pattern_guess(&state("7", &[("9", "skip")]), &p), "skip");
        assert_eq!(hilo_pattern_guess(&state("7", &[("9", "skip"), ("K", "skip")]), &p), "equal");
        assert_eq!(hilo_pattern_guess(&state("7", &[("9", "skip"), ("K", "skip"), ("K", "equal")]), &p), "equal");
        // the pattern starts over: 1, 7, 1, 7 …
        let p = parse_hilo_pattern("1 7").unwrap();
        assert_eq!(hilo_pattern_guess(&state("5", &[("9", "higherEqual")]), &p), "skip");
        assert_eq!(hilo_pattern_guess(&state("5", &[("9", "higherEqual"), ("3", "skip")]), &p), "higherEqual");
        // low and high on the edge cards, the least likely side, and the skip limit
        assert_eq!((hilo_code_guess(0, "A"), hilo_code_guess(1, "K"), hilo_code_guess(0, "K")), ("equal", "equal", "lower"));
        assert_eq!((hilo_code_guess(4, "A"), hilo_code_guess(4, "3"), hilo_code_guess(5, "3")), ("equal", "lowerEqual", "higherEqual"));
        assert!(["higherEqual", "lowerEqual"].contains(&hilo_code_guess(6, "8")));
        let skipped: Vec<(&str, &str)> = vec![("5", "skip"); HILO_SKIPS];
        assert_eq!(hilo_pattern_guess(&state("5", &skipped), &[7, 7, 1]), "higherEqual");
        assert!(parse_hilo_pattern("7,7").is_err() && parse_hilo_pattern("1,8").is_err() && parse_hilo_pattern(" ").is_err());
    }

    #[test]
    fn builtin_tables() {
        let t = Tables::builtin();
        assert_eq!(t.keno("classic", 1), &[0.0, 3.96]);
        assert_eq!(t.wheel("high", 10).len(), 10);
        assert_eq!(t.plinko("high", 16).len(), 17);
    }
}
