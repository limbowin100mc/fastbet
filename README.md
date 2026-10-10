# FastBet for chips.gg Limbo

A Chrome extension that places Limbo bets on chips.gg straight away. It sends `private.betLimbo` over the chips.gg WebSocket API itself, so there is no animation and none of the site's built-in delays (even the site's turbo autobet waits about 260 ms between bets).

## Install / update

1. Open `chrome://extensions`
2. Turn on **Developer mode** (top right)
3. Click **Load unpacked** and pick this `fast bet` folder (if it's already loaded, press the ⟳ reload button on its card instead)
4. Log in on https://chips.gg and open Limbo

The toolbar icon shows or hides everything. On any other site, it opens Limbo.

## Windows

There are three separate windows. Each one can be dragged by its header, minimized or closed, and its position is remembered. Live and Stats can also be resized from the bottom-right corner. The **⫴** button in the FastBet header puts all three back into the default layout: controls on the left, Live in the middle, Stats on the right.

**FastBet (controls)**: balance, **Manual / Automatic** tabs, and settings grouped into sections:

| Section | What's in it |
|---|---|
| General | Currency (**Site** follows the currency picked on chips.gg), COIN/USD unit, bet size with ½ and 2×, target with quick buttons. Shows win chance and profit on a win. |
| On win / on loss | *Reset to base bet*, or *Change bet by %* (on loss +100% doubles the bet, i.e. martingale) |
| Bets & speed | Number of bets (0 = no limit) and a delay in ms between bets |
| Stop conditions | Stop profit, stop loss, max bet, **Stop on hit** (stops the first time a result reaches the hit multiplier; leave it empty to use your target), optional sound on hit |
| Auto seed change | Switch it on and type after how many bets the seed should change. Each change sets a new random client seed, which resets the nonce to 0. There's also a **Change seed now** button, and it shows the current client seed, nonce, and how many bets until the next change. |
| Hotkeys | `Space` places one bet, `P` pauses / resumes auto-bet, `Esc` stops auto-bet. Ignored while you type in a text field. |

**Live**: the latest result shown big (green for a win, red for a loss, gold for a hit), a strip of recent results, and a bet table (#, nonce, time, target, result, bet, payout, profit). Seed changes appear as their own rows in the table. The header shows session time and bet count.

Top right of the Live window:
- **Stop-on-hit chip** (target icon + `≥ 10.00×`): shows the multiplier that counts as a hit (your hit multiplier, or your target if that field is empty). Gold means stop on hit is on, crossed out means it's off. Click it to switch.
- **Pause / resume button**: pauses auto-bet without ending the run, so the bet count, profit and current bet size carry on when you resume. While paused, the result dims and shows PAUSED. Same as the `P` key.

**Stats**: bet speed (per second and per minute), current and best/worst streaks, profit, wins/losses, wagered, RTP, a live profit chart, max drawdown, highest and lowest profit, highest bet/win/loss, top multipliers, and the best win and worst loss streaks.

## How it works

- `fastbet.js` runs inside the chips.gg page. It opens its own socket to `wss://api.chips.gg/prod/socket` and logs in with the token the site already stores (`localStorage.tokenid`) by calling `auth.authenticate`.
- A bet sends `["private", id, "betLimbo", [{ currency, amount, autoCashout }]]`. `amount` is an integer string in the currency's smallest unit, exactly like the site sends it (0.001 BTC = `"100000"`). All amounts are handled with BigInt, so 18-decimal coins are exact.
- chips.gg only accepts one bet per account at a time. Anything sent while a bet is still open gets "Please wait and try again later". So all bets and seed changes run one after another, as fast as your ping allows. If the server still says "please wait" (for example because you're also betting on the site), FastBet waits a moment and retries the same bet (up to 8 times). The Stats window counts these retries.
- Pressing `Space` or BET several times quickly queues up to 10 bets, which then run one by one.
- Seed change calls `private.rotateProvableSeed({ clientSeed })` and runs between bets, so every bet lands on the seed you expect.
- The site's own Limbo screen won't animate these bets. Your balance and *My Bets* still update.

## Debugging

In DevTools on chips.gg:

```js
__FASTBET__.net.status        // "ready" when logged in
__FASTBET__.seed.state        // current clientSeed / nonce
__FASTBET__.history[0]        // last bet
__FASTBET__.arrange()         // reset window layout
```

Failed bets are logged as `[FastBet] … failed` warnings.
