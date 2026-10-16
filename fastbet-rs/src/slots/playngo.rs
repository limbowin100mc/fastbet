//! Play'n GO driver. The casino's launcher (Hub88's on chips.gg and Goated) ends on a page that
//! embeds `https://<cw>.playngonetwork.com/casino/ContainerLauncher?pid&gid&ticket&brand&practice…`.
//! The game talks a line-based text protocol: every request is `d=<seq>\r\n<session>\r\n<line>\r\n`
//! POSTed to the game server (the game client sends the same packets over a SignalR socket, but
//! plain HTTPS takes them too), every reply `d=<first line>\r\n<line>…`, an error `d=111 "<text>"`.
//!   GET  {cw}/games/getgamebygid/<gid>                 -> gameId (numeric), name, gameType
//!   GET  {cw}/configuration/prelaunchconfiguration     -> Server (the game server's URL)
//!   103 <product> "<lang>" <gameId> "<UA>" "<name>" "<channel>"  -> d=103 "<pre-session>"
//!        (product = 10 × pid − 8, as Play'n GO's launcher works it out: chips.gg 4962, Stake 16162)
//!   101 "<ticket>" "" "" "" "" ""   (empty ticket: practice) -> d=103 "<session>", 101 <uid> "<CUR>" …
//!        (the load balancer now and then hands 101 to a server that never saw the 103: start over)
//!   GET  {cw}/configuration/v2?…&currency=<CUR>         -> settings, lines, denom (default coin)
//!   104 <gameId> 0 "<settings>"   -> 54 <n> <coin values in cents…> <x>, 52 <balance cents> …
//!   0
//! A spin is `1 <units> 1 <coin> 1` on a grid slot (units: the game's fixed bet multiplier, which
//! no config names: found by trying a spin in a practice session, see [`grid_units`]) and
//! `1 1 <lines> <coin> 1` on a video slot; it costs units (lines) × coin. A command is
//! `<type> <args…>`, type 1 spin, 2 feature, 4 gamble, 7 free spin (the game client's own names);
//! the server's lines are 1 spin, 2 feature, 3 spin end, 6 collect, 52 balance. A feature line is
//! `2 <step>`: 0 start, 1 input (the player picks), 2 award (`2 2 <kind> <n>`, kind 0 coins,
//! 1 free spins, 6 feature…), 3 select, 4 data, 5 end (`2 5 1`: wants a collect), 6 collect.
//! So a round goes: a reply with feature lines is stepped with `2 6` (collect) until a spin end
//! (`3 …`) has no free spins left (its fifth field), a pick is answered `2 3 <option>`, and the
//! win is taken with `4 0` (gamble 0: no gamble); it is over at `6 <win cents>`, `52 <balance>`.

use std::collections::HashMap;
use std::sync::OnceLock;

use async_trait::async_trait;
use serde_json::Value;
use tokio::sync::Mutex;

use super::*;

/// What a grid slot's spin line carries as its bet multiplier, most common first.
const UNIT_GUESSES: [i64; 12] = [10, 20, 25, 30, 15, 40, 50, 5, 1, 9, 18, 100];
/// Grid slots whose units are already known (by gid).
const KNOWN_UNITS: &[(&str, i64)] = &[("tomeofmadness", 10), ("reactoonz", 20), ("riseofolympus", 20), ("moonprincess", 20), ("gemix", 10), ("sweetalchemy", 10)];
/// Steps a round may take before we give up on it.
const MAX_STEPS: usize = 400;
const NL: &str = "\r\n";

/// The game's place in the launch URL.
#[derive(Clone, Debug)]
struct Launch {
    /// `https://<cw>.playngonetwork.com`
    cw: String,
    pid: String,
    gid: String,
    ticket: String,
    brand: String,
    practice: bool,
    lang: String,
    channel: String,
}

impl Launch {
    fn from_url(url: &str) -> Option<Launch> {
        let map = hacksaw::query_map(url);
        let get = |k: &str| map.get(k).cloned().unwrap_or_default();
        let cw = url.split('/').take(3).collect::<Vec<_>>().join("/");
        let gid = get("gid");
        if !cw.starts_with("http") || gid.is_empty() {
            return None;
        }
        Some(Launch {
            cw,
            pid: get("pid"),
            gid,
            ticket: get("ticket"),
            brand: get("brand"),
            practice: get("practice") != "0",
            lang: map.get("lang").cloned().filter(|l| !l.is_empty()).unwrap_or_else(|| "en_GB".into()),
            channel: map.get("channel").cloned().filter(|c| !c.is_empty()).unwrap_or_else(|| "desktop".into()),
        })
    }

