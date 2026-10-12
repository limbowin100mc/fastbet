//! The originals' boards and settings, one module per game, plus the pieces
//! they share: tiles, gems, mines, cards and the payout colour scale.
//! [`crate::ui::original`] puts them on the page.

mod baccarat;
mod bars;
mod blackjack;
mod blitz;
mod cases;
mod chicken;
mod darts;
mod dice;
mod diamonds;
mod dragon;
mod drill;
mod flip;
mod hilo;
mod keno;
mod mines;
mod moles;
mod packs;
mod plinko;
mod primedice;
mod pump;
mod reels;
mod roulette;
mod rps;
mod samurai;
mod snakes;
mod tarot;
mod video_poker;
mod wheel;

use std::collections::{HashMap, HashSet};
use std::time::Instant;

use egui::{pos2, vec2, Align2, Color32, CornerRadius, Painter, Pos2, Rect, Shape, Stroke, StrokeKind, Ui};
use serde_json::Value;

use super::theme::{self, alpha, mix, BG2, BG3, GOLD, GREEN, LINE2, RED, TEXT};
use super::App;
use crate::originals::{Game, Round, Tables};

/// What the originals' pages keep between frames.
pub struct State {
    pub tables: Tables,
    pub info_requested: bool,
    /// The casino's own list of its originals, for names and artwork.
    pub catalog: Vec<crate::slots::SlotGame>,
    /// Rounds still open, per game.
    pub open: HashMap<Game, Round>,
    /// The latest round per game, open or settled: what the board shows.
    pub last: HashMap<Game, Round>,
    /// A manual request is on its way; the board waits.
    pub busy: HashSet<Game>,
    /// Games already asked for a round left open.
    pub resumed: HashSet<Game>,
    /// When the latest round landed, for the reveal animation.
    pub landed: Option<(Game, Instant)>,
    /// Steps of FASTBET_ORIG_PLAY already sent.
    pub dev_steps: usize,
    /// Video poker: the round the holds belong to, and which cards are held.
    pub vp_hold: (String, [bool; 5]),
}

impl Default for State {
    fn default() -> Self {
        Self {
            tables: Tables::builtin(),
            info_requested: false,
            catalog: vec![],
            open: HashMap::new(),
            last: HashMap::new(),
            busy: HashSet::new(),
            resumed: HashSet::new(),
            landed: None,
            dev_steps: 0,
            vp_hold: (String::new(), [false; 5]),
        }
    }
}

impl State {
    pub fn open(&self, game: Game) -> Option<&Round> {
        self.open.get(&game)
    }

    pub fn last(&self, game: Game) -> Option<&Round> {
        self.last.get(&game)
    }

    /// Seconds since `game`'s latest round landed.
    pub fn age(&self, game: Game) -> f32 {
        match self.landed {
            Some((g, at)) if g == game => at.elapsed().as_secs_f32(),
            _ => f32::MAX,
        }
    }
}

