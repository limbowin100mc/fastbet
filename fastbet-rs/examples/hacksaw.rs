//! Diagnostic: drive a Hacksaw Gaming demo session straight over HTTP.
//! `cargo run --example hacksaw -- [gameId] [spins] [bonusGameId]`

use serde_json::{json, Value};
use std::time::Instant;

#[tokio::main]
async fn main() {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let args: Vec<String> = std::env::args().collect();
    let game_id: u64 = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(2564);
    let spins: usize = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(5);
    let bonus = args.get(3).cloned();
    let env = "https://rgs-demo.hacksawgaming.com/api";
    let ua = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/154.0.0.0 Safari/537.36 Edg/154.0.0.0";
    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert("Origin", "https://static-live.hacksawgaming.com".parse().unwrap());
    headers.insert("Referer", format!("https://static-live.hacksawgaming.com/{game_id}/").parse().unwrap());
    headers.insert("Accept", "application/json, text/plain, */*".parse().unwrap());
    headers.insert("Access-Control-Allow-Origin", "*".parse().unwrap());
    let client = reqwest::Client::builder().user_agent(ua).default_headers(headers).build().unwrap();

    let version: Value = client.get(format!("https://static-live.hacksawgaming.com/{game_id}/version.json")).send().await.unwrap().json().await.unwrap();
    let ver = version["version"].as_str().unwrap_or("1.0.0").to_string();
    println!("game version {ver}");

    let mut seq = 1u64;
    let resp = client
        .post(format!("{env}/play/authenticate"))
        .json(&json!({"seq": seq, "partner": "demo", "gameId": game_id, "gameVersion": ver, "currency": "EUR", "languageCode": "en-us", "mode": 2, "branding": "default", "channel": 1, "userAgent": ua, "token": "demo", "currencyMultiplier": 1}))
        .send().await.unwrap();
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    let auth: Value = match serde_json::from_str(&text) {
        Ok(v) => v,
        Err(_) => {
            println!("authenticate HTTP {status}: {}", text.chars().take(600).collect::<String>());
            return;
        }
    };
    let session = auth["sessionUuid"].as_str().unwrap_or_default().to_string();
    println!("session {session}  statusCode {:?}", auth.get("statusCode"));
    println!("betLevels {:?}", auth["betLevels"]);
    println!("default {:?}  bonusGames {}", auth["defaultBetLevel"], auth["bonusGames"]);
    println!("balance {:?}", auth["accountBalance"]);
    let bet = auth["defaultBetLevel"].as_str().unwrap_or("200").to_string();

    for i in 0..spins {
        seq += 1;
        let mut b = json!({"betAmount": bet});
        if let Some(id) = &bonus {
            b["bonusGameId"] = json!(id);
        }
        let t0 = Instant::now();
        let res: Value = client
            .post(format!("{env}/play/bet"))
            .json(&json!({"seq": seq, "sessionUuid": session, "bets": [b], "offerId": null, "promotionId": null, "autoplay": false}))
            .send().await.unwrap().json().await.unwrap();
        let ms = t0.elapsed().as_millis();
        let round = &res["round"];
        let events = round["events"].as_array().cloned().unwrap_or_default();
        let awa = events.last().and_then(|e| e["awa"].as_str()).unwrap_or("0");
        let cost: f64 = round["events"].as_array().and_then(|e| e.first()).and_then(|e| e["bc"].as_str()).and_then(|s| s.parse().ok()).unwrap_or(0.0);
        println!(
            "spin {} | status {:?} | round {:?} | events {} ({}) | win {} | cost {} | balance {:?} | statusCode {:?} {:?} | {} ms",
            i + 1,
            round["status"],
            round["roundId"],
            events.len(),
            events.iter().map(|e| e["etn"].as_str().unwrap_or("?")).collect::<Vec<_>>().join(","),
            awa,
            cost,
            res["accountBalance"]["balance"],
            res["statusCode"],
            res["statusMessage"],
            ms
        );
        if i == 0 && bonus.is_some() {
            println!("first reply: {}", serde_json::to_string(&res).unwrap_or_default().chars().take(1500).collect::<String>());
        }
    }
}