    /// The `ContainerLauncher` URL in the launcher's last page (or the page's own URL).
    fn find(url: &str, body: &str) -> Option<Launch> {
        if url.contains("/casino/ContainerLauncher") {
            return Launch::from_url(url);
        }
        let at = body.find("/casino/ContainerLauncher?")?;
        let start = body[..at].rfind("http")?;
        let end = body[at..].find(['"', '\'', ' ', '<', '>', '\\']).map_or(body.len(), |e| at + e);
        Launch::from_url(&body[start..end].replace("&amp;", "&"))
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Family {
    /// `1 <units> 1 <coin> 1`
    Grid(i64),
    /// `1 1 <lines> <coin> 1`
    Video(i64),
}

impl Family {
    fn mult(self) -> i64 {
        match self {
            Family::Grid(u) | Family::Video(u) => u,
        }
    }
    fn line(self, coin: i64) -> String {
        match self {
            Family::Grid(u) => format!("1 {u} 1 {coin} 1"),
            Family::Video(l) => format!("1 1 {l} {coin} 1"),
        }
    }
}

/// A logged-in game: the packets go to `server` under `session`.
struct Conn {
    http: reqwest::Client,
    /// The game client's origin, `{cw}`.
    origin: String,
    server: String,
    session: String,
    seq: u64,
}

impl Conn {
    async fn post(&mut self, line: &str) -> Result<String, SlotError> {
        self.seq += 1;
        let body = format!("d={}{NL}{}{NL}{line}{NL}", self.seq, self.session);
        let res = self
            .http
            .post(&self.server)
            .header("Content-Type", "text/plain")
            .header("Origin", &self.origin)
            .header("Referer", format!("{}/", self.origin))
            .body(body)
            .timeout(std::time::Duration::from_secs(20))
            .send()
            .await
            .map_err(|e| SlotError::Bridge(format!("Play'n GO: {e}")))?;
        let status = res.status().as_u16();
        let text = res.text().await.unwrap_or_default();
        if status == 429 {
            return Err(SlotError::RateLimited);
        }
        if status != 200 {
            return Err(SlotError::Provider(format!("Play'n GO HTTP {status}: {}", text.chars().take(160).collect::<String>())));
        }
        Ok(text)
    }

    /// Like [`Conn::post`], an error reply (`d=111 "<text>"`) as an error.
    async fn ask(&mut self, line: &str) -> Result<String, SlotError> {
        let r = self.post(line).await?;
        match error_of(&r) {
            Some(e) => Err(e),
            None => Ok(r),
        }
    }
}

/// A `111 "<text>"` line (the reply's first, or after the echo of a step) -> the error it means.
fn error_of(reply: &str) -> Option<SlotError> {
    let line = lines(reply).find(|l| *l == "111" || l.starts_with("111 "))?;
    let msg = line[3..].trim().trim_matches('"').to_string();
    let low = msg.to_ascii_lowercase();
    Some(if ["money", "balance", "fund", "insufficient"].iter().any(|w| low.contains(w)) {
        SlotError::Balance
    } else if ["session", "logged", "failed to process packets", "expired", "ticket"].iter().any(|w| low.contains(w)) {
        SlotError::SessionExpired
    } else {
        SlotError::Provider(format!("Play'n GO: {}", if msg.is_empty() { "error".into() } else { msg }))
    })
}

/// The reply's lines without the `d=` (the first one is the first line).
fn lines(reply: &str) -> impl Iterator<Item = &str> {
    reply.split(NL).map(|l| l.strip_prefix("d=").unwrap_or(l)).filter(|l| !l.is_empty())
}

/// The numbers of the line with code `code` (`52 2500000 0 0` -> [2500000, 0, 0]).
fn numbers(reply: &str, code: &str) -> Option<Vec<i64>> {
    lines(reply).filter_map(|l| l.split_once(' ').filter(|(c, _)| *c == code).map(|x| x.1)).last().map(|rest| rest.split(' ').filter_map(|n| n.parse().ok()).collect())
}

fn quoted(reply: &str) -> Option<&str> {
    reply.split('"').nth(1)
}

// these three look at the lines as they come, so the echo of the command (`d=2 6`) isn't a step

fn closed(reply: &str) -> bool {
    reply.split(NL).any(|l| l.starts_with("52 "))
}

fn steps(reply: &str) -> bool {
    reply.split(NL).any(|l| l.starts_with("2 "))
}

/// The last `3 …` line's fifth field: 0 the round's spins are done, 1 more (free spins) follow.
fn more_spins(reply: &str) -> Option<bool> {
    reply.split(NL).filter(|l| l.starts_with("3 ")).last().and_then(|l| l.split(' ').nth(4)).map(|f| f != "0")
}

/// A feature waits for the player to pick (`2 1`).
fn wants_pick(reply: &str) -> bool {
    reply.split(NL).any(|l| l == "2 1" || l.starts_with("2 1 "))
}

/// Free spins awarded (`2 2 1 <n>`); one or two are a respin, not what anyone calls free spins.
fn awards_free_spins(reply: &str) -> bool {
    reply.split(NL).any(|l| l.strip_prefix("2 2 1 ").and_then(|n| n.split(' ').next()?.parse::<i64>().ok()).is_some_and(|n| n >= 3))
}

/// The product id a session asks for: the launcher's `10 * pid - 8`.
fn product_id(pid: &str) -> i64 {
    pid.trim().parse::<i64>().map(|p| 10 * p - 8).unwrap_or(4962)
}

fn urlencode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'!' | b'*' | b'\'' | b'(' | b')' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

/// The game, from `getgamebygid`.
struct Game {
    id: String,
    name: String,
    video: bool,
}

/// A logged-in game with its 104 reply.
struct Opened {
    conn: Conn,
    currency: String,
    lines: i64,
    denom: i64,
    init: String,
}

async fn get_json(http: &reqwest::Client, url: &str) -> Result<Value, SlotError> {
    let res = http.get(url).timeout(std::time::Duration::from_secs(20)).send().await.map_err(|e| SlotError::Bridge(format!("Play'n GO: {e}")))?;
    let status = res.status().as_u16();
    let text = res.text().await.unwrap_or_default();
    if status != 200 {
        return Err(SlotError::Provider(format!("Play'n GO HTTP {status}: {}", text.chars().take(160).collect::<String>())));
    }
    serde_json::from_str(&text).map_err(|_| SlotError::Provider(format!("Play'n GO: bad reply {}", text.chars().take(160).collect::<String>())))
}

async fn game_of(http: &reqwest::Client, l: &Launch) -> Result<Game, SlotError> {
    let g = get_json(http, &format!("{}/games/getgamebygid/{}", l.cw, l.gid)).await?;
    let id = match &g["gameId"] {
        Value::Number(n) => n.to_string(),
        Value::String(s) => s.clone(),
        _ => return Err(SlotError::Provider(format!("Play'n GO doesn't know the game {}", l.gid))),
    };
    Ok(Game { id, name: g["name"].as_str().unwrap_or(&l.gid).to_string(), video: g["gameType"].as_str() == Some("videoslot") })
}

/// Logs in to the game (`practice`: with play money, whatever the launch was) and loads it.
async fn login(http: &reqwest::Client, l: &Launch, game: &Game, practice: bool) -> Result<Opened, SlotError> {
    let p = if practice { "1" } else { "0" };
    let pre = get_json(
        http,
        &format!(
            "{}/configuration/prelaunchconfiguration?gameId={}&language={}&productGroup={}&gapTime=&brand={}&ctx=&practice={p}&channel={}",
            l.cw, game.id, l.lang, l.pid, urlencode(&l.brand), l.channel
        ),
    )
    .await?;
    let server = pre["Server"].as_str().filter(|s| s.starts_with("http")).ok_or_else(|| SlotError::Provider("Play'n GO didn't name a game server".into()))?;
    let mut conn = Conn { http: http.clone(), origin: l.cw.clone(), server: server.trim_end_matches('/').to_string(), session: String::new(), seq: 0 };
    let ticket = if practice { "" } else { l.ticket.as_str() };
    let mut reply = String::new();
    for _ in 0..6 {
        conn.seq = 0;
        conn.session = "0".into();
        let hello = conn.ask(&format!("103 {} \"{}\" {} \"{}\" \"{}\" \"{}\"", product_id(&l.pid), l.lang, game.id, urlencode(UA), game.name, l.channel)).await?;
        conn.session = quoted(&hello).unwrap_or_default().to_string();
        reply = conn.post(&format!("101 \"{ticket}\" \"\" \"\" \"\" \"\" \"\"")).await?;
        if reply.starts_with("d=103 \"") {
            break;
        }
        // another server got the login: a new session, unless the reply says why it can't be
        if !reply.contains("Failed to process packets") {
            break;
        }
    }
    if std::env::var_os("FASTBET_PNG_DUMP").is_some() {
        eprintln!("playngo login: {}", reply.replace(NL, " | ").chars().take(400).collect::<String>());
    }
    if !reply.starts_with("d=103 \"") {
        return Err(error_of(&reply).unwrap_or_else(|| SlotError::Provider(format!("Play'n GO login: {}", reply.chars().take(160).collect::<String>()))));
    }
    conn.session = quoted(&reply).unwrap_or_default().to_string();
    let currency = lines(&reply).find(|x| x.starts_with("101 ")).and_then(quoted).filter(|c| !c.is_empty()).unwrap_or(if practice { "DEMO" } else { "USD" }).to_string();
    let r: u128 = rand::random();
    let cfg = get_json(
        http,
        &format!(
            "{}/configuration/v2?pid={}&gameid={}&lang={}&practice={p}&brand={}&ctx=&channel={}&jurisdiction=&platform=megaton&currency={currency}&country=&region=&resourceRoot=&_r={r:032x}",
            l.cw, l.pid, game.id, l.lang, urlencode(&l.brand), l.channel
        ),
    )
    .await?;
    let settings = cfg["settings"].as_str().unwrap_or("").trim_start_matches("&settings=").to_string();
    let num = |v: &Value| v.as_i64().or_else(|| v.as_str().and_then(|s| s.trim().parse().ok())).unwrap_or(0);
    let load = if settings.is_empty() { format!("104 {} 0 ", game.id) } else { format!("104 {} 0 \"{settings}\"", game.id) };
    let init = conn.ask(&load).await?;
    conn.ask("0").await?;
    Ok(Opened { conn, currency, lines: num(&cfg["lines"]), denom: num(&cfg["denom"]), init })
}

/// What one round came to.
struct Round {
    win: i64,
    balance: Option<i64>,
    free_spins: bool,
    held: bool,
    steps: usize,
}

/// Plays the round `last` (the spin's reply) started to its end, or with `hold` up to its free
/// spins or the pick that starts them, which the game in the browser then picks up.
async fn finish(conn: &mut Conn, mut last: String, hold: bool) -> Result<Round, SlotError> {
    let mut stepping = steps(&last);
    // `last` answers a step: its spin end says whether the win is ready to take
    let mut stepped = false;
    let mut free_spins = false;
    let mut n = 0usize;
    while !closed(&last) {
        let pick = wants_pick(&last);
        if pick || awards_free_spins(&last) {
            free_spins = true;
            if hold {
                return Ok(Round { win: 0, balance: None, free_spins, held: true, steps: n });
            }
        }
        n += 1;
        if n > MAX_STEPS {
            return Err(SlotError::Provider("Play'n GO: the round doesn't end".into()));
        }
        // the first option: what's always there (the game's autoplay picks at random)
        let cmd = if pick {
            "2 3 0"
        } else if !stepping || (stepped && more_spins(&last) == Some(false)) {
            "4 0"
        } else {
            "2 6"
        };
        last = conn.ask(cmd).await?;
        stepped = cmd != "4 0";
        stepping = stepping || pick || steps(&last);
    }
    Ok(Round { win: numbers(&last, "6").and_then(|v| v.first().copied()).unwrap_or(0), balance: numbers(&last, "52").and_then(|v| v.first().copied()), free_spins, held: false, steps: n })
}

/// The units of a grid slot: known, saved from an earlier try, or found now by spinning each
/// guess in a practice session until the game takes one (a wrong one fails the game module and
/// ends that session).
async fn grid_units(http: &reqwest::Client, l: &Launch, game: &Game) -> Result<i64, SlotError> {
    if let Some((_, u)) = KNOWN_UNITS.iter().find(|(g, _)| *g == l.gid) {
        return Ok(*u);
    }
    if let Some(u) = saved_units().lock().await.get(&l.gid) {
        return Ok(*u);
    }
    let mut last = String::new();
    for u in UNIT_GUESSES {
        let mut o = login(http, l, game, true).await?;
        let coin = if o.denom > 0 { o.denom } else { 1 };
        match o.conn.ask(&Family::Grid(u).line(coin)).await {
            Ok(first) => {
                let _ = finish(&mut o.conn, first, false).await;
                let mut saved = saved_units().lock().await;
                saved.insert(l.gid.clone(), u);
                save_units(&saved);
                return Ok(u);
            }
            // "Game module failed": not this one
            Err(SlotError::Provider(e)) => last = e,
            Err(e) => return Err(e),
        }
    }
    tracing::warn!("playngo: no units for {}: {last}", l.gid);
    Err(SlotError::Provider(format!("Play'n GO: couldn't find how {} bets", game.name)))
}

fn units_file() -> Option<std::path::PathBuf> {
    directories::ProjectDirs::from("gg", "fastbet", "FastBet").map(|d| d.cache_dir().join("playngo-units.json"))
}

fn saved_units() -> &'static Mutex<HashMap<String, i64>> {
    static UNITS: OnceLock<Mutex<HashMap<String, i64>>> = OnceLock::new();
    UNITS.get_or_init(|| {
        let map = units_file().and_then(|f| std::fs::read_to_string(f).ok()).and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
        Mutex::new(map)
    })
}

fn save_units(map: &HashMap<String, i64>) {
    let Some(f) = units_file() else { return };
    if let Some(dir) = f.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Ok(t) = serde_json::to_string(map) {
        let _ = std::fs::write(f, t);
    }
}

pub struct PlayNGoSession {
    conn: Mutex<Conn>,
    family: Family,
    info: SessionInfo,
}

pub async fn open(launch_url: &str) -> Result<PlayNGoSession, SlotError> {
    let (url, body) = follow_launcher(launch_url).await.map_err(SlotError::Bridge)?;
    let l = Launch::find(&url, &body).ok_or_else(|| SlotError::Provider(format!("Could not read the launch parameters from {url}")))?;
    let http = reqwest::Client::builder().user_agent(UA).build().map_err(|e| SlotError::Bridge(e.to_string()))?;
    let game = game_of(&http, &l).await?;
    let units = if game.video { None } else { Some(grid_units(&http, &l, &game).await?) };
    let mut o = login(&http, &l, &game, l.practice).await?;
    let family = match units {
        Some(u) => Family::Grid(u),
        None if o.lines > 0 => Family::Video(o.lines),
        None => return Err(SlotError::Provider(format!("Play'n GO didn't say how many lines {} has", game.name))),
    };
    // 54 <count> <coin values…> <x>
    let coins: Vec<i64> = numbers(&o.init, "54").map(|v| v.iter().skip(1).take(v.first().copied().unwrap_or(0).max(0) as usize).copied().collect()).unwrap_or_default();
    if coins.is_empty() {
        return Err(SlotError::Provider(format!("Play'n GO didn't list the bets of {}", game.name)));
    }
    let mult = family.mult();
    let bet_levels = coins.iter().map(|c| BetLevel { raw: c.to_string(), minor: c * mult }).collect();
    let default_coin = if coins.contains(&o.denom) { o.denom } else { coins[0] };
    let mut balance = numbers(&o.init, "52").and_then(|v| v.first().copied());
    // a round left open (free spins the browser didn't finish) is played out first
    if steps(&o.init) {
        let init = o.init.clone();
        if let Ok(r) = finish(&mut o.conn, init, false).await {
            balance = r.balance.or(balance);
        }
    }
    let currency = if o.currency == "DEMO" { "FUN".to_string() } else { o.currency.clone() };
    let info = SessionInfo { currency, decimals: 2, bet_levels, default_bet: Some(default_coin * mult), features: vec![], balance };
    Ok(PlayNGoSession { conn: Mutex::new(o.conn), family, info })
}

#[async_trait]
impl SlotSession for PlayNGoSession {
    fn info(&self) -> &SessionInfo {
        &self.info
    }