/// How the game plays, for the "?" on its settings card.
pub fn help(casino: crate::casino::CasinoId, game: Game) -> String {
    let text = match game {
        Game::Dice => "Roll 0–100: the bet wins when the roll lands over (or under) the target. The lower the chance, the more it pays.",
        Game::Keno => "Pick 1 to 10 of the 40 numbers; ten are drawn. The payout depends on the risk and on how many of your numbers are drawn.",
        Game::Mines => "25 tiles hide gems and mines. Every gem raises the payout, a mine ends the round. Manual: reveal tiles and cash out any time. Automatic: every bet reveals the tiles picked on the board, then cashes out.",
        Game::Plinko => "A ball drops through rows of pegs into a bucket, and the bucket's multiplier pays. More risk means bigger edges and smaller middles.",
        Game::Wheel => "A spin lands on one segment, which pays its multiplier. The risk and the number of segments set the payouts.",
        Game::Diamonds => "Five gems are drawn: pairs and better pay like poker hands, up to 50× for five of a kind.",
        Game::DragonTower => "Climb nine rows, one tile per row: eggs are safe, a dragon ends the round, and every row raises the payout. Automatic: click a path on the board; every bet climbs it and cashes out at the top.",
        Game::Pump => "Every pump raises the payout until the balloon pops; harder levels pop sooner and pay more. Automatic: pumps a set number of times, then cashes out.",
        Game::Hilo => "Guess whether the next card is higher or lower: every right guess multiplies the payout, a wrong one ends the round, a skip draws another card. Automatic: every bet starts from the start card and plays the pattern until Cash out or Stop.",
        Game::Flip => "Call heads or tails: every right call multiplies the payout by 1.96, a wrong one ends the round. Automatic: set the calls up front; every bet plays them and cashes out.",
        Game::Blackjack => "Beat the dealer without going over 21. Hit, stand, double, split and insurance are played on the board. Blackjack pays 3:2, insurance 2:1.",
        Game::VideoPoker => "Jacks or better: five cards are dealt, click the ones to hold, then draw once. The final hand pays from the table.",
        Game::Baccarat => "Back the player, the banker or a tie: the hand closest to nine wins. The bet size goes on each side picked.",
        Game::Roulette => "Single-zero roulette. Click the table to put a chip of the bet size on a number or an outside bet; right-click takes one off.",
        Game::Cases => "Open a case: what's inside pays. Harder levels hold bigger prizes and more near-empty cases.",
        Game::Darts => "One dart at a board of rings: the ring it lands in pays. Harder levels have smaller bullseyes that pay more.",
        Game::Tarot => "Three cards are drawn and their multipliers multiply.",
        Game::Packs => "A pack holds five cards, and their multipliers add up.",
        Game::Bars => "Pick up to five of the 30 tiles: each hides a multiplier, and the picked ones add up.",
        Game::Drill => "Three drills dig. Pick one and a target depth (a multiplier): it wins if your drill gets that far.",
        Game::Primedice => "Roll 0–100 and win inside a range, inside either of two ranges, or outside one. The smaller the winning area, the more it pays.",
        Game::Chicken => "Cross lane after lane: every lane raises the payout, a car ends the round. Automatic: crosses a set number of lanes, then cashes out.",
        Game::Snakes => "Roll two dice: the total multiplies the payout, a snake total ends the round. Automatic: rolls a set number of times, then cashes out.",
        Game::Moles => "Each round some of the seven holes hide a mole: whack one to go on. Automatic: plan one hole per round on the board.",
        Game::RockPaperScissors => "Rock, paper, scissors against the house: every win multiplies the payout, a loss ends the round. Automatic: set the throws up front.",
        Game::ScarabSpin => "A five-reel slot: a spin pays its winning lines and scatters. The bet size is spread over the lines played.",
        Game::TomeOfLife => "A five-reel slot: a spin pays its winning lines; bonus rounds play out by themselves. The bet size is spread over the lines played.",
        Game::BlueSamurai => "A slot with uneven reels: a spin pays its wins, and bonus spins play out by themselves.",
        Game::Blitz => "Cards come from an endless deck: the bet pays when every card drawn is different. More cards pay more.",
    };
    if game == Game::Hilo && casino.hilo_side_guesses() {
        return format!("{text} chips.gg also takes red / black and card-kind guesses, and takes its 1% edge once per round.");
    }
    text.to_string()
}

/// The game's settings, under the bet size.
pub fn params(app: &mut App, ui: &mut Ui, game: Game, locked: bool) {
    match game {
        Game::Dice => dice::params(app, ui, locked),
        Game::Keno => keno::params(app, ui, locked),
        Game::Mines => mines::params(app, ui, locked),
        Game::Plinko => plinko::params(app, ui, locked),
        Game::Wheel => wheel::params(app, ui, locked),
        Game::Diamonds => diamonds::params(app, ui),
        Game::DragonTower => dragon::params(app, ui, locked),
        Game::Pump => pump::params(app, ui, locked),
        Game::Hilo => hilo::params(app, ui, locked),
        Game::Flip => flip::params(app, ui, locked),
        Game::Blackjack => blackjack::params(app, ui),
        Game::VideoPoker => video_poker::params(app, ui),
        Game::Baccarat => baccarat::params(app, ui, locked),
        Game::Roulette => roulette::params(app, ui, locked),
        Game::Cases => cases::params(app, ui, locked),
        Game::Darts => darts::params(app, ui, locked),
        Game::Tarot => tarot::params(app, ui, locked),
        Game::Packs => packs::params(app, ui),
        Game::Bars => bars::params(app, ui, locked),
        Game::Drill => drill::params(app, ui, locked),
        Game::Primedice => primedice::params(app, ui, locked),
        Game::Chicken => chicken::params(app, ui, locked),
        Game::Snakes => snakes::params(app, ui, locked),
        Game::Moles => moles::params(app, ui, locked),
        Game::RockPaperScissors => rps::params(app, ui, locked),
        Game::ScarabSpin | Game::TomeOfLife => reels::params(app, ui, game, locked),
        Game::BlueSamurai => samurai::params(app, ui),
        Game::Blitz => blitz::params(app, ui, locked),
    }
}

