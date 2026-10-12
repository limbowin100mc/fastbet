//! Spinomenal driver. The launcher ends on `generic-play.html?partnerId&launchToken&
//! gameCode&langCode&IsFunMode&inter&InitClientUrl&PlatformId`. JSON POSTs:
//!   {InitClientUrl}  {partnerId, gameCode, langCode, IntegrationTypeId} -> Result.Urls (every
//!                    endpoint below), GameInfo.BuyFeatures / BuyBonusOptions
//!   StartUrl         {gameToken = launchToken, …} -> Token (the session token every later
//!                    call carries), Coins (balance), BettingUnits (bet per line),
//!                    DefaultBetUnit, Lines, ClassId, RtpVariantId, CurrencyCode
//!   ProcessUrl       {bet, lines, …} -> Result{TotalBetPlaced, Win, CoinIn (after the debit),
//!                    CoinOut, IsRoundFinished, FreeTickets{PendingAction, Id, From, To, Win}}
//!   BuyFeatureUrl    {bet, lines, freeSpinType, freeSpinAmount, buyFeatureType} -> Result{
//!                    FreeTickets{PendingAction, PurchasePrice}, CoinOut}
//!   PickFreeSpinsUrl {ticketId} (after any ticket whose FreeTickets.PendingAction is set)
//! Hold-and-hit style buys (BuyBonusOptions) are a ProcessUrl spin with
//! `BuyFeatureBonusType`, settled in the same ticket. A round goes on while
//! `IsRoundFinished` is false (free spins are plain ProcessUrl calls). The "1 Reel"
//! tower games (`Tower_…`) are different: an open round is a bank the player
//! keeps paying spins into until TowerCollectUrl; autoplay banks after every spin.

use async_trait::async_trait;
use serde_json::{json, Value};

use super::*;

pub struct SpinomenalSession {
    client: reqwest::Client,
    page: String,
    urls: Value,
    /// Fields every request carries (token, game, partner, class…).
    common: Value,
    lines: i64,
    /// Bet per line for each level, as the server spells it.
    units: Vec<f64>,
    buys: Vec<Buy>,
    /// A "1 Reel" tower game (collect after each spin).
    tower: bool,
    info: SessionInfo,
}

#[derive(Clone, Debug)]
enum Buy {
    /// BuyFeatureUrl with these fields.
    FreeSpins(Value),
    /// A ProcessUrl spin with `BuyFeatureBonusType`.
    Bonus(i64),
}

pub async fn open(launch_url: &str) -> Result<SpinomenalSession, SlotError> {
    let page = resolve_launcher(launch_url).await.map_err(SlotError::Bridge)?;
    let q = super::hacksaw::query_map(&page);
    let need = |k: &str| q.get(k).cloned().ok_or_else(|| SlotError::Provider(format!("No {k} in the Spinomenal launch")));
    let (init_url, token, game, partner) = (need("InitClientUrl")?, need("launchToken")?, need("gameCode")?, need("partnerId")?);
    let lang = q.get("langCode").cloned().unwrap_or_else(|| "en_US".into());
    let platform: i64 = q.get("PlatformId").and_then(|p| p.parse().ok()).unwrap_or(1);
    let client = reqwest::Client::builder().user_agent(UA).cookie_store(true).gzip(true).build().map_err(|e| SlotError::Bridge(e.to_string()))?;
    let tower = game.starts_with("Tower_");
    let mut s = SpinomenalSession { client, page, urls: Value::Null, common: Value::Null, lines: 1, units: vec![], buys: vec![], tower, info: SessionInfo { decimals: 2, ..Default::default() } };

    let integration: i64 = q.get("inter").and_then(|i| i.parse().ok()).unwrap_or(18);
    let init = s.post(&init_url, &json!({ "partnerId": partner, "gameCode": game, "langCode": lang, "IntegrationTypeId": integration })).await?;
    s.urls = init["Result"]["Urls"].clone();
    // real launch tokens only start a session as they are (the demo client adds a random
    // suffix, which real-money sessions reject as TOKEN_EXPIRED)
    let mut common = json!({ "gameCode": game, "gameToken": token, "langCode": lang, "partnerId": partner, "playerId": 0, "originalLangCode": lang, "platformId": platform, "subPlatformId": 0 });
    let start = s.post(&s.url("StartUrl")?, &common).await?;
    let r = &start["Result"];
    if let Some(t) = r["Token"].as_str().filter(|t| !t.is_empty()) {
        common["gameToken"] = json!(t);
    }
    s.lines = r["Lines"].as_i64().or(r["DefaultLines"].as_i64()).unwrap_or(1).max(1);
    let mut units: Vec<f64> = r["BettingUnits"].as_array().map(|a| a.iter().filter_map(Value::as_f64).collect()).unwrap_or_default();
    units.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    units.dedup();
    if units.is_empty() {
        return Err(SlotError::Provider("Spinomenal sent no bet sizes".into()));
    }
    s.units = units;
    let total = |u: f64| (u * s.lines as f64 * 100.0).round() as i64;
    s.info.bet_levels = s.units.iter().map(|u| BetLevel { raw: u.to_string(), minor: total(*u) }).collect();
    s.info.default_bet = r["DefaultBetUnit"].as_f64().map(total);
    s.info.balance = r["Coins"].as_f64().map(|c| (c * 100.0).round() as i64);
    s.info.currency = r["CurrencyCode"].as_str().filter(|c| !c.is_empty()).map(str::to_uppercase).unwrap_or_else(|| "FUN".into());
    common["RtpVariantId"] = r["RtpVariantId"].clone();
    common["classId"] = r["ClassId"].clone();
    common["CurrencyCode"] = json!(r["CurrencyCode"].as_str().unwrap_or(""));
    common["EnvironmentRuntime"] = json!(30);
    common["fastPlay"] = json!(false);
    common["autoPlay"] = json!(false);
    common["forceTicket"] = json!(0);
    s.common = common;

    let gi = &init["Result"]["GameInfo"];
    let rtp = r["RTP"].as_f64().map(|x| x / 100.0);
    if let Some(list) = gi["BuyFeatures"].as_array().filter(|a| !a.is_empty()) {
        for (i, b) in list.iter().enumerate() {
            let cost = b["BuyFeatureCost"].as_f64().unwrap_or(0.0);
            let spins = b["FreeSpinAmount"].as_str().map(|n| format!("{n} free spins")).unwrap_or_else(|| "free spins".into());
            s.buys.push(Buy::FreeSpins(json!({ "freeSpinType": b["FreeSpinType"], "freeSpinAmount": b["FreeSpinAmount"], "buyFeatureType": b["BuyFeatureType"].as_i64().unwrap_or(i as i64) })));
            s.info.features.push(Feature { id: format!("buy{i}"), name: format!("Buy {spins} (×{})", fmt_mult(cost)), cost_mult: cost, rtp });
        }
    } else if let Some(list) = r["BuyBonusOptions"].as_array().or(gi["BuyBonusOptions"].as_array()).filter(|a| !a.is_empty()) {
        for (i, b) in list.iter().enumerate() {
            let cost = b["BuyFeatureCost"].as_f64().unwrap_or(0.0);
            let kind = b["BonusType"].as_i64().unwrap_or(i as i64 + 1);
            s.buys.push(Buy::Bonus(kind));
            s.info.features.push(Feature { id: format!("buy{i}"), name: format!("Buy bonus {kind} (×{})", fmt_mult(cost)), cost_mult: cost, rtp });
        }
    }
    Ok(s)
}