    async fn spin(&self, bet: i64, _feature: Option<&str>, hold: bool) -> Result<SpinResult, SlotError> {
        let t0 = std::time::Instant::now();
        let mult = self.family.mult();
        let coin = self.info.bet_levels.iter().find(|b| b.minor == bet).map(|b| b.minor / mult).ok_or_else(|| SlotError::Provider("Pick one of the game's bet sizes".into()))?;
        let mut conn = self.conn.lock().await;
        let first = conn.ask(&self.family.line(coin)).await?;
        let r = finish(&mut conn, first, hold).await?;
        Ok(SpinResult {
            round_id: String::new(),
            cost: bet,
            win: r.win,
            multiplier: if bet > 0 { r.win as f64 / bet as f64 } else { 0.0 },
            balance: r.balance,
            pending: r.held,
            free_spins: r.free_spins,
            held: r.held,
            events: r.steps + 1,
            ms: t0.elapsed().as_secs_f64() * 1000.0,
        })
    }

    fn close(&self) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn launch_from_the_launcher_page() {
        let body = r#"<iframe src="https://bsicw.playngonetwork.com/casino/ContainerLauncher?pid=497&amp;origin=https%3A%2F%2Flauncher-as1.fh8labs.com&amp;gid=bookofdead&amp;ticket=F1B0E5&amp;lang=en_GB&amp;channel=desktop&amp;demo=2&amp;brand=goated&amp;lobby=https%3A%2F%2Fgoated.com&amp;practice=0"></iframe>"#;
        let l = Launch::find("https://launcher-as1.fh8labs.com/games/encrypted/playngo/play?payload=x", body).unwrap();
        assert_eq!((l.cw.as_str(), l.pid.as_str(), l.gid.as_str(), l.ticket.as_str(), l.brand.as_str()), ("https://bsicw.playngonetwork.com", "497", "bookofdead", "F1B0E5", "goated"));
        assert!(!l.practice);
        assert_eq!((l.lang.as_str(), l.channel.as_str()), ("en_GB", "desktop"));
    }

