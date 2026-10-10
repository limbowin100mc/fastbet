// FastBet for chips.gg Limbo.
// Runs in the page (MAIN world) and opens its own websocket to the chips.gg
// API, authenticated with the same token the site keeps in localStorage.
//
// Wire protocol (@chipsgg/openservice-ws-client):
//   call:     [channel, id, action, [params]]
//   response: [channel, id, result, err]        err = [message, stack]
//   event:    [channel, null, [path, data]]     state patch
// Frames arrive as a JSON array of messages.
(() => {
  "use strict";
  if (window.__FASTBET__) return;

  const WS_URL = "wss://api.chips.gg/prod/socket";
  const TOKEN_KEY = "tokenid";
  const SETTINGS_KEY = "fastbet:v2";
  const MIN_TARGET = 1.01; // public.getGameConfig({gamename:"limbo"})
  const MAX_TARGET = 1000000;
  const RPC_TIMEOUT = 20000;
  const HISTORY_MAX = 500;
  const TABLE_ROWS = 120;
  const RIBBON_MAX = 30;
  const SERIES_MAX = 4000;

  // ---------------------------------------------------------------- amounts
  // Bet amounts are integer strings in the currency's smallest unit
  // (value * 10^decimals). Everything money-related stays in BigInt.

  function parseUnits(str, dec) {
    const s = String(str ?? "").trim().replace(",", ".");
    if (s === "" || s === "." || !/^\d*\.?\d*$/.test(s)) return null;
    const [int, frac = ""] = s.split(".");
    return BigInt((int || "0") + frac.padEnd(dec, "0").slice(0, dec));
  }

  function formatUnits(v, dec, maxFrac = 8) {
    const neg = v < 0n;
    const s = (neg ? -v : v).toString().padStart(dec + 1, "0");
    const int = s.slice(0, s.length - dec);
    const frac = s.slice(s.length - dec, s.length - dec + maxFrac).replace(/0+$/, "");
    return (neg ? "-" : "") + int + (frac ? "." + frac : "");
  }

  function toBig(v) {
    if (typeof v === "bigint") return v;
    if (typeof v === "string" && /^-?\d+$/.test(v.trim())) return BigInt(v.trim());
    if (typeof v === "number" && Number.isInteger(v)) return BigInt(v);
    return null;
  }

  // v * (1 + pct/100), rounded down
  const scale = (v, pct) => (v * BigInt(Math.round((100 + pct) * 1e6))) / 100000000n;

  const clamp = (n, lo, hi) => Math.min(hi, Math.max(lo, n));
  const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
  const num = (s) => Number(String(s ?? "").trim().replace(",", "."));
  const esc = (s) => String(s).replace(/[&<>"']/g, (c) => `&#${c.charCodeAt(0)};`);

  function pushTop(list, v, n = 8) {
    let i = list.findIndex((x) => v > x);
    if (i === -1) i = list.length;
    if (i >= n) return;
    list.splice(i, 0, v);
    if (list.length > n) list.pop();
  }

  // --------------------------------------------------------------- settings
  const DEFAULTS = {
    mode: "manual", // manual | auto
    currency: "", // "" = follow the currency selected on the site
    unit: "coin", // coin | usd
    amount: "",
    target: "2",
    count: "0",
    delay: "0",
    onWin: "reset",
    onWinPct: "0",
    onLoss: "reset",
    onLossPct: "100",
    stopProfit: "",
    stopLoss: "",
    maxBet: "",
    stopOnHit: false,
    hitMulti: "",
    hitSound: true,
    seedEnabled: false,
    seedEvery: "100",
    hotkeys: true,
    hidden: false,
    open: { strategy: true, count: true, stops: true, seed: true, keys: false },
    wins: {},
  };

  function loadSettings() {
    try {
      return JSON.parse(localStorage.getItem(SETTINGS_KEY)) || {};
    } catch {
      return {};
    }
  }
  const saved = loadSettings();
  const settings = {
    ...DEFAULTS,
    ...saved,
    open: { ...DEFAULTS.open, ...saved.open },
    wins: { ...saved.wins },
  };
  function saveSettings() {
    try {
      localStorage.setItem(SETTINGS_KEY, JSON.stringify(settings));
    } catch {}
  }

  // ----------------------------------------------------------------- socket
  const net = {
    ws: null,
    seq: 0,
    pending: new Map(),
    state: {},
    status: "connecting", // connecting | auth | ready | nologin | authfail | offline
    detail: "",
    userid: null,
    token: null,
    me: null,
    retries: 0,
  };

  function setStatus(status, detail = "") {
    net.status = status;
    net.detail = detail;
    render();
  }

  function connect() {
    const ws = new WebSocket(WS_URL);
    net.ws = ws;
    setStatus("connecting");
    ws.onopen = () => {
      net.retries = 0;
      authenticate();
    };
    ws.onmessage = (e) => onFrame(e.data);
    ws.onerror = () => {};
    ws.onclose = () => {
      if (net.ws !== ws) return;
      failPending(new Error("Connection lost"));
      net.userid = null;
      setStatus("offline");
      setTimeout(connect, Math.min(5000, 300 * 2 ** net.retries++));
    };
  }

  function onFrame(raw) {
    let msgs;
    try {
      msgs = JSON.parse(raw);
    } catch {
      return;
    }
    if (!Array.isArray(msgs)) return;
    let changed = false;
    for (const m of msgs) {
      if (!Array.isArray(m)) continue;
      if (m[1] == null) {
        applyEvent(m[0], m[2]);
        changed = true;
        continue;
      }
      const p = net.pending.get(m[1]);
      if (!p) continue;
      net.pending.delete(m[1]);
      clearTimeout(p.timer);
      if (m[3]) p.reject(new Error(Array.isArray(m[3]) ? m[3][0] : String(m[3])));
      else p.resolve(m[2]);
    }
    if (changed) render();
  }

  function applyEvent(channel, patch) {
    const [path = [], data] = Array.isArray(patch) ? patch : [];
    if (!path.length) {
      net.state[channel] = data;
      return;
    }
    if (!net.state[channel] || typeof net.state[channel] !== "object") net.state[channel] = {};
    let o = net.state[channel];
    for (let i = 0; i < path.length - 1; i++) {
      if (!o[path[i]] || typeof o[path[i]] !== "object") o[path[i]] = {};
      o = o[path[i]];
    }
    const last = path[path.length - 1];
    if (data == null) delete o[last];
    else o[last] = data;
  }

  function call(channel, action, ...args) {
    return new Promise((resolve, reject) => {
      const ws = net.ws;
      if (!ws || ws.readyState !== WebSocket.OPEN) return reject(new Error("Not connected"));
      const id = ++net.seq;
      const timer = setTimeout(() => {
        net.pending.delete(id);
        reject(new Error("Server timeout"));
      }, RPC_TIMEOUT);
      net.pending.set(id, { resolve, reject, timer });
      ws.send(JSON.stringify([channel, id, action, args]));
    });
  }

  function failPending(err) {
    for (const p of net.pending.values()) {
      clearTimeout(p.timer);
      p.reject(err);
    }
    net.pending.clear();
  }

  async function authenticate() {
    const token = localStorage.getItem(TOKEN_KEY);
    net.token = token;
    if (!token) return setStatus("nologin");
    setStatus("auth");
    try {
      // guest tokens authenticate fine but come back without a userid
      net.userid = await call("auth", "authenticate", token);
      if (!net.userid) return setStatus("nologin");
      setStatus("ready");
      call("private", "me")
        .then((me) => {
          if (me && typeof me === "object") net.me = me;
          render();
        })
        .catch(() => {});
      refreshSeed();
    } catch (e) {
      setStatus("authfail", e.message);
    }
  }

  // Pick up a login / account switch without a page reload.
  setInterval(() => {
    if (!net.ws || net.ws.readyState !== WebSocket.OPEN) return;
    const token = localStorage.getItem(TOKEN_KEY);
    if (token !== net.token && net.status !== "auth") authenticate();
  }, 2000);

  // The site merges every channel's state into one object; look keys up the same way.
  function pick(key) {
    for (const ch of ["private", "public", ...Object.keys(net.state)]) {
      const s = net.state[ch];
      if (s && typeof s === "object" && s[key] != null) return s[key];
    }
    return undefined;
  }
  const currencies = () => pick("currencies") || {};
  const wallets = () => pick("myWallets") || {};
  const me = () => pick("me") || net.me || {};
  const activeCurrency = () => settings.currency || me().currency || "";

  function curInfo(cur) {
    const c = currencies()[cur];
    if (!c || !Number.isInteger(c.decimals)) return null;
    return { decimals: c.decimals, price: Number(c.price) || 0 };
  }

  // Amount typed in the panel (coin or USD) -> base units BigInt.
  function toBase(str, cur) {
    const info = curInfo(cur);
    if (!info || String(str ?? "").trim() === "") return null;
    if (settings.unit === "usd") {
      const usd = num(str);
      if (!(usd >= 0) || !(info.price > 0)) return null;
      return parseUnits((usd / info.price).toFixed(Math.min(info.decimals, 20)), info.decimals);
    }
    return parseUnits(str, info.decimals);
  }

  function fmtCoin(v, cur) {
    const info = curInfo(cur);
    return info ? formatUnits(v, info.decimals, 8) : String(v);
  }

  function usdValue(v, cur) {
    const info = curInfo(cur);
    if (!info || !info.price) return null;
    return Number(formatUnits(v, info.decimals, info.decimals)) * info.price;
  }

  function fmtUsd(v, cur) {
    const usd = usdValue(v, cur);
    if (usd == null) return "";
    const abs = Math.abs(usd);
    const d = abs !== 0 && abs < 0.01 ? 4 : 2;
    return (usd < 0 ? "-$" : "$") + abs.toLocaleString("en-US", { minimumFractionDigits: d, maximumFractionDigits: d });
  }

  // Primary / secondary display follows the unit picked in the panel.
  const usdMode = () => settings.unit === "usd";
  function prim(v, cur, sign = false) {
    const plus = sign && v > 0n ? "+" : "";
    if (usdMode() && curInfo(cur)?.price) return plus + fmtUsd(v, cur);
    return plus + fmtCoin(v, cur) + (cur ? " " + cur.toUpperCase() : "");
  }
  function sec(v, cur, sign = false) {
    const plus = sign && v > 0n ? "+" : "";
    if (usdMode()) return plus + fmtCoin(v, cur) + (cur ? " " + cur.toUpperCase() : "");
    return plus + fmtUsd(v, cur);
  }

  function fmtMulti(m) {
    if (!Number.isFinite(m)) return "?";
    return (m >= 1000 ? Math.floor(m).toLocaleString("en-US") : m.toFixed(2)) + "×";
  }

  function fmtTime(ms) {
    const s = Math.max(0, Math.floor(ms / 1000));
    const p = (n) => String(n).padStart(2, "0");
    return `${p(Math.floor(s / 3600))}:${p(Math.floor(s / 60) % 60)}:${p(s % 60)}`;
  }

  // ----------------------------------------------------------- stats/history
  let histVer = 0;
  let statVer = 0;
  const history = [];
  const stats = freshStats("");

  function freshStats(cur) {
    return {
      cur,
      bets: 0,
      wins: 0,
      losses: 0,
      hits: 0,
      retries: 0,
      wagered: 0n,
      profit: 0n,
      peak: 0n,
      low: 0n,
      maxDD: 0n,
      hiWin: 0n,
      hiLoss: 0n,
      hiBet: 0n,
      streak: 0, // >0 wins in a row, <0 losses in a row
      winStreaks: [],
      lossStreaks: [],
      topMulti: [],
      series: [0],
      step: 1,
      skip: 0,
      times: [],
      latency: 0,
      started: 0,
      lastAt: 0,
    };
  }

  function resetStats(cur = activeCurrency()) {
    Object.assign(stats, freshStats(cur));
    statVer++;
    render();
  }

  function pushHistory(entry) {
    history.unshift(entry);
    if (history.length > HISTORY_MAX) history.pop();
    histVer++;
  }

  function hitLine(target) {
    if (!settings.stopOnHit) return null;
    const h = num(settings.hitMulti);
    return h >= MIN_TARGET ? h : target;
  }

  function settle(res, sent, target, cur, ms) {
    if (stats.cur !== cur) Object.assign(stats, freshStats(cur));
    const r = res && typeof res === "object" ? res : {};
    // The reply's `multiplier` is the payout multiplier (0 on a loss), so the
    // target is always the autoCashout we sent.
    const outcome = Number(r.outcome);
    const multi = target;
    const amount = toBig(r.amount) ?? sent;
    let payout = toBig(r.winnings);
    const win = typeof r.win === "boolean" ? r.win : payout != null ? payout > 0n : outcome >= multi;
    if (payout == null) payout = win ? (amount * BigInt(Math.floor(multi * 1e6))) / 1000000n : 0n;
    const profit = payout - amount;
    const line = hitLine(multi);
    const hit = line != null && outcome >= line;

    const now = Date.now();
    const s = stats;
    if (!s.started) s.started = now;
    s.lastAt = now;
    s.bets++;
    if (win) {
      s.wins++;
      if (s.streak < 0) pushTop(s.lossStreaks, -s.streak);
      s.streak = s.streak > 0 ? s.streak + 1 : 1;
    } else {
      s.losses++;
      if (s.streak > 0) pushTop(s.winStreaks, s.streak);
      s.streak = s.streak < 0 ? s.streak - 1 : -1;
    }
    if (hit) s.hits++;
    s.wagered += amount;
    s.profit += profit;
    if (s.profit > s.peak) s.peak = s.profit;
    if (s.profit < s.low) s.low = s.profit;
    if (s.peak - s.profit > s.maxDD) s.maxDD = s.peak - s.profit;
    if (profit > s.hiWin) s.hiWin = profit;
    if (-profit > s.hiLoss) s.hiLoss = -profit;
    if (amount > s.hiBet) s.hiBet = amount;
    if (Number.isFinite(outcome)) pushTop(s.topMulti, outcome);
    s.latency = s.latency ? s.latency * 0.8 + ms * 0.2 : ms;
    const t = performance.now();
    s.times.push(t);
    while (s.times.length && t - s.times[0] > 60000) s.times.shift();
    if (++s.skip >= s.step) {
      s.skip = 0;
      const info = curInfo(cur);
      s.series.push(info ? Number(formatUnits(s.profit, info.decimals, info.decimals)) : 0);
      if (s.series.length > SERIES_MAX) {
        s.series = s.series.filter((_, i) => i % 2 === 0);
        s.step *= 2;
      }
    }
    if (r.nonce != null && seed.state) seed.state.nonce = r.nonce;
    statVer++;

    const entry = {
      n: s.bets,
      nonce: r.nonce ?? null,
      t: new Date(now).toLocaleTimeString([], { hour: "numeric", minute: "2-digit", second: "2-digit" }),
      target: multi,
      outcome,
      win,
      hit,
      amount,
      payout,
      profit,
      cur,
      ms,
      fresh: true,
    };
    pushHistory(entry);
    return entry;
  }

  // ------------------------------------------------------------- bet lane
  // chips.gg takes one bet per account at a time and answers anything sent
  // while one is open with "Please wait and try again later". Every bet and
  // seed change therefore runs through this single lane, in order.
  let lane = Promise.resolve();
  let queued = 0;
  function inLane(task) {
    queued++;
    render();
    const run = lane.then(task).finally(() => {
      queued--;
      render();
    });
    lane = run.catch(() => {});
    return run;
  }

  const BUSY = /please wait|try again later|too many|rate.?limit|slow down/i;
  const BUSY_RETRIES = 8;
  const MAX_QUEUE = 10;
  class Cancelled extends Error {}

  // ------------------------------------------------------------ provable seed
  // rotateProvableSeed({clientSeed}) swaps in a new client seed and resets the nonce.
  const seed = { count: 0, state: null, rotations: 0, busy: false };

  function randomSeed() {
    const a = new Uint8Array(12);
    crypto.getRandomValues(a);
    return Array.from(a, (b) => b.toString(16).padStart(2, "0")).join("");
  }

  const seedEvery = () => (settings.seedEnabled ? Math.max(0, parseInt(settings.seedEvery, 10) || 0) : 0);

  function refreshSeed() {
    call("private", "getMyProvableState")
      .then((st) => {
        if (st && typeof st === "object") seed.state = { ...st };
        render();
      })
      .catch(() => {});
  }

  // Only call from inside the lane.
  async function rotateSeed(reason) {
    seed.busy = true;
    render();
    try {
      const clientSeed = randomSeed();
      const st = await call("private", "rotateProvableSeed", { clientSeed });
      seed.state = { clientSeed, nonce: 0, ...(st && typeof st === "object" ? st : {}) };
      seed.count = 0;
      seed.rotations++;
      pushHistory({ seed: true, t: new Date().toLocaleTimeString(), clientSeed: seed.state.clientSeed, reason });
    } catch (e) {
      throw new Error("Seed change failed: " + e.message);
    } finally {
      seed.busy = false;
      render();
    }
  }

  function rotateNow() {
    inLane(() => rotateSeed("manual")).catch((e) => flash(e.message));
  }

  // ------------------------------------------------------------------ bets
  function betSetup() {
    if (net.status !== "ready") throw new Error(statusText());
    const cur = activeCurrency();
    if (!cur) throw new Error("No currency selected yet");
    if (!curInfo(cur)) throw new Error("Currency data not loaded yet");
    const amount = toBase(settings.amount, cur);
    if (amount == null || amount <= 0n) throw new Error("Enter a bet amount");
    const target = num(settings.target);
    if (!(target >= MIN_TARGET && target <= MAX_TARGET)) {
      throw new Error(`Target must be ${MIN_TARGET}× to ${MAX_TARGET.toLocaleString("en-US")}×`);
    }
    return { cur, amount, target };
  }

  // Sends one bet, waiting and retrying while the server says it's busy.
  async function sendBet(params, keepGoing) {
    for (let attempt = 0; ; attempt++) {
      const t0 = performance.now();
      try {
        const res = await call("private", "betLimbo", params);
        return { res, ms: performance.now() - t0 };
      } catch (e) {
        if (!BUSY.test(e.message) || attempt >= BUSY_RETRIES) throw e;
        stats.retries++;
        statVer++;
        await sleep(Math.min(1000, 50 * 2 ** attempt));
        if (!keepGoing()) throw new Cancelled("Cancelled");
      }
    }
  }

  function placeBet({ cur, amount, target }, keepGoing = () => true) {
    return inLane(async () => {
      if (!keepGoing()) throw new Cancelled("Cancelled");
      const every = seedEvery();
      if (every && seed.count >= every) await rotateSeed("auto");
      seed.count++;
      render();
      try {
        const { res, ms } = await sendBet({ currency: cur, amount: amount.toString(), autoCashout: target }, keepGoing);
        return settle(res, amount, target, cur, ms);
      } catch (e) {
        seed.count--;
        throw e;
      }
    });
  }

  async function manualBet() {
    if (auto.running) return;
    let setup;
    try {
      setup = betSetup();
    } catch (e) {
      return flash(e.message);
    }
    if (queued >= MAX_QUEUE) return flash(`${MAX_QUEUE} bets already queued`);
    try {
      const r = await placeBet(setup);
      if (r.hit && settings.hitSound) ding();
    } catch (e) {
      console.warn("[FastBet] bet failed", e);
      flash(e.message);
    }
  }

  // --------------------------------------------------------------- autobet
  const auto = { running: false, run: 0, msg: "", cur: "", done: 0 };

  function startAuto() {
    let setup, stopProfit, stopLoss, maxBet;
    try {
      setup = betSetup();
      const opt = (v, name) => {
        if (String(v ?? "").trim() === "") return null;
        const b = toBase(v, setup.cur);
        if (b == null) throw new Error(`Invalid ${name}`);
        return b;
      };
      stopProfit = opt(settings.stopProfit, "stop profit");
      stopLoss = opt(settings.stopLoss, "stop loss");
      maxBet = opt(settings.maxBet, "max bet");
      const hm = String(settings.hitMulti).trim();
      if (settings.stopOnHit && hm && !(num(hm) >= MIN_TARGET)) throw new Error(`Hit multiplier must be at least ${MIN_TARGET}×`);
    } catch (e) {
      return flash(e.message);
    }
    if (maxBet != null && setup.amount > maxBet) return flash("Bet size is above max bet");

    Object.assign(auto, {
      running: true,
      run: auto.run + 1,
      cur: setup.cur,
      target: setup.target,
      base: setup.amount,
      stake: setup.amount,
      limit: Math.max(0, parseInt(settings.count, 10) || 0),
      delay: Math.max(0, num(settings.delay) || 0),
      onWin: settings.onWin === "increase" ? num(settings.onWinPct) || 0 : null,
      onLoss: settings.onLoss === "increase" ? num(settings.onLossPct) || 0 : null,
      stopProfit,
      stopLoss,
      maxBet,
      done: 0,
      hits: 0,
      profit: 0n,
      paused: false,
      wake: null,
      msg: "Running",
    });
    $("err").textContent = "";
    worker(auto.run);
    render();
  }

  function stopAuto(msg = "Stopped") {
    if (!auto.running) return;
    auto.running = false;
    auto.msg = msg;
    wakeAuto();
    render();
  }

  // Pausing keeps the run (bet count, profit, current stake) and continues it on resume.
  function wakeAuto() {
    auto.paused = false;
    const wake = auto.wake;
    auto.wake = null;
    if (wake) wake();
  }
  function togglePause() {
    if (!auto.running) return;
    if (auto.paused) {
      wakeAuto();
      auto.msg = "Running";
    } else {
      auto.paused = true;
      auto.msg = "Paused";
    }
    render();
  }

  async function worker(run) {
    const live = () => auto.running && auto.run === run;
    while (live()) {
      if (auto.paused) {
        await new Promise((r) => (auto.wake = r));
        continue;
      }
      const amount = auto.stake;
      if (amount <= 0n) return stopAuto("Bet size hit 0");
      if (auto.maxBet != null && amount > auto.maxBet) return stopAuto("Next bet over max bet");
      let r;
      try {
        r = await placeBet({ cur: auto.cur, amount, target: auto.target }, live);
      } catch (e) {
        if (e instanceof Cancelled) return;
        console.warn("[FastBet] auto bet failed", e);
        if (live()) stopAuto(e.message.startsWith("Seed") ? e.message : "Error: " + e.message);
        return;
      }
      if (auto.run !== run) return;
      auto.done++;
      auto.profit += r.profit;
      const pct = r.win ? auto.onWin : auto.onLoss;
      auto.stake = pct == null ? auto.base : scale(auto.stake, pct);
      if (r.hit) {
        auto.hits++;
        if (live()) {
          if (settings.hitSound) ding();
          lastHit = r;
          return stopAuto(`HIT ${fmtMulti(r.outcome)}`);
        }
      }
      if (!live()) return;
      if (auto.stopProfit != null && auto.profit >= auto.stopProfit) return stopAuto("Stop profit reached");
      if (auto.stopLoss != null && -auto.profit >= auto.stopLoss) return stopAuto("Stop loss reached");
      if (auto.limit && auto.done >= auto.limit) return stopAuto("Done");
      if (auto.delay) await sleep(auto.delay);
    }
  }

  // ----------------------------------------------------------------- sound
  let actx;
  function ding() {
    try {
      actx = actx || new AudioContext();
      const t = actx.currentTime;
      for (const [f, d] of [
        [880, 0],
        [1320, 0.09],
      ]) {
        const o = actx.createOscillator();
        const g = actx.createGain();
        o.type = "triangle";
        o.frequency.setValueAtTime(f, t + d);
        g.gain.setValueAtTime(0.0001, t + d);
        g.gain.exponentialRampToValueAtTime(0.22, t + d + 0.02);
        g.gain.exponentialRampToValueAtTime(0.0001, t + d + 0.45);
        o.connect(g).connect(actx.destination);
        o.start(t + d);
        o.stop(t + d + 0.5);
      }
    } catch {}
  }

  // -------------------------------------------------------------------- UI
  const svg = (p) =>
    `<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">${p}</svg>`;
  const I = {
    bolt: `<svg viewBox="0 0 24 24"><path fill="currentColor" d="M13.2 2 4 13.6h6.6L9.8 22 20 9.9h-6.7z"/></svg>`,
    x: svg(`<path d="M18 6 6 18M6 6l12 12"/>`),
    minus: svg(`<path d="M5 12h14"/>`),
    plus: svg(`<path d="M12 5v14M5 12h14"/>`),
    layout: svg(`<rect x="3" y="4" width="18" height="16" rx="2"/><path d="M9 4v16M15 4v16"/>`),
    chev: svg(`<path d="m6 9 6 6 6-6"/>`),
    live: svg(`<path d="M22 12h-4l-3 9L9 3l-3 9H2"/>`),
    chart: svg(`<path d="M12 20V10M18 20V4M6 20v-4"/>`),
    seed: svg(`<path d="M21 3v6h-6M3 21v-6h6"/><path d="M3.5 9a9 9 0 0 1 14.9-3.4L21 9M3 15l2.6 3.4A9 9 0 0 0 20.5 15"/>`),
    trend: svg(`<path d="m23 6-9.5 9.5-5-5L1 18"/><path d="M17 6h6v6"/>`),
    hash: svg(`<path d="M4 9h16M4 15h16M10 3 8 21M16 3l-2 18"/>`),
    stop: svg(`<path d="M7.9 2h8.2L22 7.9v8.2L16.1 22H7.9L2 16.1V7.9z"/><path d="M9 9h6v6H9z"/>`),
    keys: svg(`<rect x="2" y="6" width="20" height="12" rx="2"/><path d="M6 10h.01M10 10h.01M14 10h.01M18 10h.01M7 14h10"/>`),
    clock: svg(`<circle cx="12" cy="12" r="9"/><path d="M12 7v5l3 2"/>`),
    trash: svg(`<path d="M3 6h18M8 6V4h8v2M6 6l1 14h10l1-14"/>`),
    reset: svg(`<path d="M1 4v6h6"/><path d="M3.5 15a9 9 0 1 0 2.1-9.4L1 10"/>`),
    target: svg(`<circle cx="12" cy="12" r="9"/><circle cx="12" cy="12" r="5"/><circle cx="12" cy="12" r="1"/>`),
    pause: `<svg viewBox="0 0 24 24"><rect fill="currentColor" x="6" y="5" width="4.2" height="14" rx="1.2"/><rect fill="currentColor" x="13.8" y="5" width="4.2" height="14" rx="1.2"/></svg>`,
    play: `<svg viewBox="0 0 24 24"><path fill="currentColor" d="M8 5.2v13.6a1 1 0 0 0 1.5.86l11-6.8a1 1 0 0 0 0-1.72l-11-6.8A1 1 0 0 0 8 5.2z"/></svg>`,
  };

  const CSS = `
    :host { all: initial; }
    * { box-sizing: border-box; margin: 0; padding: 0; font-family: Poppins, Inter, system-ui, -apple-system, "Segoe UI", sans-serif; }
    svg { width: 15px; height: 15px; flex: none; display: block; }
    button { font: inherit; color: inherit; background: none; border: 0; cursor: pointer; }
    button:disabled { cursor: default; }
    .num, input, .tbl td { font-variant-numeric: tabular-nums; }

    .win {
      --bg: #0c0f16; --bg1: #121722; --bg2: #171d2b; --bg3: #1e2536; --line: #232a3b; --line2: #2e3750;
      --text: #e9edf7; --muted: #7d869e; --dim: #566078;
      --g1: #00f5a0; --g2: #00d4ff; --green: #18e68e; --red: #ff3d63; --gold: #ffc23d; --blue: #6a7dff;
      position: absolute; pointer-events: auto; display: flex; flex-direction: column;
      color: var(--text); font-size: 12px; line-height: 1.35;
      background: linear-gradient(180deg, rgba(20,25,36,.97) 0%, rgba(11,14,20,.98) 100%);
      border: 1px solid var(--line); border-radius: 16px; overflow: hidden;
      box-shadow: 0 24px 70px rgba(0,0,0,.65), 0 0 0 1px rgba(255,255,255,.02) inset;
      backdrop-filter: blur(14px);
    }
    .win::before { content: ""; position: absolute; left: 18px; right: 18px; top: 0; height: 1px; z-index: 2;
      background: linear-gradient(90deg, transparent, var(--g1), var(--g2), transparent); opacity: .65; pointer-events: none; }
    .win.resizable { resize: both; }
    .win.min { resize: none; height: auto !important; }
    .win.min .win-bd { display: none; }

    .win-hd { display: flex; align-items: center; gap: 7px; padding: 10px 8px 10px 12px; cursor: grab;
      background: linear-gradient(180deg, rgba(255,255,255,.035), transparent); border-bottom: 1px solid var(--line);
      user-select: none; flex: none; }
    .win-hd:active { cursor: grabbing; }
    .win.min .win-hd { border-bottom: 0; }
    .tools { margin-left: auto; display: flex; gap: 2px; flex: none; }
    .tb { width: 24px; height: 24px; border-radius: 8px; display: grid; place-items: center; color: var(--muted); transition: .15s; }
    .tb:hover { background: var(--bg3); color: var(--text); }
    .tb.on { color: var(--g1); background: rgba(0,245,160,.08); }
    .wt { display: flex; align-items: center; gap: 7px; font-weight: 700; font-size: 13px; letter-spacing: .2px; }
    .wt svg { color: var(--g1); }
    .hstat { display: flex; align-items: center; gap: 5px; color: var(--muted); font-weight: 600; font-variant-numeric: tabular-nums; }
    .hstat svg { width: 13px; height: 13px; }

    .brand { display: flex; align-items: center; gap: 6px; }
    .logo { width: 26px; height: 26px; border-radius: 8px; display: grid; place-items: center; color: #04140d;
      background: linear-gradient(135deg, var(--g1), var(--g2)); box-shadow: 0 0 18px rgba(0,245,160,.45); }
    .logo svg { width: 16px; height: 16px; }
    .brand-t { font-weight: 800; font-size: 14px; letter-spacing: .8px; }
    .brand-t b { background: linear-gradient(90deg, var(--g1), var(--g2)); -webkit-background-clip: text; background-clip: text; color: transparent; }
    .pill { display: flex; align-items: center; gap: 6px; padding: 3px 9px; border-radius: 99px; background: var(--bg2);
      border: 1px solid var(--line); color: var(--muted); font-size: 11px; font-weight: 600; white-space: nowrap; min-width: 0; }
    .pill span { overflow: hidden; text-overflow: ellipsis; }
    .brand { flex: none; }
    .dot { width: 7px; height: 7px; border-radius: 50%; background: var(--gold); box-shadow: 0 0 8px var(--gold); }
    .pill.ok .dot { background: var(--green); box-shadow: 0 0 8px var(--green); animation: breathe 2.4s infinite; }
    .pill.bad { color: #ff8da3; } .pill.bad .dot { background: var(--red); box-shadow: 0 0 8px var(--red); }
    @keyframes breathe { 50% { opacity: .45; } }

    .win-bd { display: flex; flex-direction: column; min-height: 0; flex: 1; }

    /* ---------- control window */
    .ctrl { width: 336px; }
    .live { min-width: 380px; min-height: 220px; }
    .stats { min-width: 250px; min-height: 200px; }
    .balance { margin: 12px 12px 0; padding: 12px 14px; border-radius: 12px; display: flex; justify-content: space-between; align-items: flex-end;
      background: radial-gradient(120% 140% at 0% 0%, rgba(0,245,160,.16), transparent 55%), var(--bg1); border: 1px solid var(--line); }
    .lbl { color: var(--muted); font-size: 10px; text-transform: uppercase; letter-spacing: 1px; font-weight: 600; }
    .bal-v { font-size: 19px; font-weight: 800; letter-spacing: .2px; margin-top: 2px; font-variant-numeric: tabular-nums; }
    .bal-usd { color: var(--muted); font-weight: 600; font-variant-numeric: tabular-nums; }

    .tabs { position: relative; margin: 10px 12px 0; display: grid; grid-template-columns: 1fr 1fr; padding: 3px; border-radius: 11px;
      background: var(--bg); border: 1px solid var(--line); }
    .tabs button { position: relative; z-index: 1; height: 30px; border-radius: 8px; font-weight: 700; color: var(--muted); transition: color .2s; }
    .tabs button.on { color: #04140d; }
    .tab-ind { position: absolute; top: 3px; bottom: 3px; left: 3px; width: calc(50% - 3px); border-radius: 8px;
      background: linear-gradient(135deg, var(--g1), var(--g2)); box-shadow: 0 0 16px rgba(0,245,160,.35); transition: transform .25s cubic-bezier(.4,1.3,.5,1); }
    .tabs[data-mode="auto"] .tab-ind { transform: translateX(100%); }

    .scroll { overflow-y: auto; min-height: 0; flex: 1; padding: 10px 12px 4px; display: flex; flex-direction: column; gap: 8px; }
    .scroll::-webkit-scrollbar, .tblwrap::-webkit-scrollbar, .sbody::-webkit-scrollbar { width: 6px; height: 6px; }
    .scroll::-webkit-scrollbar-thumb, .tblwrap::-webkit-scrollbar-thumb, .sbody::-webkit-scrollbar-thumb { background: var(--line2); border-radius: 9px; }

    .card { background: var(--bg1); border: 1px solid var(--line); border-radius: 12px; padding: 10px; display: flex; flex-direction: column; gap: 9px; }
    .row2 { display: grid; grid-template-columns: 1fr 1fr; gap: 8px; }
    .row3 { display: grid; grid-template-columns: 1fr 1fr 1fr; gap: 6px; }
    .f { display: flex; flex-direction: column; gap: 4px; color: var(--muted); font-size: 11px; font-weight: 600; min-width: 0; }
    .fl { display: flex; justify-content: space-between; gap: 6px; }
    .fl em { font-style: normal; color: var(--dim); font-weight: 600; }
    input, select { width: 100%; height: 32px; background: var(--bg); color: var(--text); border: 1px solid var(--line); border-radius: 8px;
      padding: 0 9px; font-size: 12px; font-weight: 600; outline: none; transition: border-color .15s, box-shadow .15s; }
    select { cursor: pointer; padding-right: 4px; }
    input::placeholder { color: var(--dim); font-weight: 500; }
    input:focus, select:focus { border-color: rgba(0,245,160,.6); box-shadow: 0 0 0 3px rgba(0,245,160,.12); }
    input:disabled, select:disabled { opacity: .5; }
    .ig { display: flex; align-items: center; background: var(--bg); border: 1px solid var(--line); border-radius: 8px; padding-right: 3px; transition: border-color .15s, box-shadow .15s; }
    .ig:focus-within { border-color: rgba(0,245,160,.6); box-shadow: 0 0 0 3px rgba(0,245,160,.12); }
    .ig input { border: 0; background: none; box-shadow: none !important; flex: 1; min-width: 0; }
    .ig select { width: auto; flex: 2; min-width: 0; border: 0; background: none; box-shadow: none !important; }
    .ig .pct { flex: 1; min-width: 38px; padding: 0 4px 0 8px; border-left: 1px solid var(--line); border-radius: 0; height: 22px; }
    .unit { color: var(--dim); font-weight: 700; font-size: 11px; padding: 0 6px; white-space: nowrap; }
    .mini { height: 25px; min-width: 30px; padding: 0 7px; border-radius: 6px; background: var(--bg3); color: var(--text); font-weight: 700; font-size: 11px; margin-left: 3px; transition: .15s; }
    .mini:hover:not(:disabled) { background: var(--line2); }
    .mini:disabled { opacity: .45; }
    .seg { display: grid; grid-template-columns: 1fr 1fr; height: 32px; padding: 3px; border-radius: 8px; background: var(--bg); border: 1px solid var(--line); }
    .seg button { border-radius: 6px; font-size: 11px; font-weight: 700; color: var(--muted); }
    .seg button.on { background: var(--bg3); color: var(--text); box-shadow: 0 1px 0 rgba(255,255,255,.05) inset; }
    .quick { display: flex; gap: 4px; flex-wrap: wrap; }
    .quick button { flex: 1; height: 24px; border-radius: 6px; background: var(--bg); border: 1px solid var(--line); color: var(--muted); font-size: 10.5px; font-weight: 700; transition: .15s; }
    .quick button:hover:not(:disabled) { color: var(--text); border-color: var(--line2); }
    .quick button.on { color: var(--g1); border-color: rgba(0,245,160,.4); background: rgba(0,245,160,.06); }

    .sec { background: var(--bg1); border: 1px solid var(--line); border-radius: 12px; overflow: hidden; flex: none; }
    .sec-hd { width: 100%; display: flex; align-items: center; gap: 8px; padding: 10px 11px; font-weight: 700; font-size: 12px; color: #cfd5e4; text-align: left; }
    .sec-hd:hover { background: rgba(255,255,255,.02); }
    .sec-hd svg:first-child { color: var(--g1); width: 14px; height: 14px; }
    .sec-hd .chev { margin-left: auto; color: var(--dim); transition: transform .2s; width: 14px; height: 14px; }
    .sec.open .chev { transform: rotate(180deg); }
    .badge { font-size: 9.5px; font-weight: 800; letter-spacing: .6px; padding: 2px 6px; border-radius: 5px; background: var(--bg3); color: var(--dim); }
    .badge.on { color: #04140d; background: linear-gradient(135deg, var(--g1), var(--g2)); }
    .sec-bd { display: none; padding: 0 11px 11px; flex-direction: column; gap: 9px; }
    .sec.open .sec-bd { display: flex; }
    .note { color: var(--dim); font-size: 10.5px; font-weight: 500; }

    .swrow { display: flex; align-items: center; justify-content: space-between; gap: 10px; color: #cfd5e4; font-weight: 600; font-size: 12px; }
    .swrow small { display: block; color: var(--dim); font-weight: 500; font-size: 10.5px; }
    .sw { position: relative; width: 36px; height: 20px; flex: none; cursor: pointer; }
    .sw input { position: absolute; opacity: 0; width: 0; height: 0; }
    .sw i { position: absolute; inset: 0; border-radius: 99px; background: var(--bg3); border: 1px solid var(--line2); transition: .2s; }
    .sw i::after { content: ""; position: absolute; top: 2px; left: 2px; width: 14px; height: 14px; border-radius: 50%; background: #8a93ab; transition: .2s cubic-bezier(.4,1.3,.5,1); }
    .sw input:checked + i { background: linear-gradient(135deg, var(--g1), var(--g2)); border-color: transparent; box-shadow: 0 0 12px rgba(0,245,160,.35); }
    .sw input:checked + i::after { transform: translateX(16px); background: #04140d; }
    .sw input:focus-visible + i { outline: 2px solid rgba(0,245,160,.5); outline-offset: 2px; }

    .seedinfo { display: grid; grid-template-columns: 1fr 1fr; gap: 6px; }
    .kv { background: var(--bg); border: 1px solid var(--line); border-radius: 8px; padding: 6px 8px; min-width: 0; }
    .kv span { display: block; color: var(--dim); font-size: 10px; font-weight: 600; }
    .kv b { display: block; font-size: 11.5px; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; font-variant-numeric: tabular-nums; }
    .kv.wide { grid-column: span 2; }
    .ghost { height: 30px; border-radius: 8px; border: 1px dashed var(--line2); color: #cfd5e4; font-weight: 700; display: flex; align-items: center; justify-content: center; gap: 6px; transition: .15s; }
    .ghost:hover:not(:disabled) { border-color: var(--g1); color: var(--g1); }
    .ghost:disabled { opacity: .5; }
    .ghost svg { width: 13px; height: 13px; }
    kbd { display: inline-block; min-width: 22px; text-align: center; background: var(--bg3); border: 1px solid var(--line2); border-bottom-width: 2px;
      border-radius: 5px; padding: 0 5px; font-size: 10.5px; font-weight: 700; color: var(--text); font-family: inherit; }
    .keys { display: grid; grid-template-columns: auto 1fr; gap: 6px 10px; align-items: center; color: var(--muted); font-weight: 600; }

    .foot { flex: none; padding: 10px 12px 12px; border-top: 1px solid var(--line); background: linear-gradient(0deg, rgba(0,0,0,.25), transparent);
      display: flex; flex-direction: column; gap: 6px; }
    .go { position: relative; height: 46px; border-radius: 12px; font-size: 15px; font-weight: 800; letter-spacing: 1.6px; color: #04140d; overflow: hidden;
      background: linear-gradient(135deg, var(--g1), var(--g2)); box-shadow: 0 8px 26px rgba(0,245,160,.3), inset 0 1px 0 rgba(255,255,255,.35); transition: transform .08s, filter .15s, box-shadow .2s; }
    .go::after { content: ""; position: absolute; inset: 0; background: linear-gradient(110deg, transparent 30%, rgba(255,255,255,.35) 50%, transparent 70%);
      transform: translateX(-120%); transition: transform .6s; }
    .go:hover:not(:disabled)::after { transform: translateX(120%); }
    .go:hover:not(:disabled) { filter: brightness(1.07); }
    .go:active:not(:disabled) { transform: translateY(1px) scale(.995); }
    .go small { font-size: 10.5px; font-weight: 700; letter-spacing: .4px; opacity: .6; margin-left: 4px; }
    .go.stop { color: #fff; background: linear-gradient(135deg, #ff3d63, #ff7a45); box-shadow: 0 8px 26px rgba(255,61,99,.35); animation: pulse 1.6s infinite; }
    .go:disabled { filter: grayscale(.7) brightness(.6); box-shadow: none; }
    @keyframes pulse { 50% { box-shadow: 0 8px 34px rgba(255,61,99,.6); } }
    .automsg { color: var(--muted); font-weight: 600; font-size: 11px; text-align: center; min-height: 15px; font-variant-numeric: tabular-nums; }
    .automsg.hit { color: var(--gold); text-shadow: 0 0 10px rgba(255,194,61,.5); }
    .err { color: #ff8da3; font-weight: 600; font-size: 11px; text-align: center; }
    .err:empty { display: none; }

    /* ---------- live window */
    .stage { position: relative; flex: none; height: 38%; min-height: 130px; display: flex; flex-direction: column; align-items: center; justify-content: center; gap: 4px;
      background: radial-gradient(60% 80% at 50% 50%, rgba(255,255,255,.025), transparent), #07090d; border-bottom: 1px solid var(--line); overflow: hidden; }
    /* plain gradients: a blur filter here gets cut out around the animated number */
    .glow { position: absolute; inset: 0; opacity: 0; transition: opacity .35s; pointer-events: none; }
    .glow.w { background: radial-gradient(40% 75% at 50% 50%, rgba(24,230,142,.24), rgba(24,230,142,.06) 55%, transparent 80%); opacity: 1; }
    .glow.l { background: radial-gradient(40% 75% at 50% 50%, rgba(255,61,99,.2), rgba(255,61,99,.05) 55%, transparent 80%); opacity: 1; }
    .glow.h { background: radial-gradient(40% 75% at 50% 50%, rgba(255,194,61,.32), rgba(255,194,61,.08) 55%, transparent 80%); opacity: 1; }
    .result { position: relative; font-size: clamp(44px, 8vw, 92px); font-weight: 800; letter-spacing: -1px; color: #3a4258; font-variant-numeric: tabular-nums; line-height: 1; }
    .result.w { color: var(--green); text-shadow: 0 0 40px rgba(24,230,142,.45); }
    .result.l { color: var(--red); text-shadow: 0 0 40px rgba(255,61,99,.4); }
    .result.h { color: var(--gold); text-shadow: 0 0 50px rgba(255,194,61,.6); }
    .result.pop { animation: pop .22s cubic-bezier(.3,1.6,.5,1); }
    @keyframes pop { from { transform: scale(.86); opacity: .6; } to { transform: none; opacity: 1; } }
    .rsub { position: relative; color: var(--muted); font-weight: 600; font-size: 12px; font-variant-numeric: tabular-nums; text-align: center; padding: 0 12px; }
    .rsub b.pos { color: var(--green); } .rsub b.neg { color: var(--red); }
    .hitbadge { position: absolute; top: 12px; left: 12px; padding: 4px 10px; border-radius: 99px; font-weight: 800; font-size: 11px; letter-spacing: 1px;
      color: #1b1200; background: linear-gradient(135deg, #ffd66b, #ffab1f); box-shadow: 0 0 20px rgba(255,194,61,.55); display: none; }
    .hitbadge.on { display: block; animation: pop .3s cubic-bezier(.3,1.6,.5,1); }
    .stagectl { position: absolute; top: 12px; right: 12px; display: flex; gap: 6px; z-index: 2; }
    .sbtn { height: 32px; min-width: 32px; padding: 0 10px; border-radius: 10px; display: flex; align-items: center; justify-content: center; gap: 6px;
      font-weight: 800; font-size: 12px; letter-spacing: .2px; color: var(--muted); background: rgba(10,13,19,.75); border: 1px solid var(--line2);
      backdrop-filter: blur(6px); transition: color .15s, border-color .15s, background .15s, box-shadow .15s; font-variant-numeric: tabular-nums; }
    .sbtn svg { width: 15px; height: 15px; }
    .sbtn:hover:not(:disabled) { color: var(--text); border-color: #3d4864; }
    .sbtn.on { color: var(--gold); border-color: rgba(255,194,61,.55); background: rgba(255,194,61,.1); box-shadow: 0 0 18px rgba(255,194,61,.22); }
    .sbtn.on:hover { color: #ffd772; border-color: rgba(255,194,61,.8); }
    .sbtn.off span { text-decoration: line-through; text-decoration-color: rgba(125,134,158,.6); }
    .sbtn.paused { color: #04140d; background: linear-gradient(135deg, var(--g1), var(--g2)); border-color: transparent; box-shadow: 0 0 18px rgba(0,245,160,.4); animation: breathe 1.6s infinite; }
    .sbtn:disabled { opacity: .35; cursor: default; }
    .stage.paused .result { color: #3d475f; text-shadow: none; }
    .pausedtag { position: relative; display: none; margin-top: 2px; padding: 3px 10px; border-radius: 99px; font-size: 10.5px; font-weight: 800; letter-spacing: 1.4px;
      color: var(--g1); background: rgba(0,245,160,.08); border: 1px solid rgba(0,245,160,.3); }
    .stage.paused .pausedtag { display: block; }
    .ribbon { flex: none; display: flex; gap: 4px; padding: 8px 10px; overflow: hidden; border-bottom: 1px solid var(--line); min-height: 37px;
      -webkit-mask-image: linear-gradient(90deg, #000 80%, transparent); mask-image: linear-gradient(90deg, #000 80%, transparent); }
    .chip { flex: none; font-size: 10.5px; font-weight: 700; padding: 3px 7px; border-radius: 6px; font-variant-numeric: tabular-nums; }
    .chip.w { background: rgba(24,230,142,.12); color: var(--green); }
    .chip.l { background: rgba(255,61,99,.12); color: #ff6d88; }
    .chip.h { background: rgba(255,194,61,.16); color: var(--gold); box-shadow: 0 0 0 1px rgba(255,194,61,.4) inset; }
    .chip.new { animation: slidein .2s ease-out; }
    @keyframes slidein { from { transform: translateX(-10px); opacity: 0; } }
    .tblwrap { flex: 1; min-height: 0; overflow: auto; }
    table { width: 100%; border-collapse: collapse; font-size: 11.5px; }
    th { position: sticky; top: 0; z-index: 1; background: #0f131c; color: var(--dim); font-weight: 700; font-size: 10.5px; text-transform: uppercase; letter-spacing: .5px;
      text-align: right; padding: 7px 10px; border-bottom: 1px solid var(--line); white-space: nowrap; }
    td { text-align: right; padding: 5px 10px; white-space: nowrap; color: #b9c0d3; border-bottom: 1px solid rgba(35,42,59,.55); }
    th:first-child, td:first-child, th:nth-child(3), td:nth-child(3) { text-align: left; }
    td:first-child { color: var(--dim); }
    tr.w td.res { color: var(--green); font-weight: 700; } tr.l td.res { color: var(--red); font-weight: 700; }
    td.pos { color: var(--green); } td.neg { color: #ff6d88; }
    tbody tr:hover td { background: rgba(255,255,255,.025); }
    tr.h td { background: rgba(255,194,61,.09); } tr.h td.res { color: var(--gold); }
    tr.h td:first-child { box-shadow: 3px 0 0 var(--gold) inset; }
    tr.seed td { text-align: center; color: var(--blue); font-weight: 700; font-size: 10.5px; letter-spacing: .3px; background: rgba(106,125,255,.06); }
    tr.new td { animation: rowin .35s ease-out; }
    @keyframes rowin { from { background: rgba(255,255,255,.07); } }
    .empty { padding: 30px; text-align: center; color: var(--dim); font-weight: 600; }

    /* ---------- stats window */
    .sbody { flex: 1; min-height: 0; overflow: auto; padding: 10px; }
    .sgrid { display: grid; grid-template-columns: 1fr 1fr; gap: 6px; }
    .wd { background: var(--bg1); border: 1px solid var(--line); border-radius: 10px; padding: 8px 10px; min-width: 0; }
    .wd > span { display: block; color: var(--dim); font-size: 10px; font-weight: 700; text-transform: uppercase; letter-spacing: .6px; margin-bottom: 2px; }
    .wd > b { display: block; font-size: 13px; font-weight: 700; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; font-variant-numeric: tabular-nums; }
    .wd > i { display: block; font-style: normal; color: var(--muted); font-size: 11px; font-weight: 600; font-variant-numeric: tabular-nums; }
    .wd.wide { grid-column: span 2; }
    .wd.hero { background: radial-gradient(120% 160% at 0% 0%, rgba(0,245,160,.12), transparent 60%), var(--bg1); }
    .wd.hero.neg { background: radial-gradient(120% 160% at 0% 0%, rgba(255,61,99,.13), transparent 60%), var(--bg1); }
    .wd.hero > b { font-size: 20px; font-weight: 800; }
    .pos { color: var(--green); } .neg { color: var(--red); } .gold { color: var(--gold); }
    .chartbox { height: 150px; padding: 0; position: relative; overflow: hidden; }
    .chartbox canvas { position: absolute; inset: 0; width: 100%; height: 100%; }
    .tags { display: flex; flex-wrap: wrap; gap: 4px; margin-top: 3px; }
    .tags span { font-size: 10.5px; font-weight: 700; padding: 2px 6px; border-radius: 5px; background: var(--bg3); font-variant-numeric: tabular-nums; }
    .tags.pos span { background: rgba(24,230,142,.1); } .tags.neg span { background: rgba(255,61,99,.1); }
    .tags.gold span { background: rgba(255,194,61,.1); }
    .tags:empty::after { content: "–"; color: var(--dim); font-weight: 700; }
  `;

  const quick = [1.5, 2, 3, 5, 10, 100, 1000];
  const HTML = `
  <section class="win ctrl" data-win="ctrl">
    <header class="win-hd">
      <div class="brand"><span class="logo">${I.bolt}</span><span class="brand-t">FAST<b>BET</b></span></div>
      <span class="pill" id="statusPill"><i class="dot"></i><span id="statusTxt">Connecting</span></span>
      <div class="tools">
        <button class="tb" data-toggle="live" title="Live window">${I.live}</button>
        <button class="tb" data-toggle="stats" title="Stats window">${I.chart}</button>
        <button class="tb" id="arrange" title="Arrange windows">${I.layout}</button>
        <button class="tb" data-act="min" title="Minimize">${I.minus}</button>
        <button class="tb" data-act="close" title="Hide everything (toolbar icon brings it back)">${I.x}</button>
      </div>
    </header>
    <div class="win-bd">
      <div class="balance">
        <div><div class="lbl">Balance</div><div class="bal-v" id="bal">—</div></div>
        <div class="bal-usd" id="balusd"></div>
      </div>
      <div class="tabs" id="tabs"><span class="tab-ind"></span><button data-mode="manual">Manual</button><button data-mode="auto">Automatic</button></div>
      <div class="scroll">
        <div class="card">
          <div class="row2">
            <label class="f">Currency<select id="cur" data-lock></select></label>
            <div class="f">Unit<div class="seg"><button data-unit="coin" data-lock>COIN</button><button data-unit="usd" data-lock>USD</button></div></div>
          </div>
          <label class="f"><span class="fl">Bet size <em id="amtconv"></em></span>
            <div class="ig"><input data-k="amount" data-lock inputmode="decimal" placeholder="0.00" autocomplete="off"><span class="unit" id="amtUnit"></span>
              <button class="mini" id="half" data-lock>½</button><button class="mini" id="dbl" data-lock>2×</button></div>
          </label>
          <label class="f"><span class="fl">Target <em id="chance"></em></span>
            <div class="ig"><input data-k="target" data-lock inputmode="decimal" autocomplete="off"><span class="unit">×</span></div>
          </label>
          <div class="quick">${quick.map((q) => `<button data-t="${q}" data-lock>${q.toLocaleString("en-US")}×</button>`).join("")}</div>
        </div>

        <div class="sec" data-sec="strategy" data-only="auto">
          <button class="sec-hd">${I.trend}On win / on loss<span class="badge" id="b-strategy"></span>${I.chev.replace("<svg", '<svg class="chev"')}</button>
          <div class="sec-bd">
            <label class="f">On win
              <div class="ig"><select data-k="onWin" data-lock><option value="reset">Reset to base bet</option><option value="increase">Change bet by %</option></select>
              <input class="pct" data-k="onWinPct" data-lock inputmode="decimal"><span class="unit">%</span></div></label>
            <label class="f">On loss
              <div class="ig"><select data-k="onLoss" data-lock><option value="reset">Reset to base bet</option><option value="increase">Change bet by %</option></select>
              <input class="pct" data-k="onLossPct" data-lock inputmode="decimal"><span class="unit">%</span></div></label>
            <div class="note">Change by % works on the current bet: on loss +100% doubles it (martingale), -50% halves it.</div>
          </div>
        </div>

        <div class="sec" data-sec="count" data-only="auto">
          <button class="sec-hd">${I.hash}Bets &amp; speed<span class="badge" id="b-count"></span>${I.chev.replace("<svg", '<svg class="chev"')}</button>
          <div class="sec-bd">
            <div class="row2">
              <label class="f"><span class="fl">Bets<em>0 = ∞</em></span><input data-k="count" data-lock inputmode="numeric"></label>
              <label class="f"><span class="fl">Delay<em>ms</em></span><input data-k="delay" data-lock inputmode="numeric"></label>
            </div>
            <div class="note">chips.gg takes one bet per account at a time, so bets go back to back as fast as your ping allows. If the server says "please wait", FastBet waits a moment and retries the same bet.</div>
          </div>
        </div>

        <div class="sec" data-sec="stops" data-only="auto">
          <button class="sec-hd">${I.stop}Stop conditions<span class="badge" id="b-stops"></span>${I.chev.replace("<svg", '<svg class="chev"')}</button>
          <div class="sec-bd">
            <div class="row3">
              <label class="f">Stop profit<input data-k="stopProfit" data-lock inputmode="decimal" placeholder="off"></label>
              <label class="f">Stop loss<input data-k="stopLoss" data-lock inputmode="decimal" placeholder="off"></label>
              <label class="f">Max bet<input data-k="maxBet" data-lock inputmode="decimal" placeholder="off"></label>
            </div>
            <div class="swrow"><div>Stop on hit<small>Stops the first time a result reaches the hit multiplier</small></div>
              <label class="sw"><input type="checkbox" data-b="stopOnHit"><i></i></label></div>
            <label class="f"><span class="fl">Hit multiplier <em>empty = your target</em></span>
              <div class="ig"><input data-k="hitMulti" inputmode="decimal" placeholder="= target" autocomplete="off"><span class="unit">×</span></div></label>
            <div class="swrow"><div>Sound on hit</div><label class="sw"><input type="checkbox" data-b="hitSound"><i></i></label></div>
          </div>
        </div>

        <div class="sec" data-sec="seed">
          <button class="sec-hd">${I.seed}Auto seed change<span class="badge" id="b-seed"></span>${I.chev.replace("<svg", '<svg class="chev"')}</button>
          <div class="sec-bd">
            <div class="swrow"><div>Change seed automatically<small>New random client seed, nonce resets to 0</small></div>
              <label class="sw"><input type="checkbox" data-b="seedEnabled"><i></i></label></div>
            <label class="f">Change every (bets)<input data-k="seedEvery" inputmode="numeric" placeholder="100"></label>
            <div class="seedinfo">
              <div class="kv wide"><span>Client seed</span><b id="seedCs">–</b></div>
              <div class="kv"><span>Nonce</span><b id="seedNonce">–</b></div>
              <div class="kv"><span>Next change in</span><b id="seedNext">–</b></div>
            </div>
            <button class="ghost" id="rotateNow">${I.seed}Change seed now<span class="note" id="seedRot"></span></button>
          </div>
        </div>

        <div class="sec" data-sec="keys">
          <button class="sec-hd">${I.keys}Hotkeys<span class="badge" id="b-keys"></span>${I.chev.replace("<svg", '<svg class="chev"')}</button>
          <div class="sec-bd">
            <div class="swrow"><div>Enable hotkeys</div><label class="sw"><input type="checkbox" data-b="hotkeys"><i></i></label></div>
            <div class="keys"><kbd>Space</kbd><span>Place one bet</span><kbd>P</kbd><span>Pause / resume auto bet</span><kbd>Esc</kbd><span>Stop auto bet</span></div>
            <div class="note">Ignored while typing in a text field.</div>
          </div>
        </div>
      </div>
      <div class="foot">
        <button class="go" id="go">BET</button>
        <div class="automsg" id="automsg"></div>
        <div class="err" id="err"></div>
      </div>
    </div>
  </section>

  <section class="win live resizable" data-win="live">
    <header class="win-hd">
      <div class="wt">${I.live}<span>Limbo</span></div>
      <span class="hstat" title="Session time">${I.clock}<span id="timer">00:00:00</span></span>
      <span class="hstat" title="Bets this session">${I.hash}<span id="betCount">0</span></span>
      <div class="tools">
        <button class="tb" id="clearHist" title="Clear history">${I.trash}</button>
        <button class="tb" data-act="min" title="Minimize">${I.minus}</button>
        <button class="tb" data-act="close" title="Close">${I.x}</button>
      </div>
    </header>
    <div class="win-bd">
      <div class="stage" id="stage">
        <div class="glow" id="glow"></div>
        <div class="result" id="result">0.00×</div>
        <div class="rsub" id="rsub">Waiting for your first bet</div>
        <div class="pausedtag">PAUSED</div>
        <div class="hitbadge" id="hitBadge">HIT</div>
        <div class="stagectl">
          <button class="sbtn off" id="hitToggle">${I.target}<span id="hitVal">≥ 2.00×</span></button>
          <button class="sbtn" id="pauseBtn" data-state="pause" disabled>${I.pause}</button>
        </div>
      </div>
      <div class="ribbon" id="ribbon"></div>
      <div class="tblwrap">
        <table>
          <thead><tr><th>#</th><th>Nonce</th><th>Time</th><th>Target</th><th>Result</th><th>Bet</th><th>Payout</th><th>Profit</th></tr></thead>
          <tbody id="rows"></tbody>
        </table>
        <div class="empty" id="empty">No bets yet</div>
      </div>
    </div>
  </section>

  <section class="win stats resizable" data-win="stats">
    <header class="win-hd">
      <div class="wt">${I.chart}<span>Stats</span></div>
      <div class="tools">
        <button class="tb" id="resetStats" title="Reset stats">${I.reset}</button>
        <button class="tb" data-act="min" title="Minimize">${I.minus}</button>
        <button class="tb" data-act="close" title="Close">${I.x}</button>
      </div>
    </header>
    <div class="win-bd">
      <div class="sbody">
        <div class="sgrid">
          <div class="wd"><span>Bet speed</span><b id="st-speed">0/s</b><i id="st-speed2">0/m</i></div>
          <div class="wd"><span>Streak</span><b id="st-streak">–</b><i id="st-streak2"></i></div>
          <div class="wd wide hero" id="heroProfit"><span>Profit</span><b id="st-profit">0</b><i id="st-profit2"></i></div>
          <div class="wd"><span>Wins</span><b class="pos" id="st-wins">0</b><i id="st-winpct"></i></div>
          <div class="wd"><span>Losses</span><b class="neg" id="st-losses">0</b><i id="st-hits"></i></div>
          <div class="wd"><span>Wagered</span><b id="st-wagered">0</b><i id="st-wagered2"></i></div>
          <div class="wd"><span>RTP</span><b id="st-rtp">–</b><i>returned / wagered</i></div>
          <div class="wd wide chartbox"><canvas id="chart"></canvas></div>
          <div class="wd"><span>Max drawdown</span><b class="neg" id="st-dd">0</b></div>
          <div class="wd"><span>Highest profit</span><b class="pos" id="st-peak">0</b></div>
          <div class="wd"><span>Lowest</span><b class="neg" id="st-low">0</b></div>
          <div class="wd"><span>Highest bet</span><b id="st-hibet">0</b></div>
          <div class="wd"><span>Highest win</span><b class="pos" id="st-hiwin">0</b></div>
          <div class="wd"><span>Highest loss</span><b class="neg" id="st-hiloss">0</b></div>
          <div class="wd"><span>Ping</span><b id="st-ping">–</b></div>
          <div class="wd"><span>Queue</span><b id="st-inflight">0</b><i id="st-retries"></i></div>
          <div class="wd wide"><span>Highest multipliers</span><div class="tags gold" id="st-top"></div></div>
          <div class="wd"><span>Best win streaks</span><div class="tags pos" id="st-ws"></div></div>
          <div class="wd"><span>Worst loss streaks</span><div class="tags neg" id="st-ls"></div></div>
        </div>
      </div>
    </div>
  </section>
  `;

  const host = document.createElement("div");
  host.id = "fastbet-root";
  host.style.cssText = "position:fixed;inset:0;pointer-events:none;z-index:2147483647;";
  const root = host.attachShadow({ mode: "open" });
  root.innerHTML = `<style>${CSS}</style>${HTML}`;
  const $ = (id) => root.getElementById(id);
  const $$ = (sel) => root.querySelectorAll(sel);

  function mount() {
    if (!host.isConnected) document.documentElement.appendChild(host);
  }
  mount();
  // Keep the UI alive if the SPA ever rebuilds the document.
  setInterval(mount, 3000);

  // ------------------------------------------------------------- render
  let renderQueued = false;
  function render() {
    if (renderQueued) return;
    renderQueued = true;
    requestAnimationFrame(() => {
      renderQueued = false;
      draw();
    });
  }

  let curSig = "";
  let drawnHist = -1;
  let drawnStats = -1;
  let chartDirty = true;
  let lastShown = null;
  let lastHit = null;
  let lastPop = 0;
  let goHtml = "";

  // ------------------------------------------------------------ windows
  const WIN_IDS = ["ctrl", "live", "stats"];
  const MIN_W = { ctrl: 336, live: 380, stats: 250 };
  const winEl = {};
  for (const id of WIN_IDS) winEl[id] = root.querySelector(`[data-win="${id}"]`);
  const G = settings.wins;
  const zOrder = [...WIN_IDS];

  function front(id) {
    zOrder.splice(zOrder.indexOf(id), 1);
    zOrder.push(id);
    zOrder.forEach((w, i) => (winEl[w].style.zIndex = String(i + 1)));
  }

  function arrange() {
    const m = 12;
    const top = Math.round(clamp(innerHeight * 0.075, m, 76));
    const W = innerWidth;
    const H = innerHeight - top - m;
    const sw = W >= 1300 ? 310 : 270;
    const cw = MIN_W.ctrl;
    const lx = m + cw + 10;
    G.ctrl = { x: m, y: top, show: true, min: false };
    G.stats = { x: W - sw - m, y: top, w: sw, h: H, show: true, min: false };
    G.live = { x: lx, y: top, w: Math.max(MIN_W.live, W - lx - sw - m - 10), h: H, show: true, min: false };
    saveSettings();
    placeAll();
  }

  function place(id) {
    const el = winEl[id];
    const g = G[id];
    const W = innerWidth;
    const H = innerHeight;
    el.style.display = !settings.hidden && g.show ? "" : "none";
    el.classList.toggle("min", !!g.min);
    if (id === "ctrl") {
      el.style.maxHeight = Math.max(200, H - clamp(g.y, 0, H) - 10) + "px";
      el.style.height = g.min ? "" : el.style.maxHeight;
    } else if (!g.min) {
      g.w = clamp(g.w || 400, MIN_W[id], Math.max(MIN_W[id], W));
      g.h = clamp(g.h || 400, 160, Math.max(160, H));
      el.style.width = g.w + "px";
      el.style.height = g.h + "px";
    } else {
      el.style.width = (g.w || 400) + "px";
    }
    g.x = clamp(g.x, 0, Math.max(0, W - 90));
    g.y = clamp(g.y, 0, Math.max(0, H - 44));
    el.style.left = g.x + "px";
    el.style.top = g.y + "px";
  }

  function placeAll() {
    for (const id of WIN_IDS) place(id);
    render();
  }

  if (!G.ctrl || !G.live || !G.stats) arrange();

  for (const id of WIN_IDS) {
    const el = winEl[id];
    const hd = el.querySelector(".win-hd");
    el.addEventListener("pointerdown", () => front(id), true);
    hd.addEventListener("pointerdown", (e) => {
      if (e.button !== 0 || e.target.closest("button")) return;
      const g = G[id];
      const sx = e.clientX - g.x;
      const sy = e.clientY - g.y;
      hd.setPointerCapture(e.pointerId);
      const mv = (ev) => {
        g.x = ev.clientX - sx;
        g.y = ev.clientY - sy;
        place(id);
      };
      const up = () => {
        hd.removeEventListener("pointermove", mv);
        hd.removeEventListener("pointerup", up);
        hd.removeEventListener("pointercancel", up);
        saveSettings();
      };
      hd.addEventListener("pointermove", mv);
      hd.addEventListener("pointerup", up);
      hd.addEventListener("pointercancel", up);
    });
    el.querySelector('[data-act="min"]').addEventListener("click", () => {
      G[id].min = !G[id].min;
      saveSettings();
      placeAll();
    });
    el.querySelector('[data-act="close"]').addEventListener("click", () => {
      if (id === "ctrl") return toggle(false);
      G[id].show = false;
      saveSettings();
      placeAll();
    });
  }
  front("ctrl");

  // remember sizes after the user drags the resize corner
  let sizeTimer;
  const ro = new ResizeObserver(() => {
    for (const id of ["live", "stats"]) {
      const el = winEl[id];
      if (G[id].min || el.style.display === "none") continue;
      G[id].w = el.offsetWidth;
      G[id].h = el.offsetHeight;
    }
    chartDirty = true;
    render();
    clearTimeout(sizeTimer);
    sizeTimer = setTimeout(saveSettings, 300);
  });
  ro.observe(winEl.live);
  ro.observe(winEl.stats);
  addEventListener("resize", placeAll);

  for (const b of $$("[data-toggle]")) {
    b.addEventListener("click", () => {
      const g = G[b.dataset.toggle];
      g.show = !g.show;
      if (g.show) {
        g.min = false;
        front(b.dataset.toggle);
      }
      saveSettings();
      placeAll();
    });
  }
  $("arrange").addEventListener("click", arrange);

  function toggle(show = settings.hidden) {
    settings.hidden = !show;
    saveSettings();
    placeAll();
  }

  // ------------------------------------------------------------ bindings
  for (const el of $$("[data-k]")) {
    el.value = settings[el.dataset.k];
    el.addEventListener("input", () => {
      settings[el.dataset.k] = el.value;
      saveSettings();
      if (el.dataset.k === "hitMulti" || el.dataset.k === "target") histVer++;
      render();
    });
  }
  for (const el of $$("[data-b]")) {
    el.checked = !!settings[el.dataset.b];
    el.addEventListener("change", () => {
      settings[el.dataset.b] = el.checked;
      if (el.dataset.b === "seedEnabled" && el.checked) seed.count = 0;
      saveSettings();
      render();
    });
  }
  const setField = (k, v) => {
    settings[k] = v;
    const el = root.querySelector(`[data-k="${k}"]`);
    if (el) el.value = v;
    saveSettings();
    render();
  };

  for (const s of $$(".sec")) {
    s.querySelector(".sec-hd").addEventListener("click", () => {
      settings.open[s.dataset.sec] = !settings.open[s.dataset.sec];
      saveSettings();
      render();
    });
  }

  for (const b of $$("[data-mode]")) {
    b.addEventListener("click", () => {
      settings.mode = b.dataset.mode;
      saveSettings();
      render();
    });
  }

  $("cur").addEventListener("change", (e) => {
    settings.currency = e.target.value;
    saveSettings();
    render();
  });

  for (const b of $$("[data-unit]")) {
    b.addEventListener("click", () => {
      if (settings.unit === b.dataset.unit || auto.running) return;
      // convert the typed amount so the stake stays the same
      const cur = activeCurrency();
      const base = toBase(settings.amount, cur);
      settings.unit = b.dataset.unit;
      histVer++;
      statVer++;
      if (base != null && curInfo(cur)) {
        if (settings.unit === "usd") {
          const usd = usdValue(base, cur);
          setField("amount", usd == null ? "" : String(+usd.toFixed(4)));
        } else {
          setField("amount", formatUnits(base, curInfo(cur).decimals, curInfo(cur).decimals));
        }
      }
      saveSettings();
      render();
      b.blur();
    });
  }

  function adjustAmount(factor) {
    const info = curInfo(activeCurrency());
    if (settings.unit === "usd") {
      const v = num(settings.amount) || 0;
      setField("amount", String(+(v * factor).toFixed(4)));
    } else if (info) {
      const base = parseUnits(settings.amount, info.decimals) || 0n;
      setField("amount", formatUnits(factor > 1 ? base * 2n : base / 2n, info.decimals, info.decimals));
    }
  }
  $("half").addEventListener("click", (e) => (adjustAmount(0.5), e.currentTarget.blur()));
  $("dbl").addEventListener("click", (e) => (adjustAmount(2), e.currentTarget.blur()));
  for (const b of $$("[data-t]")) {
    b.addEventListener("click", () => {
      setField("target", b.dataset.t);
      b.blur();
    });
  }

  $("go").addEventListener("click", (e) => {
    e.currentTarget.blur();
    if (auto.running) stopAuto();
    else if (settings.mode === "auto") startAuto();
    else manualBet();
  });
  $("rotateNow").addEventListener("click", (e) => {
    e.currentTarget.blur();
    rotateNow();
  });
  $("resetStats").addEventListener("click", () => resetStats());
  $("hitToggle").addEventListener("click", (e) => {
    e.currentTarget.blur();
    settings.stopOnHit = !settings.stopOnHit;
    root.querySelector('[data-b="stopOnHit"]').checked = settings.stopOnHit;
    saveSettings();
    render();
  });
  $("pauseBtn").addEventListener("click", (e) => {
    e.currentTarget.blur();
    togglePause();
  });
  $("clearHist").addEventListener("click", () => {
    history.length = 0;
    histVer++;
    lastShown = null;
    render();
  });

  // ---- hotkeys (capture phase so the site's own hotkeys / focused buttons never see them)
  const isTyping = (el) =>
    !!el && (el.tagName === "INPUT" || el.tagName === "TEXTAREA" || el.tagName === "SELECT" || el.isContentEditable);

  addEventListener(
    "keydown",
    (e) => {
      if (!settings.hotkeys || settings.hidden || e.ctrlKey || e.metaKey || e.altKey) return;
      if (isTyping(e.composedPath()[0])) return;
      if (e.code === "Space") {
        e.preventDefault();
        e.stopImmediatePropagation();
        if (!e.repeat) manualBet();
      } else if (e.code === "Escape" && auto.running) {
        e.preventDefault();
        e.stopImmediatePropagation();
        stopAuto("Stopped (Esc)");
      } else if (e.code === "KeyP" && auto.running && !e.repeat) {
        e.preventDefault();
        e.stopImmediatePropagation();
        togglePause();
      }
    },
    true
  );
  addEventListener(
    "keyup",
    (e) => {
      if (!settings.hotkeys || settings.hidden || e.code !== "Space") return;
      if (isTyping(e.composedPath()[0])) return;
      e.preventDefault();
      e.stopImmediatePropagation();
    },
    true
  );

  // ---- messages
  let flashTimer;
  function flash(msg) {
    $("err").textContent = msg;
    clearTimeout(flashTimer);
    flashTimer = setTimeout(() => ($("err").textContent = ""), 6000);
  }

  function statusText() {
    switch (net.status) {
      case "connecting":
        return "Connecting…";
      case "auth":
        return "Logging in…";
      case "ready":
        return "Ready";
      case "nologin":
        return "Not logged in — log in to chips.gg first";
      case "authfail":
        return "Login failed: " + net.detail;
      case "offline":
        return "Disconnected — reconnecting…";
    }
    return net.status;
  }
  const shortStatus = () =>
    ({ connecting: "Connecting", auth: "Logging in", ready: "Ready", nologin: "Log in", authfail: "Login failed", offline: "Offline" })[
      net.status
    ] || net.status;


  function draw() {
    if (settings.hidden) {
      for (const id of WIN_IDS) winEl[id].style.display = "none";
      return;
    }
    drawCtrl();
    if (G.live.show && !G.live.min) drawLive();
    if (G.stats.show && !G.stats.min) drawStats();
    for (const b of $$("[data-toggle]")) b.classList.toggle("on", !!G[b.dataset.toggle].show);
  }

  function drawCtrl() {
    const ok = net.status === "ready";
    const bad = ["nologin", "authfail", "offline"].includes(net.status);
    $("statusPill").className = "pill" + (ok ? " ok" : bad ? " bad" : "");
    $("statusTxt").textContent = shortStatus();
    $("statusPill").title = statusText();

    // currency list
    const list = Object.entries(currencies())
      .filter(([, c]) => c && !c.hidden)
      .map(([k]) => k);
    const siteCur = me().currency;
    const sig = list.join(",") + "|" + settings.currency + "|" + (siteCur || "");
    if (sig !== curSig) {
      curSig = sig;
      const sel = $("cur");
      sel.innerHTML = "";
      sel.add(new Option(`Site (${siteCur ? siteCur.toUpperCase() : "…"})`, ""));
      for (const k of list) sel.add(new Option(k.toUpperCase(), k));
      if (settings.currency && !list.includes(settings.currency)) {
        sel.add(new Option(settings.currency.toUpperCase(), settings.currency));
      }
      sel.value = settings.currency;
    }

    const cur = activeCurrency();
    const info = curInfo(cur);
    const C = cur ? cur.toUpperCase() : "";

    const w = wallets()[cur];
    const bal = w ? toBig(w.balance) : null;
    if (bal != null && info) {
      $("bal").textContent = prim(bal, cur);
      $("balusd").textContent = sec(bal, cur);
    } else {
      $("bal").textContent = cur ? `— ${C}` : "—";
      $("balusd").textContent = "";
    }

    $("tabs").dataset.mode = settings.mode;
    for (const b of $$("[data-mode]")) b.classList.toggle("on", b.dataset.mode === settings.mode);
    for (const b of $$("[data-unit]")) {
      b.classList.toggle("on", b.dataset.unit === settings.unit);
      if (b.dataset.unit === "coin") b.textContent = C || "COIN";
    }
    $("amtUnit").textContent = usdMode() ? "USD" : C;

    const base = toBase(settings.amount, cur);
    $("amtconv").textContent = base != null && info ? "≈ " + sec(base, cur) : "";
    const t = num(settings.target);
    const tOk = t >= MIN_TARGET && t <= MAX_TARGET;
    let ch = tOk ? `${(99 / t).toFixed(t > 9900 ? 4 : 2)}% win` : `${MIN_TARGET}–${MAX_TARGET.toLocaleString("en-US")}×`;
    if (tOk && base != null && info) ch += ` · +${prim((base * BigInt(Math.floor(t * 1e6))) / 1000000n - base, cur)}`;
    $("chance").textContent = ch;
    for (const b of $$("[data-t]")) b.classList.toggle("on", num(b.dataset.t) === t);

    for (const s of $$(".sec")) {
      s.classList.toggle("open", !!settings.open[s.dataset.sec]);
      s.style.display = s.dataset.only && s.dataset.only !== settings.mode ? "none" : "";
    }
    const badge = (id, on, text) => {
      const b = $(id);
      b.textContent = text;
      b.className = "badge" + (on ? " on" : "");
    };
    const strat = settings.onWin !== "reset" || settings.onLoss !== "reset";
    badge("b-strategy", strat, strat ? "ON" : "FLAT");
    const cnt = parseInt(settings.count, 10) || 0;
    const dl = Math.max(0, num(settings.delay) || 0);
    badge("b-count", false, `${cnt ? cnt : "∞"}${dl ? ` · ${dl}ms` : ""}`);
    const nStops = [settings.stopProfit, settings.stopLoss, settings.maxBet].filter((v) => String(v).trim()).length + (settings.stopOnHit ? 1 : 0);
    badge("b-stops", nStops > 0, settings.stopOnHit ? "HIT" + (nStops > 1 ? ` +${nStops - 1}` : "") : nStops ? String(nStops) : "OFF");
    badge("b-seed", settings.seedEnabled, settings.seedEnabled ? `/${seedEvery() || "?"}` : "OFF");
    badge("b-keys", settings.hotkeys, settings.hotkeys ? "ON" : "OFF");

    // seed info
    const st = seed.state || {};
    $("seedCs").textContent = st.clientSeed || "–";
    $("seedCs").title = st.clientSeed || "";
    $("seedNonce").textContent = st.nonce != null ? String(st.nonce) : "–";
    const every = seedEvery();
    $("seedNext").textContent = seed.busy ? "changing…" : every ? `${Math.max(0, every - seed.count)} bets` : "off";
    $("seedRot").textContent = seed.rotations ? `(${seed.rotations} this session)` : "";
    $("rotateNow").disabled = !ok || seed.busy;

    // controls
    for (const el of $$("[data-lock]")) el.disabled = auto.running;
    root.querySelector('[data-k="onWinPct"]').disabled = auto.running || settings.onWin === "reset";
    root.querySelector('[data-k="onLossPct"]').disabled = auto.running || settings.onLoss === "reset";
    const go = $("go");
    const label = auto.running ? "STOP AUTO <small>Esc</small>" : settings.mode === "auto" ? "START AUTO" : "BET <small>Space</small>";
    // only touch the DOM on change, or a click landing mid-swap gets lost
    if (label !== goHtml) go.innerHTML = goHtml = label;
    go.classList.toggle("stop", auto.running);
    go.disabled = !auto.running && !ok;

    let am = "";
    if (auto.cur && (auto.running || auto.done)) {
      am = `${auto.msg} · ${auto.done}${auto.limit ? "/" + auto.limit : ""} bets · ${prim(auto.profit, auto.cur, true)}`;
      if (auto.running && (auto.onWin != null || auto.onLoss != null)) am += ` · next ${prim(auto.stake, auto.cur)}`;
    } else if (auto.msg) am = auto.msg;
    const msgEl = $("automsg");
    msgEl.textContent = am;
    msgEl.className = "automsg" + (!auto.running && auto.msg.startsWith("HIT") ? " hit" : "");
  }

  function drawLive() {
    const s = stats;
    $("timer").textContent = fmtTime(s.started ? (auto.running ? Date.now() : s.lastAt) - s.started : 0);
    $("betCount").textContent = s.bets.toLocaleString("en-US");

    const hb = $("hitToggle");
    const line = num(settings.hitMulti) >= MIN_TARGET ? num(settings.hitMulti) : num(settings.target);
    $("hitVal").textContent = line >= MIN_TARGET ? `≥ ${fmtMulti(line)}` : "≥ ?";
    hb.className = "sbtn " + (settings.stopOnHit ? "on" : "off");
    hb.title = settings.stopOnHit ? "Stop on hit is ON: click to turn it off" : "Stop on hit is OFF: click to turn it on";
    const pb = $("pauseBtn");
    const state = auto.paused ? "play" : "pause";
    if (pb.dataset.state !== state) {
      pb.dataset.state = state;
      pb.innerHTML = auto.paused ? I.play : I.pause;
    }
    pb.disabled = !auto.running;
    pb.classList.toggle("paused", !!auto.paused);
    pb.title = !auto.running ? "Pause (start auto bet first)" : auto.paused ? "Resume auto bet (P)" : "Pause auto bet (P)";
    $("stage").classList.toggle("paused", !!auto.paused);

    if (drawnHist === histVer) return;
    drawnHist = histVer;

    const latest = history.find((h) => !h.seed);
    if (latest && latest !== lastShown) {
      lastShown = latest;
      const kind = latest.hit ? "h" : latest.win ? "w" : "l";
      const r = $("result");
      r.textContent = fmtMulti(latest.outcome);
      r.className = "result " + kind;
      const t = performance.now();
      if (t - lastPop > 120) {
        void r.offsetWidth;
        r.classList.add("pop");
      }
      lastPop = t;
      $("glow").className = "glow " + kind;
      const pc = latest.profit > 0n ? "pos" : latest.profit < 0n ? "neg" : "";
      $("rsub").innerHTML = `Target ${fmtMulti(latest.target)} · Bet ${esc(prim(latest.amount, latest.cur))} · <b class="${pc}">${esc(
        prim(latest.profit, latest.cur, true)
      )}</b> · ${Math.round(latest.ms)}ms`;
    } else if (!latest) {
      lastShown = null;
      $("result").textContent = "0.00×";
      $("result").className = "result";
      $("glow").className = "glow";
      $("rsub").textContent = "Waiting for your first bet";
    }
    $("hitBadge").classList.toggle("on", !!lastHit && lastHit === latest);

    // ribbon
    let chips = "";
    let n = 0;
    for (const h of history) {
      if (h.seed) continue;
      if (++n > RIBBON_MAX) break;
      chips += `<span class="chip ${h.hit ? "h" : h.win ? "w" : "l"}${h.fresh ? " new" : ""}">${fmtMulti(h.outcome)}</span>`;
    }
    $("ribbon").innerHTML = chips;

    // table
    const unitKey = settings.unit;
    let rows = "";
    for (let i = 0; i < history.length && i < TABLE_ROWS; i++) {
      const h = history[i];
      if (h.seed) {
        rows += `<tr class="seed${h.fresh !== false ? " new" : ""}"><td colspan="8">⟳ Seed changed (${esc(h.reason)}) · client seed ${esc(h.clientSeed)} · ${esc(h.t)}</td></tr>`;
        h.fresh = false;
        continue;
      }
      if (h.unitKey !== unitKey) {
        h.unitKey = unitKey;
        const pc = h.profit > 0n ? "pos" : h.profit < 0n ? "neg" : "";
        h.cells = `<td>${h.n}</td><td>${h.nonce ?? "–"}</td><td>${esc(h.t)}</td><td>${fmtMulti(h.target)}</td><td class="res">${fmtMulti(
          h.outcome
        )}</td><td>${esc(prim(h.amount, h.cur))}</td><td>${esc(prim(h.payout, h.cur))}</td><td class="${pc}">${esc(prim(h.profit, h.cur, true))}</td>`;
      }
      rows += `<tr class="${h.hit ? "h" : h.win ? "w" : "l"}${h.fresh ? " new" : ""}">${h.cells}</tr>`;
    }
    for (const h of history) h.fresh = false;
    $("rows").innerHTML = rows;
    $("empty").style.display = history.length ? "none" : "";
  }

  function drawStats() {
    const s = stats;
    const now = performance.now();
    while (s.times.length && now - s.times[0] > 60000) s.times.shift();
    let last3 = 0;
    for (let i = s.times.length - 1; i >= 0 && now - s.times[i] <= 3000; i--) last3++;
    $("st-speed").textContent = `${(last3 / 3).toFixed(1)}/s`;
    $("st-speed2").textContent = `${s.times.length.toLocaleString("en-US")}/m`;
    $("st-inflight").textContent = queued;
    $("st-retries").textContent = s.retries ? `${s.retries} busy retr${s.retries > 1 ? "ies" : "y"}` : "";
    $("st-ping").textContent = s.latency ? Math.round(s.latency) + "ms" : "–";
    if (drawnStats === statVer && !chartDirty) return;
    const cur = s.cur || activeCurrency();
    const has = !!curInfo(cur);
    const P = (v, sign) => (has ? prim(v, cur, sign) : "0");
    const S = (v, sign) => (has ? sec(v, cur, sign) : "");

    if (drawnStats !== statVer) {
      drawnStats = statVer;
      chartDirty = true;
      const streakTxt = s.streak > 0 ? `W${s.streak}` : s.streak < 0 ? `L${-s.streak}` : "–";
      $("st-streak").textContent = streakTxt;
      $("st-streak").className = s.streak > 0 ? "pos" : s.streak < 0 ? "neg" : "";
      const bestW = Math.max(s.winStreaks[0] || 0, s.streak > 0 ? s.streak : 0);
      const worstL = Math.max(s.lossStreaks[0] || 0, s.streak < 0 ? -s.streak : 0);
      $("st-streak2").textContent = `best W${bestW} · worst L${worstL}`;
      $("st-profit").textContent = P(s.profit, true);
      $("st-profit").className = s.profit > 0n ? "pos" : s.profit < 0n ? "neg" : "";
      $("heroProfit").className = "wd wide hero" + (s.profit < 0n ? " neg" : "");
      const edge = s.wagered > 0n ? Number((s.profit * 1000000n) / s.wagered) / 10000 : 0;
      $("st-profit2").textContent = `${S(s.profit, true)}${s.wagered > 0n ? ` · ${edge >= 0 ? "+" : ""}${edge.toFixed(2)}% of wagered` : ""}`;
      $("st-wins").textContent = s.wins.toLocaleString("en-US");
      $("st-losses").textContent = s.losses.toLocaleString("en-US");
      $("st-winpct").textContent = s.bets ? `${((s.wins / s.bets) * 100).toFixed(2)}%` : "";
      $("st-hits").textContent = s.hits ? `${s.hits} hit${s.hits > 1 ? "s" : ""}` : "";
      $("st-wagered").textContent = P(s.wagered);
      $("st-wagered2").textContent = S(s.wagered);
      $("st-rtp").textContent = s.wagered > 0n ? `${(Number(((s.wagered + s.profit) * 1000000n) / s.wagered) / 10000).toFixed(2)}%` : "–";
      $("st-dd").textContent = P(-s.maxDD);
      $("st-peak").textContent = P(s.peak, true);
      $("st-low").textContent = P(s.low);
      $("st-hibet").textContent = P(s.hiBet);
      $("st-hiwin").textContent = P(s.hiWin, true);
      $("st-hiloss").textContent = P(-s.hiLoss);
      $("st-top").innerHTML = s.topMulti.map((m) => `<span>${fmtMulti(m)}</span>`).join("");
      const ws = s.streak > 0 ? [...s.winStreaks] : s.winStreaks;
      if (s.streak > 0) pushTop(ws, s.streak, 8);
      const ls = s.streak < 0 ? [...s.lossStreaks] : s.lossStreaks;
      if (s.streak < 0) pushTop(ls, -s.streak, 8);
      $("st-ws").innerHTML = ws.map((v) => `<span>${v}</span>`).join("");
      $("st-ls").innerHTML = ls.map((v) => `<span>${v}</span>`).join("");
    }
    if (chartDirty) {
      chartDirty = false;
      drawChart();
    }
  }

  function drawChart() {
    const cv = $("chart");
    const w = cv.clientWidth;
    const h = cv.clientHeight;
    if (!w || !h) return;
    const dpr = devicePixelRatio || 1;
    if (cv.width !== Math.round(w * dpr) || cv.height !== Math.round(h * dpr)) {
      cv.width = Math.round(w * dpr);
      cv.height = Math.round(h * dpr);
    }
    const ctx = cv.getContext("2d");
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    ctx.clearRect(0, 0, w, h);
    const info = curInfo(stats.cur);
    const pts = stats.series.slice();
    if (info) pts.push(Number(formatUnits(stats.profit, info.decimals, info.decimals)));
    let min = 0;
    let max = 0;
    for (const v of pts) {
      if (v < min) min = v;
      if (v > max) max = v;
    }
    if (max === min) {
      max += 1;
      min -= 1;
    }
    const pad = 10;
    const X = (i) => pad + (i * (w - 2 * pad)) / Math.max(1, pts.length - 1);
    const Y = (v) => pad + ((max - v) * (h - 2 * pad)) / (max - min);
    const zy = Y(0);
    ctx.strokeStyle = "rgba(255,255,255,.16)";
    ctx.setLineDash([4, 4]);
    ctx.lineWidth = 1;
    ctx.beginPath();
    ctx.moveTo(0, zy);
    ctx.lineTo(w, zy);
    ctx.stroke();
    ctx.setLineDash([]);
    if (pts.length < 2) return;
    const line = new Path2D();
    pts.forEach((v, i) => (i ? line.lineTo(X(i), Y(v)) : line.moveTo(X(i), Y(v))));
    const area = new Path2D(line);
    area.lineTo(X(pts.length - 1), zy);
    area.lineTo(X(0), zy);
    area.closePath();
    for (const [color, above] of [
      ["#18e68e", true],
      ["#ff3d63", false],
    ]) {
      ctx.save();
      ctx.beginPath();
      if (above) ctx.rect(0, 0, w, zy);
      else ctx.rect(0, zy, w, h - zy);
      ctx.clip();
      const grad = ctx.createLinearGradient(0, above ? Y(max) : Y(min), 0, zy);
      grad.addColorStop(0, color + "55");
      grad.addColorStop(1, color + "00");
      ctx.fillStyle = grad;
      ctx.fill(area);
      ctx.strokeStyle = color;
      ctx.lineWidth = 1.7;
      ctx.shadowColor = color;
      ctx.shadowBlur = 8;
      ctx.stroke(line);
      ctx.restore();
    }
    const lx = X(pts.length - 1);
    const lv = pts[pts.length - 1];
    ctx.fillStyle = lv >= 0 ? "#18e68e" : "#ff3d63";
    ctx.beginPath();
    ctx.arc(lx, Y(lv), 3, 0, Math.PI * 2);
    ctx.fill();
  }

  // live counters (speed, timer) keep moving between bets
  setInterval(() => (auto.running || stats.times.length) && render(), 500);

  window.__FASTBET__ = { toggle, arrange, net, settings, stats, history, auto, seed, call, get queued() { return queued; } };
  placeAll();
  connect();
})();