/// The board: draws the game in `rect` and handles its clicks.
pub fn board(app: &mut App, ui: &mut Ui, game: Game, rect: Rect) {
    match game {
        Game::Dice => dice::board(app, ui, rect),
        Game::Keno => keno::board(app, ui, rect),
        Game::Mines => mines::board(app, ui, rect),
        Game::Plinko => plinko::board(app, ui, rect),
        Game::Wheel => wheel::board(app, ui, rect),
        Game::Diamonds => diamonds::board(app, ui, rect),
        Game::DragonTower => dragon::board(app, ui, rect),
        Game::Pump => pump::board(app, ui, rect),
        Game::Hilo => hilo::board(app, ui, rect),
        Game::Flip => flip::board(app, ui, rect),
        Game::Blackjack => blackjack::board(app, ui, rect),
        Game::VideoPoker => video_poker::board(app, ui, rect),
        Game::Baccarat => baccarat::board(app, ui, rect),
        Game::Roulette => roulette::board(app, ui, rect),
        Game::Cases => cases::board(app, ui, rect),
        Game::Darts => darts::board(app, ui, rect),
        Game::Tarot => tarot::board(app, ui, rect),
        Game::Packs => packs::board(app, ui, rect),
        Game::Bars => bars::board(app, ui, rect),
        Game::Drill => drill::board(app, ui, rect),
        Game::Primedice => primedice::board(app, ui, rect),
        Game::Chicken => chicken::board(app, ui, rect),
        Game::Snakes => snakes::board(app, ui, rect),
        Game::Moles => moles::board(app, ui, rect),
        Game::RockPaperScissors => rps::board(app, ui, rect),
        Game::ScarabSpin | Game::TomeOfLife => reels::board(app, ui, rect, game),
        Game::BlueSamurai => samurai::board(app, ui, rect),
        Game::Blitz => blitz::board(app, ui, rect),
    }
}

/// The game's fields for a bet: a manual round, or every bet of an auto run.
pub fn bet_params(app: &App, game: Game, auto: bool) -> Result<Value, String> {
    match game {
        Game::Dice => dice::bet_params(app),
        Game::Keno => keno::bet_params(app),
        Game::Mines => mines::bet_params(app, auto),
        Game::Plinko => plinko::bet_params(app),
        Game::Wheel => wheel::bet_params(app),
        Game::Diamonds => Ok(serde_json::json!({})),
        Game::DragonTower => dragon::bet_params(app, auto),
        Game::Pump => pump::bet_params(app, auto),
        Game::Hilo => hilo::bet_params(app, auto),
        Game::Flip => flip::bet_params(app, auto),
        Game::Blackjack => blackjack::bet_params(),
        Game::VideoPoker => video_poker::bet_params(),
        Game::Baccarat => baccarat::bet_params(app),
        Game::Roulette => roulette::bet_params(app),
        Game::Cases => cases::bet_params(app),
        Game::Darts => darts::bet_params(app),
        Game::Tarot => tarot::bet_params(app),
        Game::Packs => packs::bet_params(),
        Game::Bars => bars::bet_params(app),
        Game::Drill => drill::bet_params(app),
        Game::Primedice => primedice::bet_params(app),
        Game::Chicken => chicken::bet_params(app, auto),
        Game::Snakes => snakes::bet_params(app, auto),
        Game::Moles => moles::bet_params(app, auto),
        Game::RockPaperScissors => rps::bet_params(app, auto),
        Game::ScarabSpin | Game::TomeOfLife => reels::bet_params(app, game),
        Game::BlueSamurai => samurai::bet_params(),
        Game::Blitz => blitz::bet_params(app),
    }
}