    #[test]
    fn replies() {
        let init = "d=104 1\r\n54 20 1 2 3 4 5 7 10 15 20 25 35 50 75 100 200 300 400 500 750 1000 1\r\n52 2500000 0 0\r\n83 0\r\n\r\n";
        assert_eq!(numbers(init, "54").unwrap().len(), 22);
        assert_eq!(numbers(init, "52").unwrap()[0], 2_500_000);
        assert!(closed(init) && !steps(init));
        let step = "d=2 6\r\n2 0 0 7 1 0\r\n2 5 0\r\n3 0 78 1170 1 1\r\n1 20 1 15 8 3\r\n";
        assert!(steps(step) && !closed(step));
        assert_eq!(more_spins(step), Some(true));
        assert_eq!(more_spins("d=2 6\r\n3 0 0 0 0 1\r\n"), Some(false));
        let end = "d=5 1388\r\n6 20820\r\n52 2519965 0 0\r\n\r\n";
        assert_eq!(numbers(end, "6").unwrap()[0], 20_820);
        assert_eq!(numbers(end, "5").unwrap()[0], 1388);
        assert!(matches!(error_of("d=111 \"Game module failed\"\r\n"), Some(SlotError::Provider(e)) if e.contains("Game module failed")));
        assert!(matches!(error_of("d=111 \"Failed to process packets.\"\r\n"), Some(SlotError::SessionExpired)));
        assert!(error_of("d=103 \"abc\"").is_none());
        assert!(matches!(error_of("d=2 6\r\n111 \"Game module failed\"\r\n\r\n"), Some(SlotError::Provider(_))));
        // Book of Dead's trigger: 3 books, 20 coins, 10 free spins, symbol 6, feature over
        let trigger = "d=1 1 10 20 2 6 0\r\n2 0 0 9 3 0\r\n2 2 0 20\r\n2 2 1 10\r\n2 2 6 6\r\n2 5 1\r\n\r\n";
        assert!(awards_free_spins(trigger) && !wants_pick(trigger));
        // a respin isn't free spins
        assert!(!awards_free_spins("d=2 6\r\n2 0 0 7 1 0\r\n2 2 1 1\r\n"));
        // Moon Princess asks which princess
        assert!(wants_pick("d=2 6\r\n2 0 0 2 0 0\r\n2 1\r\n\r\n"));
        assert_eq!(Family::Grid(10).line(15), "1 10 1 15 1");
        assert_eq!((product_id("497"), product_id("1617")), (4962, 16162));
        assert_eq!(Family::Video(10).line(20), "1 1 10 20 1");
    }
}