impl SpinomenalSession {
    fn url(&self, name: &str) -> Result<String, SlotError> {
        self.urls[name].as_str().map(str::to_string).ok_or_else(|| SlotError::Provider(format!("Spinomenal sent no {name}")))
    }

    async fn post(&self, url: &str, body: &Value) -> Result<Value, SlotError> {
        let origin = self.page.split('/').take(3).collect::<Vec<_>>().join("/");
        let res = self.client.post(url).header("Origin", origin).header("Referer", &self.page).json(body).send().await.map_err(|e| SlotError::Bridge(e.to_string()))?;
        let status = res.status().as_u16();
        let text = res.text().await.map_err(|e| SlotError::Bridge(e.to_string()))?;
        if status == 429 || status == 403 {
            return Err(SlotError::RateLimited);
        }
        let v: Value = serde_json::from_str(&text).map_err(|_| SlotError::Provider(format!("HTTP {status}: {}", text.chars().take(120).collect::<String>())))?;
        let code = v["ResultInfo"]["Code"].as_i64().unwrap_or(0);
        if code != 0 {
            let msg = v["ResultInfo"]["DisplayMessage"].as_str().or(v["ResultInfo"]["Message"].as_str()).unwrap_or("").to_string();
            let lower = msg.to_ascii_lowercase();
            if lower.contains("balance") || lower.contains("funds") || lower.contains("credit") {
                return Err(SlotError::Balance);
            }
            if lower.contains("session") || lower.contains("token") || lower.contains("expired") {
                return Err(SlotError::SessionExpired);
            }
            return Err(SlotError::Provider(format!("Spinomenal error {code}: {msg}")));
        }
        Ok(v)
    }

    fn process_body(&self, unit: f64) -> Value {
        let mut b = self.common.clone();
        b["bet"] = json!(unit);
        b["lines"] = json!(self.lines);
        b["isRetroBetOn"] = json!(false);
        b["isToggleFeatureOn"] = json!(false);
        b["turboMode"] = json!("multi_off");
        b["timestamp"] = json!(timestamp());
        b
    }
}

#[async_trait]
impl SlotSession for SpinomenalSession {
    fn info(&self) -> &SessionInfo {
        &self.info
    }