/// An auto run's stop multiplier of the game's own (HiLo's Stop), which ends it like a hit.
pub fn stop_line(app: &App, game: Game) -> Option<f64> {
    match game {
        Game::Hilo => hilo::stop_line(app),
        _ => None,
    }
}

/// One line about a settled round, for the history table.
pub fn result_text(r: &Round) -> String {
    match r.game {
        Game::Dice => dice::result_text(r),
        Game::Keno => keno::result_text(r),
        Game::Mines => mines::result_text(r),
        Game::Plinko => plinko::result_text(r),
        Game::Wheel => format!("{}×", trim(r.multiplier)),
        Game::Diamonds => diamonds::result_text(r),
        Game::DragonTower => dragon::result_text(r),
        Game::Pump => pump::result_text(r),
        Game::Hilo => hilo::result_text(r),
        Game::Flip => flip::result_text(r),
        Game::Blackjack => blackjack::result_text(r),
        Game::VideoPoker => video_poker::result_text(r),
        Game::Baccarat => baccarat::result_text(r),
        Game::Roulette => roulette::result_text(r),
        Game::Cases => cases::result_text(r),
        Game::Darts => darts::result_text(r),
        Game::Tarot => tarot::result_text(r),
        Game::Packs => packs::result_text(r),
        Game::Bars => bars::result_text(r),
        Game::Drill => drill::result_text(r),
        Game::Primedice => primedice::result_text(r),
        Game::Chicken => chicken::result_text(r),
        Game::Snakes => snakes::result_text(r),
        Game::Moles => moles::result_text(r),
        Game::RockPaperScissors => rps::result_text(r),
        Game::ScarabSpin | Game::TomeOfLife => reels::result_text(r),
        Game::BlueSamurai => samurai::result_text(r),
        Game::Blitz => blitz::result_text(r),
    }
}

/// What a game's settings add up to, on the settings card ("5 picked", "pays 1.28× …").
pub fn readout(ui: &mut Ui, text: impl Into<String>) {
    super::widgets::readout(ui, text, theme::TEXT2, theme::G1);
}

/// A readout of settings still to be made ("Pick at least one side").
pub fn readout_idle(ui: &mut Ui, text: impl Into<String>) {
    super::widgets::readout(ui, text, theme::MUTED, LINE2);
}

/// A readout of settings that don't work as they are (a range out of order).
pub fn readout_bad(ui: &mut Ui, text: impl Into<String>) {
    super::widgets::readout(ui, text, RED, RED);
}

/// For games with nothing to set: what to do instead.
pub fn nothing_to_set(ui: &mut Ui, text: &str) {
    super::widgets::note(ui, text);
}

/// "1 flip", "3 flips".
pub fn count(n: u64, word: &str) -> String {
    format!("{n} {word}{}", if n == 1 { "" } else { "s" })
}

/// A multiplier without trailing zeros: 2 -> "2", 1.125 -> "1.125", 0.2 -> "0.2".
pub fn trim(m: f64) -> String {
    let s = format!("{m:.4}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s.is_empty() {
        "0".into()
    } else {
        s.to_string()
    }
}

/// Multiplier with two decimals and the × sign.
pub fn times(m: f64) -> String {
    if m >= 1000.0 {
        format!("{}×", crate::money::group_thousands(&format!("{}", m.round() as i64)))
    } else {
        format!("{m:.2}×")
    }
}

// ------------------------------------------------------------------ drawing

/// How a tile looks.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Tile {
    /// Face down, can be clicked.
    Idle,
    /// Face down, not clickable right now.
    Off,
    /// Picked by the player (keno numbers, auto tiles).
    Picked,
    /// Revealed and safe.
    Good,
    /// Revealed and lost.
    Bad,
    /// Revealed after the round, not picked: shown faded.
    Faded,
}

