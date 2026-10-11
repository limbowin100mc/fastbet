# FastBet (desktop)

A native desktop app that places Limbo bets straight over the casino's websocket API. No browser, no animations, none of the site's built-in delays. This is the Rust rewrite of the `FastBet for chips.gg Limbo` Chrome extension in the parent folder, built so more casinos can be added later.

- **chips.gg**: Limbo and slots.
- **Stake**: Limbo, 28 more Stake Originals and Stake Engine slots, through stake.com or any of its 131 mirrors.
- **Shuffle**: Limbo, Dice, Mines, Keno, Plinko, Blitz, Waifu Tower and Chicken, through shuffle.com or any of its 43 mirrors.
- **Goated**: Limbo, Dice and Mines.
- One window, three panes: **controls** on the left, **live** results in the middle, **stats** on the right. The side panes can be dragged wider or narrower.
- Everything money-related is exact integer math in the currency's smallest unit (18-decimal coins included).

## Build & run

Needs a Rust toolchain (`rustup`). Then:

```
cd fastbet-rs
cargo run --release
```

The binary ends up in `target/release/fastbet.exe`. It is self-contained; copy it anywhere.

## Screens

1. **Pick your account**: one row per account (casino logo, casino, the username last seen). A casino can have several accounts. **Add account** lists the casinos; pick one and its settings open. **Remove account**, under an account's settings card, forgets that login on this PC (press it twice).
2. **Account**: avatar, **Settings** and **Login ›**. Settings holds the **API key**: on chips.gg open *Settings → API Keys → Create API Key*, copy the key (the site shows it only once), paste it and press Save. The same kind of key other betting bots use; it keeps working until you delete it on the site. No key? *log in in a window instead* opens a login window (Microsoft Edge WebView2, part of Windows 11); the app picks up the session token the moment you are in and closes the window by itself. For **Stake** the card asks for the **API token** (*Settings → Security → API Tokens* on Stake; *Get API token ↗* opens it) and the **Mirror** to connect through, a searchable list of stake.com and its mirrors. *Casino website ↗* opens the chosen mirror.
3. **Login ›** runs the connection check (reaching the casino, checking credentials) and drops you in the **lobby**: left rail, the wallet pill on top, the Limbo tile, and the session stats on the right. The wallet pill shows one balance, in fiat or crypto; click it for the coin list with a search box, **Display in fiat** and **Hide empty balances**. Picking a coin is what you bet with.
4. **Limbo**: title, session timer and bet count on the left; **Manual Bet / Automatic** tabs in the middle; **Start** on the right. While a run is going, Start becomes **Stop on next win / Pause / Stop**. Settings live in the left panel (General open, the rest as collapsible rows), the result and bet table in the middle.

The casino's logo at the bottom of the rail opens the account menu (username, casino, **Switch account**); Switch account takes you back to the account list. `FASTBET_ACCOUNT_MENU=1` opens it at start, for UI work. The key is stored in your settings file (see below).

Under the hood the login window is a second `fastbet.exe` process (`fastbet --login chipsgg`), because the webview needs its own event loop. chips.gg keeps the same `tokenid` before and after login (a guest token gets upgraded in place), so the window watches the site's own websocket for the signs of a real login (private-channel events, a successful `auth.login` reply) and reports `{token, loggedIn}` on stdout. The main app only verifies the token once `loggedIn` is true, so it never touches the guest token while you are still logging in. Press `F12` in the login window for devtools if something looks off.

## Stake

Stake is reached over its GraphQL API (`/_api/graphql`, the `limboBet` and `rotateSeedPair` mutations), signed in with your API token in the `x-access-token` header. Stake's domains sit behind Cloudflare checks that turn plain HTTP clients away (stake.com answers them with its "blocked" page), so every call runs through the same hidden WebView2 browser bridge the slots use, parked on the chosen mirror (its own browser profile, `webview\stake`). A fresh bridge usually passes the check by itself within a few seconds; if Cloudflare wants a click, the bridge window pops up (*FastBet – security check*, centered and kept on top, with the check started over so it runs where you can see it) and hides again once you're through. If a mirror stays blocked, pick another one in Settings.

