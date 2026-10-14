//! Slot games: the catalog types, the provider driver trait and the drivers.
//! A driver talks to the slot provider's own game server through the
//! browser bridge, skipping the game client (and its animations) entirely.

pub mod avatarux;
pub mod bgaming;
pub mod hacksaw;
pub mod onetouch;
pub mod onlyplay;
pub mod outstanding;
pub mod playson;
pub mod pragmatic;
pub mod relax;
pub mod slotmill;
pub mod spinomenal;
pub mod stake_engine;

use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::money::Units;

/// One catalog entry from the casino.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct SlotGame {
    pub id: String,
    pub name: String,
    pub producer: String,
    pub image: String,
    pub demo: bool,
    pub rtp: f64,
    /// Provider-side game id (e.g. Hacksaw's numeric id).
    pub provider_game_id: Option<String>,
    /// Studio that made the game, where the producer is a platform (Stake Engine).
    #[serde(default)]
    pub studio: Option<String>,
}

impl SlotGame {
    /// Who made the game, for labels.
    pub fn maker(&self) -> &str {
        self.studio.as_deref().unwrap_or_else(|| Provider::display_producer(&self.producer))
    }
}

/// Which driver handles a producer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Provider {
    Hacksaw,
    Avatarux,
    Bgaming,
    StakeEngine,
    OneTouch,
    Onlyplay,
    /// Penguin King, on Outstanding Games' platform
    Outstanding,
    Slotmill,
    Relax,
    Pragmatic,
    Playson,
    Spinomenal,
}

impl Provider {
    pub fn for_producer(producer: &str) -> Option<Provider> {
        match producer {
            // Hacksaw's own studios and the ones it hosts (Shuffle's names) launch on its RGS
            "hacksawgaming" | "bullsharkgames" | "nownow" | "trustygaming" | "kitsunegaming" | "aceroll" | "goodtimesstudios"
            | "pineappleplay" | "alohagaming" => Some(Provider::Hacksaw),
            // studios on AvatarUX's RGS (Hub88's b2b platform)
            "avatarux" | "peter&sons" | "shadylady" | "wickedgames" => Some(Provider::Avatarux),
            // Popiplay runs on BGaming's platform
            "bgaming" | "popiplay" => Some(Provider::Bgaming),
            STAKE_ENGINE => Some(Provider::StakeEngine),
            "onetouch" => Some(Provider::OneTouch),
            "onlyplay" => Some(Provider::Onlyplay),
            "penguinking" => Some(Provider::Outstanding),
            "slotmillgames" => Some(Provider::Slotmill),
            "relaxgaming" => Some(Provider::Relax),
            "pragmaticplay" => Some(Provider::Pragmatic),
            "playson" => Some(Provider::Playson),
            "spinomenal" => Some(Provider::Spinomenal),
            _ => None,
        }
    }
    pub fn display_producer(producer: &str) -> &str {
        match producer {
            "hacksawgaming" => "Hacksaw Gaming",
            "bullsharkgames" => "Bullshark Games",
            "nownow" => "Nownow",
            "trustygaming" => "Trusty Gaming",
            "kitsunegaming" => "Kitsune Gaming",
            "aceroll" => "Aceroll",
            "goodtimesstudios" => "Goodtimes Studios",
            "pineappleplay" => "Pineapple Play",
            "alohagaming" => "Aloha Gaming",
            "avatarux" => "AvatarUX",
            "peter&sons" => "Peter & Sons",
            "shadylady" => "Shady Lady",
            "wickedgames" => "Wicked Games",
            "bgaming" => "BGaming",
            "popiplay" => "Popiplay",
            STAKE_ENGINE => "Stake Engine",
            "onetouch" => "OneTouch",
            "onlyplay" => "Onlyplay",
            "penguinking" => "Penguin King",
            "slotmillgames" => "Slotmill Games",
            "relaxgaming" => "Relax Gaming",
            "playson" => "Playson",
            "spinomenal" => "Spinomenal",
            "nolimitcity" => "Nolimit City",
            "pragmaticplay" => "Pragmatic Play",
            other => other,
        }
    }
}