/// A raised tile, Stake-style: a lighter top over a darker bottom edge.
pub fn tile(p: &Painter, rect: Rect, kind: Tile, hovered: bool) {
    let r = CornerRadius::same(((rect.height() * 0.12) as u8).clamp(4, 10));
    let (top, edge) = match kind {
        Tile::Idle if hovered => (mix(BG3, LINE2, 0.9), BG2),
        Tile::Idle => (mix(BG3, LINE2, 0.55), mix(BG2, BG3, 0.3)),
        Tile::Off => (mix(BG3, LINE2, 0.35), BG2),
        Tile::Picked => (Color32::from_rgb(0x8b, 0x5c, 0xf6), Color32::from_rgb(0x5b, 0x34, 0xc4)),
        Tile::Good => (Color32::from_rgb(0x0f, 0x21, 0x2e), Color32::from_rgb(0x0b, 0x17, 0x20)),
        Tile::Bad => (Color32::from_rgb(0x2a, 0x10, 0x18), Color32::from_rgb(0x1c, 0x0a, 0x10)),
        Tile::Faded => (alpha(Color32::from_rgb(0x0f, 0x21, 0x2e), 150), alpha(Color32::from_rgb(0x0b, 0x17, 0x20), 150)),
    };
    let lift = if matches!(kind, Tile::Idle | Tile::Off | Tile::Picked) { (rect.height() * 0.07).clamp(2.0, 5.0) } else { 0.0 };
    let lift = if hovered && kind == Tile::Idle { lift + 1.5 } else { lift };
    p.rect_filled(rect, r, edge);
    let face = Rect::from_min_max(rect.min, pos2(rect.max.x, rect.max.y - lift));
    p.rect_filled(face, r, top);
    if kind == Tile::Good {
        p.rect_stroke(rect, r, Stroke::new(1.5, alpha(GREEN, 140)), StrokeKind::Inside);
    }
    if kind == Tile::Bad {
        p.rect_stroke(rect, r, Stroke::new(1.5, alpha(RED, 150)), StrokeKind::Inside);
    }
}

/// The face of a tile without its bottom edge, where an icon goes.
pub fn tile_face(rect: Rect, kind: Tile) -> Rect {
    let lift = if matches!(kind, Tile::Idle | Tile::Off | Tile::Picked) { (rect.height() * 0.07).clamp(2.0, 5.0) } else { 0.0 };
    Rect::from_min_max(rect.min, pos2(rect.max.x, rect.max.y - lift))
}

/// A cut gem: a crown over a pointed pavilion, with a highlight facet.
pub fn gem(p: &Painter, c: Pos2, size: f32, color: Color32) {
    let w = size * 0.5;
    let top = c.y - size * 0.32;
    let girdle = c.y - size * 0.08;
    let tip = c.y + size * 0.42;
    let dark = mix(color, Color32::BLACK, 0.35);
    let light = mix(color, Color32::WHITE, 0.45);
    // pavilion
    p.add(Shape::convex_polygon(vec![pos2(c.x - w, girdle), pos2(c.x + w, girdle), pos2(c.x, tip)], dark, Stroke::NONE));
    p.add(Shape::convex_polygon(vec![pos2(c.x - w * 0.35, girdle), pos2(c.x + w * 0.35, girdle), pos2(c.x, tip)], color, Stroke::NONE));
    // crown
    p.add(Shape::convex_polygon(
        vec![pos2(c.x - w * 0.55, top), pos2(c.x + w * 0.55, top), pos2(c.x + w, girdle), pos2(c.x - w, girdle)],
        color,
        Stroke::NONE,
    ));
    p.add(Shape::convex_polygon(vec![pos2(c.x - w * 0.55, top), pos2(c.x, top), pos2(c.x - w * 0.35, girdle), pos2(c.x - w, girdle)], light, Stroke::NONE));
}