**Log in with your browser.** Some connections never get through the check inside the app even though the site opens fine in a normal browser. For those, the Stake settings card has *Log in with your browser* (the connection check offers it too once it has been stuck on Cloudflare for 25 s). It takes your own browser's User Agent and cookies for the mirror, and the app then connects as that browser:

1. In a browser on the same computer, open the mirror picked above the section and wait until the site loads. Being logged in there is best: then the login comes along.
2. *Get User Agent ↗* opens Cloudflare's info page on that mirror (`/cdn-cgi/trace`) in the default browser; open it in the browser the cookies come from and copy everything after `uag=` into **User Agent**.
3. Press F12 → *Network*, reload the page, click the first request and copy the value of `cookie` under *Request Headers* into **Cookies**. Only the `cf_clearance` value works too, and so do rows copied from *Application* → *Cookies*. The card says what it found: your Stake login (`session`), `cf_clearance`, `__cf_bm`.
4. Press **Save**. With a logged-in browser's cookies the API token can stay empty: the `session` cookie is Stake's own website login and works as the token. A token typed in the field wins over it.

How it connects: plain HTTPS, no browser window, carrying the pasted User Agent and cookies (and keeping whatever Cloudflare and Stake set later). Cloudflare lets that through the way it lets your browser through: measured from a home connection, `__cf_bm` with its User Agent turns Stake's 403 "Just a moment" into a 200, and with another User Agent it stays 403. The request headers go out in browser casing (`User-Agent`, `Cookie`); Cloudflare turned away the same request with lowercase header names. If Cloudflare refuses the cookies that way, the app tries them in its WebView2 bridge (profile `webview\stake-cf`) before giving up.

Cloudflare ties the cookies to the browser's User Agent, your IP address and the mirror. *Your IP address* in the card is the one Cloudflare sees from the app; *Compare in browser ↗* shows your browser's (the `ip=` line), and they must match. The cookies are saved for the mirror picked when you press Save; after an IP change (new router lease, VPN) or when they've expired, copy fresh ones. *Remove* forgets them. If Cloudflare refuses them, the connection check says so instead of opening a check window.

On a server (VPS, RDP), Cloudflare often doesn't trust the IP address itself. If the check also loops in a normal browser on that server, no cookie can be had there and the app can't get through either; it works from a home connection.

- Balances come from the account and are updated locally after every bet; while no bet is running they are re-read every 10 s. Stake keeps the selected coin in the browser, not the account, so the app starts on your biggest balance; pick another in the balance pill.
- Bet amounts go out as floats with 8 decimals, like the site sends them. Each bet carries a random 21-character `identifier`, and the reply's nonce is shown in the table.
- Stake's "slow down" replies (parallel-bet or rate-limit errors, HTTP 429) count as busy and are retried like on chips.gg.

## Shuffle

Shuffle has no API keys, so the account is linked the way Antebot links it: **Open shuffle.com to log in** opens the site in the login window (on the mirror picked above it), you log in as usual (captcha, 2FA, Google), and the window closes by itself. The site keeps its session in localStorage (`persist:root` → `auth`): a short-lived access token and the refresh token that renews it. The window hands both to the app; your password never reaches it. **Clear cookies** logs that account's login window out, to log in to another account.

- API: GraphQL at `https://<mirror>/main-api/graphql/api/graphql`, `Authorization: Bearer <access token>`, plain HTTPS (the browser bridge only if Cloudflare ever turns the app away). Every request carries an `X-Correlation-Id` built the way the site builds it; bets are refused without one.
- When the access token runs out, the app trades the refresh token for a new pair (as the site does) and saves it at once: the old refresh token stops working the moment it's used.
- Originals reuse Stake's boards (the rules and payouts are the same; Waifu Tower plays like Dragon Tower, Plinko has no Expert risk). Blitz is Shuffle's own: cards come from an endless deck and it pays when every one of them is different.
- Bets of 0 are allowed.