    async fn spin(&self, bet: i64, feature: Option<&str>) -> Result<SpinResult, SlotError> {
        let t0 = std::time::Instant::now();
        let level = self.info.bet_levels.iter().position(|b| b.minor == bet).ok_or_else(|| SlotError::Provider("That bet size isn't offered by this game".into()))?;
        let unit = self.units[level];
        let buy = match feature.and_then(|f| f.strip_prefix("buy")).and_then(|i| i.parse::<usize>().ok()) {
            Some(i) => Some(self.buys.get(i).cloned().ok_or_else(|| SlotError::Provider("No such buy".into()))?),
            None => None,
        };
        let process = self.url("ProcessUrl")?;
        let (mut res, cost, start) = match &buy {
            Some(Buy::FreeSpins(fields)) => {
                let mut b = self.process_body(unit);
                for (k, v) in fields.as_object().into_iter().flatten() {
                    b[k] = v.clone();
                }
                let res = self.post(&self.url("BuyFeatureUrl")?, &b).await?;
                let price = res["Result"]["FreeTickets"]["PurchasePrice"].as_f64().or(res["Result"]["PurchasePrice"].as_f64()).unwrap_or(0.0);
                let start = res["Result"]["CoinOut"].as_f64();
                (res, price, start)
            }
            Some(Buy::Bonus(kind)) => {
                let mut b = self.process_body(unit);
                b["BuyFeatureBonusType"] = json!(kind);
                let res = self.post(&process, &b).await?;
                let r = &res["Result"];
                (res.clone(), r["TotalBetPlaced"].as_f64().unwrap_or(0.0), r["CoinIn"].as_f64())
            }
            None => {
                let res = self.post(&process, &self.process_body(unit)).await?;
                let r = &res["Result"];
                (res.clone(), r["TotalBetPlaced"].as_f64().unwrap_or(0.0), r["CoinIn"].as_f64())
            }
        };
        let mut cost = cost;
        let round_id = res["Result"]["UniqueIdentifier"].as_i64().filter(|n| *n != 0).map(|n| n.to_string()).unwrap_or_default();
        let mut end = res["Result"]["CoinOut"].as_f64().filter(|c| *c > 0.0);
        let mut steps = 1usize;
        let mut free_spins = matches!(buy, Some(Buy::FreeSpins(_)));
        loop {
            let r = &res["Result"];
            let ft = &r["FreeTickets"];
            if ft["PendingAction"].as_bool().unwrap_or(false) {
                free_spins = true;
                let mut b = self.common.clone();
                b["ticketId"] = ft["Id"].clone();
                // the pick reply is a free-spins ticket, not a spin: the round goes on
                self.post(&self.url("PickFreeSpinsUrl")?, &b).await?;
                res = self.post(&process, &self.process_body(unit)).await?;
            } else if self.tower && r["IsRoundFinished"].as_bool() == Some(false) {
                // bank what the tower holds instead of paying for another climb
                res = self.post(&self.url("TowerCollectUrl")?, &self.common).await?;
                if let Some(c) = res["Result"]["CoinOut"].as_f64().filter(|c| *c > 0.0) {
                    end = Some(c);
                }
                steps += 1;
                break;
            } else if r["IsRoundFinished"].as_bool() == Some(false) {
                free_spins |= ft["To"].as_i64().unwrap_or(0) > 0 || !r["BonusSpin"].is_null();
                res = self.post(&process, &self.process_body(unit)).await?;
            } else {
                break;
            }
            if let Some(c) = res["Result"]["CoinOut"].as_f64().filter(|c| *c > 0.0) {
                end = Some(c);
            }
            // anything a continuation charged (free spins charge nothing)
            cost += res["Result"]["TotalBetPlaced"].as_f64().unwrap_or(0.0);
            steps += 1;
            if steps > 600 {
                return Err(SlotError::Provider("The round didn't end".into()));
            }
        }
        let cents = |x: f64| (x * 100.0).round() as i64;
        let cost = cents(cost);
        let win = match (start, end) {
            (Some(s), Some(e)) => cents(e - s).max(0),
            _ => cents(res["Result"]["Win"].as_f64().unwrap_or(0.0)),
        };
        let ms = t0.elapsed().as_secs_f64() * 1000.0;
        Ok(SpinResult {
            round_id,
            cost,
            win,
            multiplier: if cost > 0 { win as f64 / cost as f64 } else { 0.0 },
            balance: end.map(cents),
            pending: false,
            free_spins,
            events: steps,
            ms,
        })
    }

    fn close(&self) {}
}

/// ISO-8601 UTC with milliseconds, like the client's `new Date().toISOString()`.
fn timestamp() -> String {
    let d = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
    let (secs, ms) = (d.as_secs() as i64, d.subsec_millis());
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    // civil-from-days (Howard Hinnant)
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + if month <= 2 { 1 } else { 0 };
    format!("{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{ms:03}Z", rem / 3600, rem % 3600 / 60, rem % 60)
}

fn fmt_mult(x: f64) -> String {
    if (x - x.round()).abs() < 1e-6 {
        format!("{}", x.round() as i64)
    } else {
        format!("{x:.2}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids() {
        let t = timestamp();
        assert_eq!(t.len(), 24);
        assert!(t.ends_with('Z') && t.as_bytes()[10] == b'T');
    }
}