/// A round bomb with a fuse and a spark.
pub fn bomb(p: &Painter, c: Pos2, size: f32) {
    let r = size * 0.32;
    let body = Color32::from_rgb(0xe8, 0x33, 0x4f);
    let cc = pos2(c.x, c.y + size * 0.06);
    p.circle_filled(cc, r, mix(body, Color32::BLACK, 0.25));
    p.circle_filled(cc - vec2(r * 0.12, r * 0.12), r * 0.86, body);
    p.circle_filled(cc - vec2(r * 0.38, r * 0.38), r * 0.22, alpha(Color32::WHITE, 120));
    let neck = cc + vec2(r * 0.55, -r * 0.7);
    p.rect_filled(Rect::from_center_size(neck, vec2(r * 0.45, r * 0.3)), CornerRadius::same(2), mix(body, Color32::BLACK, 0.45));
    let spark = neck + vec2(r * 0.45, -r * 0.45);
    p.line_segment([neck, spark], Stroke::new((size * 0.04).max(1.0), Color32::from_rgb(0xc9, 0xb8, 0x9a)));
    p.circle_filled(spark, r * 0.18, GOLD);
}

/// Payout colour: grey for nothing, then green, yellow, orange, red as the multiplier grows.
pub fn payout_color(m: f64) -> Color32 {
    let stops: [(f64, Color32); 6] = [
        (0.0, Color32::from_rgb(0x55, 0x60, 0x75)),
        (1.0, Color32::from_rgb(0x2b, 0xe4, 0xa6)),
        (2.0, Color32::from_rgb(0xb6, 0xe3, 0x3a)),
        (5.0, Color32::from_rgb(0xff, 0xd1, 0x4a)),
        (20.0, Color32::from_rgb(0xff, 0x8a, 0x3d)),
        (100.0, Color32::from_rgb(0xff, 0x3d, 0x5f)),
    ];
    if m <= 0.0 {
        return stops[0].1;
    }
    for w in stops.windows(2) {
        if m <= w[1].0 {
            let t = ((m - w[0].0) / (w[1].0 - w[0].0)).clamp(0.0, 1.0) as f32;
            return mix(w[0].1, w[1].1, t);
        }
    }
    stops[5].1
}

/// A playing card: white face, rank top-left, the suit large in the middle.
pub fn card(p: &Painter, rect: Rect, rank: &str, suit: &str, dim: bool) {
    let r = CornerRadius::same((rect.width() * 0.09) as u8);
    p.rect_filled(rect.translate(vec2(0.0, 3.0)), r, alpha(Color32::BLACK, 90));
    p.rect_filled(rect, r, if dim { Color32::from_rgb(0xb8, 0xbf, 0xcc) } else { Color32::WHITE });
    let color = suit_color(suit);
    let fs = rect.width() * 0.3;
    p.text(rect.left_top() + vec2(rect.width() * 0.12, rect.width() * 0.06), Align2::LEFT_TOP, rank, theme::font_bold(fs), color);
    suit_shape(p, pos2(rect.center().x, rect.center().y + rect.height() * 0.1), rect.width() * 0.42, suit, color);
}

pub fn suit_color(suit: &str) -> Color32 {
    if matches!(suit, "H" | "D") {
        Color32::from_rgb(0xe8, 0x33, 0x4f)
    } else {
        Color32::from_rgb(0x1a, 0x23, 0x36)
    }
}