## Goated

**Authentication method**, like Antebot: **Automatic** opens goated.com in the login window; once you're in, the window takes the access token from localStorage (`g3_token`) and the `refresh_token` cookie (HttpOnly, read from the window's own browser) and closes. **Manual** takes the two values pasted from your browser (F12 → Console: `localStorage.getItem('g3_token')`; F12 → Application → Cookies → `refresh_token`).

- API: REST at `https://apis.goated.com`, `Authorization: Bearer <access token>`; the access token lives a day and is renewed an hour before it runs out with the refresh cookie (`/user/auth/token/refresh`).
- Limbo, Dice (win chance 0.1%–98%: Goated pays at most 990× on Dice) and Mines (manual with cash out, or automatic with tiles picked up front). Bets of 0 are allowed.

## Slots

The **Slots** entry in the rail opens the casino's slot catalog with a search box.

- **chips.gg**: the Hacksaw Gaming, AvatarUX and BGaming games first (marked **Tested**), then every video slot (**Untested** for providers without a driver yet).
- **Stake**: only Stake Engine games (Stake's own platform, games by many small studios), most played this week first. Every game in the list is playable, so there are no Tested/Untested badges; the tile shows the house edge and the game page shows the studio.

Click a game to open its page:

- **Real Play / Demo** tabs. Demo needs no login and uses the provider's demo wallet; Real Play launches the game through your account and bets your wallet. Stake Engine games play in USD: Stake converts each bet from the coin selected in the balance pill, and the slot balance shown is that coin's balance in USD.
- **Bet size** lists exactly the bet levels the game offers; **Feature spin** lists the game's bonus buys with their cost (×2, ×50, ×100 …).
- **Stop on multiplier ≥ N** pauses the moment a round pays N × the stake. Press **Resume** to carry on, or **Stop**.
- **No. of spins** and **Stop on profit / loss** work like for Limbo. **Stop on next win / Pause / Stop** appear while running.
- The table shows every round (feature, bet, multiplier, payout, balance); the Session panel on the right keeps the slot stats.

How it works: the app launches the game session through the casino (demo or real), follows the launcher to the game, then talks to the slot provider's game server directly, skipping the game client and all its animations. Providers sit behind Cloudflare or similar bot checks that reject plain HTTP clients, so the calls go through a hidden WebView2 "browser bridge" (`fastbet --bridge <url>`, a child process) parked on the provider's origin.

On Stake, `startThirdPartySession(slug, source: <coin>, target: usd)` (or `startThirdPartyDemoSession(slug)`) returns the game URL, which carries the session id and the game server (`rgs_url`). The app never loads that game; it plays Stake Engine's public RGS protocol on the server instead: `/wallet/authenticate` (bet levels and modes), `/wallet/play` (one call returns the whole round, free spins included) and `/wallet/end-round` (settles a winning round; the next play is refused until then).

| Provider | Status | Notes |
|---|---|---|
| Hacksaw Gaming | Tested | ~60 ms per round. Bonus buys supported. Wins need a round-close message, handled. |
| BGaming | Tested | ~90 ms per round. Free-spin rounds are followed to the end. No bonus buys yet. |
| AvatarUX | Tested | ~150 ms per round plus a round-close call. Ante and bonus-buy modes exposed as feature spins. |
| Stake Engine (Stake) | Tested on demo | ~150 ms per call; winning rounds need an end-round call. Every mode the game has (ante, bonus buys …) is a feature spin. The game server's Cloudflare drops calls that come too fast (seen from one PC: a short burst, then about 1–2 calls a second), so the app spaces calls out, widens the spacing whenever a call is refused, pauses a few seconds and retries, and narrows it again slowly. |
| Nolimit City | Not yet | Launches through Evolution's hub with Akamai bot checks, encrypted websocket messages and LZW-compressed replies. Needs its cipher broken first. |
| Pragmatic Play | Blocked | chips.gg refuses both demo and real launches for this region. |
| Others | Untested | Each needs its protocol captured (`fastbet --capture <launch url>` records every request a game makes) and a driver in `src/slots/`. |

Adding a provider means a new driver in `src/slots/` implementing `SlotSession` (open session, list bet levels and features, spin), one line in `Provider::for_producer`, and the producer in `CasinoId::slot_producers` for each casino that offers it.

## Originals

On Stake the lobby lists every Stake Original with the site's own artwork; Limbo opens its usual page, the others a page of their own, shaped like Limbo's: bet size on the left with the game's settings under it, the game's board in the middle, its rounds below, the session stats on the right. Manual / Automatic, the strategy (change by % on win / loss), number of bets, delay, async, stop on profit / loss / max bet / hit, force restart and the seed changes work as on Limbo. A bet of 0 is allowed (Stake plays it for free).

| Game | On the board | Auto |
|---|---|---|
| Dice | Drag the target on the 0–100 track; roll over / under; quick multipliers | ✓ |
| Primedice | One range, two ranges or outside a range | ✓ |
| Keno | Pick 1–10 of 40 tiles, risk, auto pick, clear, patterns; the payout strip for the picks | ✓ |
| Mines | Reveal tiles one by one, random tile, cash out. Auto: click the tiles every bet reveals | ✓ |
| Plinko | Risk (incl. Expert) and 8–16 rows; the ball's path and the buckets | ✓ |
| Wheel | Risk and 10–50 segments; the wheel spins to the result | ✓ |
| Dragon Tower | Climb row by row, cash out. Auto: click a path, one tile per row | ✓ |
| Pump, Chicken, Snakes | Pump / cross / roll and cash out. Auto: pumps, lanes or rolls per bet | ✓ |
| Moles | Whack a hole each round. Auto: plan the holes up front | ✓ |
| Flip, Rock Paper Scissors | Call each flip / throw and cash out. Auto: the calls up front | ✓ |
| HiLo | Higher / lower (or same at A and K), skip, cash out; pick the start card | – |
| Blackjack | Hit, stand, double, split, insurance | – |
| Video Poker | Click cards to hold, then draw; the paytable | – |
| Baccarat | Back the player, banker and / or tie | ✓ |
| Roulette | Click the table to stack chips of the bet size, right-click removes one | ✓ |
| Diamonds, Cases, Darts, Tarot, Packs, Bars, Drill | One request per round; the board shows the result and the game's table | ✓ |
| Scarab Spin, Tome of Life, Blue Samurai | Stake's own slots; bonus spins play out by themselves | ✓ |

Rounds left open (on the site, or when the app closed mid-round) are picked up when the game's page opens. Crash and Slide (live multiplayer rounds) are not in yet.

How it works: the site posts its originals to `/_api/casino/<game>/<action>` (`dice/roll`, `mines/bet`, `mines/next`, `mines/cashout` …), the REST twins of its GraphQL mutations; Flip only exists as GraphQL. The app does the same through the Stake bridge, with your API token. Payout tables for Keno, Wheel and Plinko come from `info { kenoPayouts wheelPayouts plinkoPayouts }` (a copy ships in `assets/stake-originals.json`); the other boards' tables were taken from the site's code. The casino settles every round; the boards only show what it answered. Baccarat and Roulette can't be tried with a 0 stake (Stake refuses an empty table), so they were built from the site's code and not yet played for real.

## Limbo settings

| Section | What's in it |
|---|---|
| General | Bet size with ½ and ×2, COIN/USD unit, target with quick buttons. On Stake a bet of 0 is placed too: a real round that pays nothing. Shows win chance and profit on a win. The currency is picked in the balance pill on top (**Site** follows the currency selected on chips.gg). |
| Increase on win / loss | *Reset to base*, or *Change by %* (+100% after a loss doubles the bet, i.e. martingale). Automatic mode only. |
| No. of bets, delay & async | Number of bets (0 = no limit), a delay in ms between bets, and **Async mode**: how many bets may be in flight at once (1, 2, 3, 4, 6 or 8). Automatic mode only. |

### Why async mode

Every bet is one round trip to the chips.gg origin, which is ~185 ms from Europe even though the Cloudflare edge is next door. Sequential betting therefore caps out near 5 bets/s. The server answers pipelined calls in parallel, so with 3 bets in flight you get ~15/s, with 4 ~20/s. Rules:

- Only for a **flat stake**. With *Change by %* the next stake depends on the previous result, so bets stay one at a time whatever the setting.
- Stop conditions (profit, loss, hit, count) are checked as results come back, so a few extra bets may complete after the one that triggers a stop.
- Seed changes wait until the pipeline has drained, then run alone, so nonces stay in order.
- If the server starts answering "Please wait and try again later", the **Busy retries** counter in Session climbs: lower the number.
| Stop on profit / loss / hit | Stop profit, stop loss, max bet, **Stop on hit ≥** (stops the first time a result reaches the multiplier; leave it empty to use your target). Automatic mode only. |
| Force restart betting | **Force restart betting always**: when a run ends by itself (stop profit or loss, a hit, the number of bets, max bet, an error), it starts again at once from the base bet with its counters at zero. When the restarted run fails before a single bet settles (the casino is down, the balance is empty), it waits 1, 2, 4, then 8 s between tries. **Force restart on errors**: an error doesn't end the run; the failed bet is placed again at once with the same bet size and counters (a martingale keeps its step), and errors in a row wait 1, 2, 4, then 8 s. With both on, errors go this way. **Stop**, `Esc` and **Stop on next win** always end the run, and Stop also ends a wait. A round an original leaves open on the board isn't restarted: it needs you. The header shows *Restarted ×N* and *Errors ×N* (hover for the last reason). Both can be switched while a run is going. Automatic mode only. |
| Auto seed changes | Rotate the client seed every N bets (a new random client seed, nonce back to 0). *change seed now* does it immediately. Shows the current client seed and nonce. |
| Sound & hotkeys | Sound on hit; `Space` places one bet, `P` pauses / resumes, `Esc` stops. Ignored while you type in a text field. `Ctrl+R` always works: it resets the whole session (stops a run, clears history and stats, changes the client seed). |

The stage shows the latest result big (green win, red loss, gold hit) with a strip of recent results, and the table lists #, nonce, time, target, result, bet size, multiplier, payout and profit. Seed changes appear as their own rows. **Session** on the right shows bet speed, streaks, profit, wins/losses, wagered, RTP, max drawdown, the profit chart, highest profit/loss/win/bet, highest multipliers and the best/worst streaks; ↺ resets it.

## How it works

```
src/
  main.rs            window + startup (+ `--login` child mode)
  login.rs           embedded WebView2 login window and the parent-side session
  money.rs           integer amounts, parsing, formatting
  settings.rs        persisted settings (JSON)
  casino/
    mod.rs           the Casino trait + shared types (Market, BetReply, ProvableState …)
    chipsgg.rs       chips.gg websocket client
    stake.rs         Stake GraphQL client (through the browser bridge) + the mirror list, Stake Engine catalog + launch, Stake Originals over REST
  originals/         the originals: games, rounds, payout math and tables
  cloudflare.rs      cf_clearance cookie + User Agent pasted from the user's browser, IP check
  bridge.rs          hidden WebView2 that runs fetch() on a site's origin for us
  engine/mod.rs      bet lane, busy retries, seed rotation, autobet loop
  ui/                screens (accounts, lobby, limbo), controls, stats, theme, widgets, sound
  ui/original.rs     an original's page; ui/originals/ holds one board per game
```

- The **engine** runs on its own tokio runtime thread. The UI sends it `Command`s and receives `Event`s; nothing in the UI ever blocks on the network.
- chips.gg accepts one bet per account at a time and answers anything sent while a bet is open with "Please wait and try again later". All bets and seed changes therefore go through a single **lane**, in order, as fast as your ping allows. If the server still says it's busy, the bet is retried up to 8 times with a short backoff.
- Pressing `Space` or **BET** several times quickly queues up to 10 bets.
- A seed change calls `rotateProvableSeed({ clientSeed })` inside the lane, so every bet lands on the seed you expect.
- Bet amounts are sent as integer strings in the currency's smallest unit, exactly like the site does.

### Adding a casino

1. Add a variant to `CasinoId` in `src/casino/mod.rs` (name, game/login URL, token-watch script, target limits, mirrors, whether it has slots and a login window).
2. Add a module that implements the `Casino` trait: `bet_limbo`, `rotate_seed`, `provable_state`, `set_token`, `shutdown` (and `set_mirror` if it has mirrors, `set_clearance` if it sits behind Cloudflare; mark it in `CasinoId::behind_cloudflare`), and emits `CasinoEvent::{Status, Market, Provable}`.
3. Wire it up in `casino::connect`, and add its logo: a white, single-colour SVG in `assets/casinos/` hooked up in `ui::logos::svg`. Keep these to plain path data (no scripts or external references); they are drawn in the account list and on the account page.

Nothing in the engine or UI needs to change.

## Updates

**Check for updates** is in the gear menu on the welcome screen and in the account menu (the casino logo at the bottom of the rail). It asks GitHub for the latest release of `limbowin100mc/fastbet` and compares it with the running version.

- **Windows**: *Download & install* downloads the `-setup.exe`, checks it against the SHA-256 digest GitHub publishes for every release file, starts it and closes FastBet so the installer can replace it.
- **Linux AppImage**: the new AppImage replaces the running one (same checks) and FastBet restarts on it.
- **Linux .deb**: *Download .deb* opens the package in the browser; installing it needs root.

The button is disabled while an auto run or slot autoplay is going. To ship an update, push a `v*` tag (see `.github/workflows/release.yml`); the release it creates is what the app finds.

The AppImage is built on Ubuntu 22.04 so it runs on older distros, then repacked by `packaging/appimage-drop-libs.sh` without the bundled `libxkbcommon`: that 2022 copy can't read the keymaps newer Wayland desktops send (KDE on CachyOS/Arch), and winit then crashes on the first key press. The system's own copy is used instead.

For UI work: `FASTBET_UPDATE=1` runs the check at start, `FASTBET_PRETEND_VERSION=2.9.0` makes the app believe it is older, `FASTBET_WELCOME_MENU=1` opens the gear menu. `cargo test -- --ignored` downloads a real release file and checks the digest handling.

## Settings file

`%APPDATA%\fastbet\FastBet\config\settings.json` on Windows (the platform config dir elsewhere). Contains your settings, your accounts (casino, login tokens, username, mirror, your browser's User Agent and cookies if you logged in with your browser), so keep it private: a Stake API token, like the `session` cookie or Shuffle's and Goated's tokens, works like a password for your account.

Saving never drops a login the app didn't remove: before every save the file is read again and accounts found there are kept. A file that exists but can't be read leaves the app running without saving; one that can't be parsed is copied to `settings.unreadable-<time>.json` first. The old one-login-per-casino fields are still written, so an older FastBet can read the file.

For UI work, `FASTBET_MIRRORS=<search>` opens the account settings with the mirror list already open (`1` for no search), and `FASTBET_CLOUDFLARE=1` opens them on the Cloudflare cookie section.

## Tests

```
cargo test
```

covers the money math and the autobet engine against a scripted mock casino (martingale, stop profit/loss, hit stop, max bet, bet limit, seed rotation, busy retries, pause/resume, force restart).