/// Opens a session from the casino's launch URL (before the launcher's redirects).
/// Hacksaw demos and Stake Engine are opened by the engine itself.
pub async fn open_launch(provider: Provider, launch_url: &str) -> Result<Arc<dyn SlotSession>, String> {
    let final_url = || async { resolve_launcher(launch_url).await };
    let bad = |url: &str| format!("Could not read the launch parameters from {url}");
    match provider {
        Provider::Hacksaw => {
            let url = final_url().await?;
            let mut l = hacksaw::Launch::from_url(&url, "USD").ok_or_else(|| bad(&url))?;
            if l.mode == 1 {
                l.env = hacksaw::live_env(&l.partner).await;
            }
            Ok(Arc::new(hacksaw::HacksawSession::open(l).await.map_err(|e| e.to_string())?))
        }
        Provider::Avatarux => {
            let url = final_url().await?;
            let l = avatarux::Launch::from_url(&url).ok_or_else(|| bad(&url))?;
            Ok(Arc::new(avatarux::AvataruxSession::open(l).await.map_err(|e| e.to_string())?))
        }
        Provider::Bgaming => {
            let url = final_url().await?;
            let l = bgaming::Launch::from_url(&url).ok_or_else(|| bad(&url))?;
            Ok(Arc::new(bgaming::BgamingSession::open(l).await.map_err(|e| e.to_string())?))
        }
        Provider::StakeEngine => {
            let l = stake_engine::Launch::from_url(launch_url).ok_or("Stake didn't return a game session")?;
            Ok(Arc::new(stake_engine::StakeEngineSession::open(l).await.map_err(|e| e.to_string())?))
        }
        Provider::OneTouch => Ok(Arc::new(onetouch::open(launch_url).await.map_err(|e| e.to_string())?)),
        Provider::Onlyplay => Ok(Arc::new(onlyplay::open(launch_url).await.map_err(|e| e.to_string())?)),
        Provider::Outstanding => Ok(Arc::new(outstanding::open(launch_url).await.map_err(|e| e.to_string())?)),
        Provider::Slotmill => Ok(Arc::new(slotmill::open(launch_url).await.map_err(|e| e.to_string())?)),
        Provider::Relax => Ok(Arc::new(relax::open(launch_url).await.map_err(|e| e.to_string())?)),
        Provider::Pragmatic => Ok(Arc::new(pragmatic::open(launch_url).await.map_err(|e| e.to_string())?)),
        Provider::Playson => Ok(Arc::new(playson::open(launch_url).await.map_err(|e| e.to_string())?)),
        Provider::Spinomenal => Ok(Arc::new(spinomenal::open(launch_url).await.map_err(|e| e.to_string())?)),
    }
}

/// Follows the casino's launcher redirects (plain HTTP, no browser needed) and
/// returns the final URL whose query string carries the launch parameters.
pub async fn resolve_launcher(url: &str) -> Result<String, String> {
    Ok(follow_launcher(url).await?.0)
}

/// Like [`resolve_launcher`], also returning the final page's body (some launchers
/// end on a page that carries the parameters in its HTML instead of its URL).
pub async fn follow_launcher(url: &str) -> Result<(String, String), String> {
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .user_agent(UA)
        .build()
        .map_err(|e| e.to_string())?;
    let mut current = url.to_string();
    for _ in 0..8 {
        let res = client.get(&current).send().await.map_err(|e| format!("launcher: {e}"))?;
        if res.status().is_redirection() {
            if let Some(loc) = res.headers().get("location").and_then(|v| v.to_str().ok()) {
                current = if loc.starts_with("http") { loc.to_string() } else { format!("{}{}", current.split('/').take(3).collect::<Vec<_>>().join("/"), loc) };
                continue;
            }
        }
        let body = res.text().await.unwrap_or_default();
        return Ok((current, body));
    }
    Ok((current, String::new()))
}

/// The browser we present ourselves as on plain HTTP calls.
pub const UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/154.0.0.0 Safari/537.36";

/// `fastbet --slot-test <producer> <launch-url> [spins] [feature]`: opens a session the
/// way the app does and spins it (demo launch URLs only, unless you mean it).
pub async fn self_test(producer: &str, launch_url: &str, spins: usize, feature: Option<&str>) {
    let Some(provider) = Provider::for_producer(producer) else {
        eprintln!("no driver for {producer}");
        return;
    };
    let t0 = std::time::Instant::now();
    let s = match open_launch(provider, launch_url).await {
        Ok(s) => s,
        Err(e) => {
            eprintln!("open failed: {e}");
            return;
        }
    };
    let info = s.info().clone();
    println!(
        "opened in {} ms: currency {} decimals {} bets {:?} default {:?} balance {:?}",
        t0.elapsed().as_millis(),
        info.currency,
        info.decimals,
        info.bet_levels.iter().map(|b| b.minor).collect::<Vec<_>>(),
        info.default_bet,
        info.balance
    );
    for f in &info.features {
        println!("  feature {} \"{}\" cost ×{} rtp {:?}", f.id, f.name, f.cost_mult, f.rtp);
    }
    let bet = info.default_bet.or(info.bet_levels.first().map(|b| b.minor)).unwrap_or(100);
    for i in 0..spins {
        let feat = if i + 1 == spins { feature } else { None };
        match s.spin(bet, feat, false).await {
            Ok(r) => println!(
                "spin {}{} round {} cost {} win {} x{:.2} balance {:?} pending {} fs {} events {} {:.0} ms",
                i + 1,
                feat.map(|f| format!(" [{f}]")).unwrap_or_default(),
                r.round_id,
                r.cost,
                r.win,
                r.multiplier,
                r.balance,
                r.pending,
                r.free_spins,
                r.events,
                r.ms
            ),
            Err(e) => println!("spin {} failed: {e}", i + 1),
        }
    }
    s.close();
}