/// Hearts, diamonds, clubs and spades from circles and polygons (no font needed).
pub fn suit_shape(p: &Painter, c: Pos2, size: f32, suit: &str, color: Color32) {
    let s = size / 2.0;
    match suit {
        "D" => {
            p.add(Shape::convex_polygon(vec![pos2(c.x, c.y - s), pos2(c.x + s * 0.72, c.y), pos2(c.x, c.y + s), pos2(c.x - s * 0.72, c.y)], color, Stroke::NONE));
        }
        "H" => {
            let r = s * 0.5;
            p.circle_filled(pos2(c.x - r * 0.95, c.y - s * 0.3), r, color);
            p.circle_filled(pos2(c.x + r * 0.95, c.y - s * 0.3), r, color);
            p.add(Shape::convex_polygon(vec![pos2(c.x - s * 0.95, c.y - s * 0.12), pos2(c.x + s * 0.95, c.y - s * 0.12), pos2(c.x, c.y + s * 0.9)], color, Stroke::NONE));
        }
        "S" => {
            let r = s * 0.5;
            p.circle_filled(pos2(c.x - r * 0.95, c.y + s * 0.2), r, color);
            p.circle_filled(pos2(c.x + r * 0.95, c.y + s * 0.2), r, color);
            p.add(Shape::convex_polygon(vec![pos2(c.x, c.y - s * 0.95), pos2(c.x + s * 0.95, c.y + s * 0.05), pos2(c.x - s * 0.95, c.y + s * 0.05)], color, Stroke::NONE));
            p.add(Shape::convex_polygon(vec![pos2(c.x, c.y + s * 0.3), pos2(c.x + s * 0.35, c.y + s), pos2(c.x - s * 0.35, c.y + s)], color, Stroke::NONE));
        }
        _ => {
            let r = s * 0.42;
            p.circle_filled(pos2(c.x, c.y - s * 0.45), r, color);
            p.circle_filled(pos2(c.x - s * 0.48, c.y + s * 0.12), r, color);
            p.circle_filled(pos2(c.x + s * 0.48, c.y + s * 0.12), r, color);
            p.add(Shape::convex_polygon(vec![pos2(c.x, c.y), pos2(c.x + s * 0.32, c.y + s), pos2(c.x - s * 0.32, c.y + s)], color, Stroke::NONE));
        }
    }
}

/// A small rounded label with a value, used for payout strips.
pub fn chip(p: &Painter, rect: Rect, text: &str, fill: Color32, fg: Color32) {
    p.rect_filled(rect, CornerRadius::same(6), fill);
    super::widgets::fit_text(p, rect.center(), Align2::CENTER_CENTER, text, theme::font_semi((rect.height() * 0.42).clamp(9.0, 13.0)), fg, rect.width() - 6.0);
}

/// A line of muted text centred under the board's content.
pub fn note(p: &Painter, pos: Pos2, text: &str) {
    p.text(pos, Align2::CENTER_CENTER, text, theme::font_semi(12.0), theme::MUTED);
}

/// A clickable region with the pointing-hand cursor.
pub fn hit(ui: &mut Ui, rect: Rect, id: impl std::hash::Hash + std::fmt::Debug, enabled: bool) -> egui::Response {
    let resp = ui.interact(rect, ui.id().with(id), if enabled { egui::Sense::click() } else { egui::Sense::hover() });
    if enabled {
        resp.on_hover_cursor(egui::CursorIcon::PointingHand)
    } else {
        resp
    }
}

/// Square cells of `n` columns × `m` rows, centred in `rect` with `gap` between them.
pub fn grid(rect: Rect, cols: usize, rows: usize, gap: f32, max_cell: f32) -> Vec<Rect> {
    let cell = ((rect.width() - gap * (cols as f32 - 1.0)) / cols as f32).min((rect.height() - gap * (rows as f32 - 1.0)) / rows as f32).min(max_cell);
    let w = cell * cols as f32 + gap * (cols as f32 - 1.0);
    let h = cell * rows as f32 + gap * (rows as f32 - 1.0);
    let x0 = rect.center().x - w / 2.0;
    let y0 = rect.center().y - h / 2.0;
    let mut out = Vec::with_capacity(cols * rows);
    for r in 0..rows {
        for c in 0..cols {
            out.push(Rect::from_min_size(pos2(x0 + c as f32 * (cell + gap), y0 + r as f32 * (cell + gap)), vec2(cell, cell)));
        }
    }
    out
}

/// Values of a JSON array of numbers.
pub fn nums(v: &Value) -> Vec<i64> {
    v.as_array().map(|a| a.iter().filter_map(Value::as_i64).collect()).unwrap_or_default()
}

pub(crate) fn text_color_on(c: Color32) -> Color32 {
    let l = 0.299 * c.r() as f32 + 0.587 * c.g() as f32 + 0.114 * c.b() as f32;
    if l > 150.0 {
        Color32::from_rgb(0x07, 0x0b, 0x14)
    } else {
        TEXT
    }
}

