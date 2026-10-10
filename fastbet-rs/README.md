# FastBet (desktop)

A native desktop app that places Limbo bets straight over the casino's websocket API. No browser, no animations, none of the site's built-in delays. This is the Rust rewrite of the `FastBet for chips.gg Limbo` Chrome extension in the parent folder, built so more casinos can be added later.

- **chips.gg** is the first supported casino.
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

1. **Pick your account**: one row per casino with the username last seen. Click it.
2. **Account**: avatar, **Settings** and **Login ›**. Settings holds the **API key**: on chips.gg open *Settings → API Keys → Create API Key*, copy the key (the site shows it only once), paste it and press Save. The same kind of key other betting bots use; it keeps working until you delete it on the site. No key? *log in in a window instead* opens a login window (Microsoft Edge WebView2, part of Windows 11); the app picks up the session token the moment you are in and closes the window by itself.
3. **Login ›** runs the connection check (reaching the casino, checking credentials) and drops you in the **lobby**: left rail, the wallet pill on top, the Limbo tile, and the session stats on the right. The wallet pill shows one balance, in fiat or crypto; click it for the coin list with a search box, **Display in fiat** and **Hide empty balances**. Picking a coin is what you bet with.
4. **Limbo**: title, session timer and bet count on the left; **Manual Bet / Automatic** tabs in the middle; **Start** on the right. While a run is going, Start becomes **Stop on next win / Pause / Stop**. Settings live in the left panel (General open, the rest as collapsible rows), the result and bet table in the middle.

The avatar at the bottom of the rail takes you back to the account list. The key is stored in your settings file (see below).

Under the hood the login window is a second `fastbet.exe` process (`fastbet --login chipsgg`), because the webview needs its own event loop. chips.gg keeps the same `tokenid` before and after login (a guest token gets upgraded in place), so the window watches the site's own websocket for the signs of a real login (private-channel events, a successful `auth.login` reply) and reports `{token, loggedIn}` on stdout. The main app only verifies the token once `loggedIn` is true, so it never touches the guest token while you are still logging in. Press `F12` in the login window for devtools if something looks off.

## Slots

The **Slots** entry in the rail opens the chips.gg slot catalog: a search box, the Hacksaw Gaming games first (marked **Tested**), then every video slot (**Untested** for providers without a driver yet). Click a tested game to open its page:

- **Real Play / Demo** tabs. Demo needs no login and uses the provider's demo wallet; Real Play launches the game through your chips.gg account and bets your wallet.
- **Bet size** lists exactly the bet levels the game offers; **Feature spin** lists the game's bonus buys with their cost (×2, ×50, ×100 …).
- **Stop on multiplier ≥ N** pauses the moment a round pays N × the stake. Press **Resume** to carry on, or **Stop**.
- **No. of spins** and **Stop on profit / loss** work like for Limbo. **Stop on next win / Pause / Stop** appear while running.
- The table shows every round (feature, bet, multiplier, payout, balance); the Session panel on the right keeps the slot stats.

How it works: the app launches the game session through chips.gg (demo or real), follows the launcher to the game, then talks to the slot provider's game server directly, skipping the game client and all its animations. Providers sit behind Cloudflare or similar bot checks that reject plain HTTP clients, so the calls go through a hidden WebView2 "browser bridge" (`fastbet --bridge <url>`, a child process) parked on the provider's origin.

| Provider | Status | Notes |
|---|---|---|
| Hacksaw Gaming | Tested | ~60 ms per round. Bonus buys supported. Wins need a round-close message, handled. |
| BGaming | Tested | ~90 ms per round. Free-spin rounds are followed to the end. No bonus buys yet. |
| AvatarUX | Tested | ~150 ms per round plus a round-close call. Ante and bonus-buy modes exposed as feature spins. |
| Nolimit City | Not yet | Launches through Evolution's hub with Akamai bot checks, encrypted websocket messages and LZW-compressed replies. Needs its cipher broken first. |
| Pragmatic Play | Blocked | chips.gg refuses both demo and real launches for this region. |
| Others | Untested | Each needs its protocol captured (`fastbet --capture <launch url>` records every request a game makes) and a driver in `src/slots/`. |

Adding a provider means a new driver in `src/slots/` implementing `SlotSession` (open session, list bet levels and features, spin) and one line in `Provider::for_producer`.

## Limbo settings

| Section | What's in it |
|---|---|
| General | Bet size with ½ and ×2, COIN/USD unit, target with quick buttons. Shows win chance and profit on a win. The currency is picked in the balance pill on top (**Site** follows the currency selected on chips.gg). |
| Increase on win / loss | *Reset to base*, or *Change by %* (+100% after a loss doubles the bet, i.e. martingale). Automatic mode only. |
| No. of bets, delay & async | Number of bets (0 = no limit), a delay in ms between bets, and **Async mode**: how many bets may be in flight at once (1–8). Automatic mode only. |

### Why async mode

Every bet is one round trip to the chips.gg origin, which is ~185 ms from Europe even though the Cloudflare edge is next door. Sequential betting therefore caps out near 5 bets/s. The server answers pipelined calls in parallel, so with 3 bets in flight you get ~15/s, with 4 ~20/s. Rules:

- Only for a **flat stake**. With *Change by %* the next stake depends on the previous result, so bets stay one at a time whatever the setting.
- Stop conditions (profit, loss, hit, count) are checked as results come back, so a few extra bets may complete after the one that triggers a stop.
- Seed changes wait until the pipeline has drained, then run alone, so nonces stay in order.
- If the server starts answering "Please wait and try again later", the **Busy retries** counter in Session climbs: lower the number.
| Stop on profit / loss / hit | Stop profit, stop loss, max bet, **Stop on hit ≥** (stops the first time a result reaches the multiplier; leave it empty to use your target). Automatic mode only. |
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
  engine/mod.rs      bet lane, busy retries, seed rotation, autobet loop
  ui/                screens (accounts, lobby, limbo), controls, stats, theme, widgets, sound
```

- The **engine** runs on its own tokio runtime thread. The UI sends it `Command`s and receives `Event`s; nothing in the UI ever blocks on the network.
- chips.gg accepts one bet per account at a time and answers anything sent while a bet is open with "Please wait and try again later". All bets and seed changes therefore go through a single **lane**, in order, as fast as your ping allows. If the server still says it's busy, the bet is retried up to 8 times with a short backoff.
- Pressing `Space` or **BET** several times quickly queues up to 10 bets.
- A seed change calls `rotateProvableSeed({ clientSeed })` inside the lane, so every bet lands on the seed you expect.
- Bet amounts are sent as integer strings in the currency's smallest unit, exactly like the site does.

### Adding a casino

1. Add a variant to `CasinoId` in `src/casino/mod.rs` (name, game/login URL, token-watch script, target limits).
2. Add a module that implements the `Casino` trait: `bet_limbo`, `rotate_seed`, `provable_state`, `set_token`, `shutdown`, and emits `CasinoEvent::{Status, Market, Provable}`.
3. Wire it up in `casino::connect`.

Nothing in the engine or UI needs to change.

## Settings file

`%APPDATA%\fastbet\FastBet\config\settings.json` on Windows (the platform config dir elsewhere). Contains your settings and the login token, so keep it private.

## Tests

```
cargo test
```

covers the money math and the autobet engine against a scripted mock casino (martingale, stop profit/loss, hit stop, max bet, bet limit, seed rotation, busy retries, pause/resume).