/// Producer key of games running on Stake Engine (made by many studios).
pub const STAKE_ENGINE: &str = "stake-engine";

/// How a session was launched (real money or demo).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum PlayMode {
    Real,
    Demo,
}

/// A bet level the game offers, in the session currency's minor units
/// (cents for fiat) with `decimals` fraction digits.
#[derive(Clone, Debug, PartialEq)]
pub struct BetLevel {
    pub raw: String,
    pub minor: i64,
}

/// A buyable feature / feature spin.
#[derive(Clone, Debug, PartialEq)]
pub struct Feature {
    pub id: String,
    pub name: String,
    /// Cost as a multiple of the bet level.
    pub cost_mult: f64,
    pub rtp: Option<f64>,
}

#[derive(Clone, Debug, Default)]
pub struct SessionInfo {
    pub currency: String,
    pub decimals: u32,
    pub bet_levels: Vec<BetLevel>,
    pub default_bet: Option<i64>,
    pub features: Vec<Feature>,
    /// Wallet balance in minor units, when the provider reports it.
    pub balance: Option<i64>,
}

#[derive(Clone, Debug, Default)]
pub struct SpinResult {
    pub round_id: String,
    /// What was charged, in minor units (bet level × feature cost).
    pub cost: i64,
    /// Total round win in minor units.
    pub win: i64,
    pub multiplier: f64,
    pub balance: Option<i64>,
    /// Round still open server-side (bonus in progress etc.).
    pub pending: bool,
    pub free_spins: bool,
    /// The spin triggered free spins and, as asked, the round was left open for the game in the
    /// browser: the free spins unplayed, or the bonus not yet shown and closed. `win` is then only
    /// what was paid before the free spins (0 where the provider decides the whole bonus in the
    /// spin: its result stays for the browser to show).
    pub held: bool,
    pub events: usize,
    pub ms: f64,
}

#[derive(Debug, Clone, thiserror::Error)]
pub enum SlotError {
    #[error("{0}")]
    Provider(String),
    #[error("Insufficient balance")]
    Balance,
    #[error("Rate limited by the provider")]
    RateLimited,
    #[error("Session expired")]
    SessionExpired,
    #[error("{0}")]
    Bridge(String),
}

#[async_trait]
pub trait SlotSession: Send + Sync {
    fn info(&self) -> &SessionInfo;
    /// One spin at `bet` minor units, optionally buying `feature`. `hold`: when the spin triggers
    /// free spins, stop there and leave the round open for the browser (see [`SpinResult::held`]).
    async fn spin(&self, bet: i64, feature: Option<&str>, hold: bool) -> Result<SpinResult, SlotError>;
    fn close(&self);
}

/// Formats minor units of a session currency with its symbol: 200 cents USD -> "$2.00".
/// Fraction digits past the second show only when they aren't zero (Stake Engine's
/// six-decimal amounts: 35_000 -> "$0.035", 100_000 -> "$0.10").
pub fn fmt_money(v: i64, currency: &str, decimals: u32) -> String {
    let scale = 10i64.pow(decimals);
    let abs = v.unsigned_abs() as i64;
    let int = crate::money::group_thousands(&(abs / scale).to_string());
    let body = if decimals > 0 {
        let frac = format!("{:0width$}", abs % scale, width = decimals as usize);
        let keep = frac.trim_end_matches('0').len().max(2.min(decimals as usize));
        format!("{int}.{}", &frac[..keep])
    } else {
        int
    };
    let sign = if v < 0 { "-" } else { "" };
    let _ = Units::MAX;
    match (crate::money::fiat_sign(&currency.to_ascii_lowercase()), currency.to_ascii_uppercase().as_str()) {
        (Some(s), _) => format!("{sign}{s}{body}"),
        (None, "FUN" | "XXX") => format!("{sign}FUN {body}"),
        (None, c) => format!("{sign}{body} {c}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn money() {
        assert_eq!(fmt_money(200, "USD", 2), "$2.00");
        assert_eq!(fmt_money(-5, "EUR", 2), "-€0.05");
        assert_eq!(fmt_money(100_000, "USD", 6), "$0.10");
        assert_eq!(fmt_money(35_000, "USD", 6), "$0.035");
        assert_eq!(fmt_money(1_234_567_891, "USD", 6), "$1,234.567891");
        assert_eq!(fmt_money(1_000, "JPY", 0), "¥1,000");
        assert_eq!(fmt_money(1_000, "PLN", 0), "1,000 PLN");
        assert_eq!(fmt_money(15, "XYZ", 1), "1.5 XYZ");
    }
}