/// A difficulty picker bound to `value`. Returns true when it changed.
pub fn levels(ui: &mut Ui, value: &mut String, keys: &[&str], names: &[&str], enabled: bool) -> bool {
    super::widgets::caption(ui, "Difficulty");
    let sel = keys.iter().position(|k| *k == value.as_str()).unwrap_or(0);
    match super::widgets::segmented(ui, names, sel, enabled) {
        Some(i) if keys[i] != value.as_str() => {
            *value = keys[i].to_string();
            true
        }
        _ => false,
    }
}

/// A button placed at `rect` on the board.
pub fn board_button(ui: &mut Ui, rect: Rect, label: &str, kind: super::widgets::Btn, enabled: bool) -> bool {
    let mut bui = ui.new_child(egui::UiBuilder::new().max_rect(rect));
    super::widgets::button(&mut bui, label, kind, enabled, Some(rect.width())).clicked()
}

/// A die showing `n` pips.
pub fn die(p: &Painter, rect: Rect, n: u32, color: Color32) {
    p.rect_filled(rect.translate(vec2(0.0, rect.height() * 0.06)), CornerRadius::same((rect.width() * 0.18) as u8), mix(color, Color32::BLACK, 0.35));
    p.rect_filled(rect, CornerRadius::same((rect.width() * 0.18) as u8), color);
    let c = rect.center();
    let d = rect.width() * 0.26;
    let pip = |dx: f32, dy: f32| p.circle_filled(pos2(c.x + dx * d, c.y + dy * d), rect.width() * 0.09, Color32::from_rgb(0x1a, 0x23, 0x36));
    let spots: &[(f32, f32)] = match n {
        1 => &[(0.0, 0.0)],
        2 => &[(-1.0, -1.0), (1.0, 1.0)],
        3 => &[(-1.0, -1.0), (0.0, 0.0), (1.0, 1.0)],
        4 => &[(-1.0, -1.0), (1.0, -1.0), (-1.0, 1.0), (1.0, 1.0)],
        5 => &[(-1.0, -1.0), (1.0, -1.0), (0.0, 0.0), (-1.0, 1.0), (1.0, 1.0)],
        _ => &[(-1.0, -1.0), (1.0, -1.0), (-1.0, 0.0), (1.0, 0.0), (-1.0, 1.0), (1.0, 1.0)],
    };
    for (dx, dy) in spots {
        pip(*dx, *dy);
    }
}

/// Cards in a casino reply (`[{rank, suit}]`).
pub fn cards_of(v: &Value) -> Vec<(String, String)> {
    v.as_array()
        .map(|a| a.iter().map(|c| (c["rank"].as_str().unwrap_or("?").to_string(), c["suit"].as_str().unwrap_or("S").to_string())).collect())
        .unwrap_or_default()
}

/// A row of cards centred on `center`, overlapping a little when space is short.
pub fn card_row(p: &Painter, center: Pos2, cards: &[(String, String)], card_w: f32, max_w: f32) {
    if cards.is_empty() {
        return;
    }
    let h = card_w * 1.4;
    let step = ((max_w - card_w) / (cards.len() as f32 - 1.0).max(1.0)).min(card_w + 8.0);
    let total = card_w + step * (cards.len() as f32 - 1.0);
    let x0 = center.x - total / 2.0;
    for (i, (rank, suit)) in cards.iter().enumerate() {
        card(p, Rect::from_min_size(pos2(x0 + i as f32 * step, center.y - h / 2.0), vec2(card_w, h)), rank, suit, false);
    }
}

/// The face-down card back.
pub fn card_back(p: &Painter, rect: Rect) {
    let r = CornerRadius::same((rect.width() * 0.09) as u8);
    p.rect_filled(rect.translate(vec2(0.0, 3.0)), r, alpha(Color32::BLACK, 90));
    p.rect_filled(rect, r, Color32::from_rgb(0x1f, 0x6f, 0xeb));
    p.rect_stroke(rect.shrink(rect.width() * 0.08), r, Stroke::new(1.5, alpha(Color32::WHITE, 90)), StrokeKind::Inside);
}
