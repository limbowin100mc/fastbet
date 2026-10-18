//! Alerts on the phone: a Telegram bot and / or a Discord webhook get a short message when a bet
//! hits, a run ends by itself, or auto vault puts profit away. Messages go out on their own
//! thread, so betting never waits for them.

use std::collections::HashSet;
use std::sync::{mpsc, Mutex, OnceLock};
use std::time::Duration;

use base64::Engine as _;
use reqwest::blocking::multipart;
use serde_json::{json, Value};

/// The app's icon: the Discord webhook's picture.
const ICON: &[u8] = include_bytes!("../assets/icon.png");

/// Where alerts go.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Targets {
    /// Bot token and chat id.
    pub telegram: Option<(String, String)>,
    /// Webhook URL.
    pub discord: Option<String>,
}

impl Targets {
    pub fn any(&self) -> bool {
        self.telegram.is_some() || self.discord.is_some()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Hit,
    RunEnded,
    Vault,
    Test,
}

impl Kind {
    /// The Discord embed's edge.
    fn color(self) -> u32 {
        match self {
            Kind::Hit => 0xffd14a,
            Kind::RunEnded => 0x5b8def,
            Kind::Vault | Kind::Test => 0x1ed380,
        }
    }

    fn mark(self) -> &'static str {
        match self {
            Kind::Hit => "🎯",
            Kind::RunEnded => "⏹",
            Kind::Vault => "🔒",
            Kind::Test => "✅",
        }
    }
}

/// What an alert says: a title and a few lines, and for a hit its card.
#[derive(Clone, Debug)]
pub struct Alert {
    pub kind: Kind,
    pub title: String,
    pub lines: Vec<String>,
    pub picture: Option<Picture>,
}

/// A hit card to send along, drawn on the alert's thread (it takes a moment).
#[derive(Clone, Debug)]
pub struct Picture {
    pub card: crate::ui::hitcard::Card,
    /// The game's artwork URL (empty for none).
    pub art: String,
}

/// A bot token as @BotFather gives it: digits, a colon, then the secret.
pub fn telegram_token_ok(token: &str) -> bool {
    let t = token.trim();
    match t.split_once(':') {
        Some((id, secret)) => !id.is_empty() && id.chars().all(|c| c.is_ascii_digit()) && secret.len() >= 20 && !secret.contains(char::is_whitespace),
        None => false,
    }
}

/// A Discord webhook link (discord.com or discordapp.com, under /api/webhooks/).
pub fn discord_url_ok(url: &str) -> bool {
    let u = url.trim();
    let Some(rest) = u.strip_prefix("https://") else { return false };
    let (host, path) = rest.split_once('/').unwrap_or((rest, ""));
    let host_ok = ["discord.com", "discordapp.com"].iter().any(|h| host == *h || host.ends_with(&format!(".{h}")));
    host_ok && path.starts_with("api/webhooks/")
}

fn client() -> Result<reqwest::blocking::Client, String> {
    reqwest::blocking::Client::builder().timeout(Duration::from_secs(12)).user_agent("FastBet").build().map_err(|e| e.to_string())
}

/// Sends `alert` to every target on its own thread. `done` hears how each one went
/// ("Telegram: sent" / "Discord: …error").
pub fn send(targets: Targets, alert: Alert, done: Option<mpsc::Sender<Result<String, String>>>) {
    if !targets.any() {
        return;
    }
    let _ = std::thread::Builder::new().name("fastbet-alert".into()).spawn(move || {
        let client = match client() {
            Ok(c) => c,
            Err(e) => {
                if let Some(d) = &done {
                    let _ = d.send(Err(e));
                }
                return;
            }
        };
        let png = alert.picture.as_ref().and_then(|p| match crate::ui::hits::card_png(&p.card, &p.art) {
            Ok(png) => Some(png),
            Err(e) => {
                tracing::warn!("alert: the hit card didn't draw: {e}");
                None
            }
        });
        if let Some((token, chat)) = &targets.telegram {
            let r = telegram(&client, token, chat, &alert, png.as_deref()).map(|_| "Telegram: sent".to_string()).map_err(|e| format!("Telegram: {e}"));
            if let Err(e) = &r {
                tracing::warn!("alert: {e}");
            }
            if let Some(d) = &done {
                let _ = d.send(r);
            }
        }
        if let Some(url) = &targets.discord {
            discord_avatar(&client, url);
            let r = discord(&client, url, &alert, png.as_deref()).map(|_| "Discord: sent".to_string()).map_err(|e| format!("Discord: {e}"));
            if let Err(e) = &r {
                tracing::warn!("alert: {e}");
            }
            if let Some(d) = &done {
                let _ = d.send(r);
            }
        }
    });
}

fn escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

/// A message, or with a hit card a photo with the message as its caption.
fn telegram(client: &reqwest::blocking::Client, token: &str, chat: &str, a: &Alert, png: Option<&[u8]>) -> Result<(), String> {
    let mut text = format!("{} <b>{}</b>", a.kind.mark(), escape(&a.title));
    for l in &a.lines {
        text.push('\n');
        text.push_str(&escape(l));
    }
    let base = format!("https://api.telegram.org/bot{}", token.trim());
    let res = match png {
        Some(png) => {
            let photo = multipart::Part::bytes(png.to_vec()).file_name("hit.png").mime_str("image/png").map_err(|e| e.to_string())?;
            let form = multipart::Form::new().text("chat_id", chat.trim().to_string()).text("caption", text).text("parse_mode", "HTML").part("photo", photo);
            client.post(format!("{base}/sendPhoto")).multipart(form).send()
        }
        None => {
            let body = json!({ "chat_id": chat.trim(), "text": text, "parse_mode": "HTML", "disable_web_page_preview": true });
            client.post(format!("{base}/sendMessage")).json(&body).send()
        }
    };
    let reply: Value = res.map_err(|e| net_error(&e))?.json().map_err(|e| e.to_string())?;
    if reply["ok"].as_bool() == Some(true) {
        Ok(())
    } else {
        Err(reply["description"].as_str().unwrap_or("not sent").to_string())
    }
}

/// An embed; with a hit card, the card as an attached file shown inside it.
fn discord(client: &reqwest::blocking::Client, url: &str, a: &Alert, png: Option<&[u8]>) -> Result<(), String> {
    let mut embed = json!({ "title": format!("{} {}", a.kind.mark(), a.title), "description": a.lines.join("\n"), "color": a.kind.color() });
    let res = match png {
        Some(png) => {
            embed["image"] = json!({ "url": "attachment://hit.png" });
            let payload = json!({ "username": "FastBet", "embeds": [embed], "attachments": [{ "id": 0, "filename": "hit.png" }] });
            let file = multipart::Part::bytes(png.to_vec()).file_name("hit.png").mime_str("image/png").map_err(|e| e.to_string())?;
            let form = multipart::Form::new().text("payload_json", payload.to_string()).part("files[0]", file);
            client.post(url.trim()).multipart(form).send()
        }
        None => client.post(url.trim()).json(&json!({ "username": "FastBet", "embeds": [embed] })).send(),
    };
    let res = res.map_err(|e| net_error(&e))?;
    if res.status().is_success() {
        Ok(())
    } else {
        let status = res.status();
        let msg = res.json::<Value>().ok().and_then(|v| v["message"].as_str().map(str::to_string)).unwrap_or_else(|| status.to_string());
        Err(msg)
    }
}

/// Gives the webhook FastBet's name and icon, once per webhook while the app runs (a webhook's
/// picture can't come with each message without a public link to it).
fn discord_avatar(client: &reqwest::blocking::Client, url: &str) {
    static DONE: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    let done = DONE.get_or_init(Default::default);
    let url = url.trim().to_string();
    if done.lock().map(|d| d.contains(&url)).unwrap_or(true) {
        return;
    }
    let avatar = format!("data:image/png;base64,{}", base64::engine::general_purpose::STANDARD.encode(ICON));
    match client.patch(&url).json(&json!({ "name": "FastBet", "avatar": avatar })).send() {
        Ok(r) if r.status().is_success() => {
            if let Ok(mut d) = done.lock() {
                d.insert(url);
            }
        }
        Ok(r) => tracing::warn!("alert: Discord didn't take the webhook picture: {}", r.status()),
        Err(e) => tracing::warn!("alert: Discord didn't take the webhook picture: {}", net_error(&e)),
    }
}

fn net_error(e: &reqwest::Error) -> String {
    if e.is_timeout() {
        "no answer (timed out)".into()
    } else if e.is_connect() {
        "couldn't connect".into()
    } else {
        e.to_string()
    }
}

/// The chat that last wrote to the bot, as (id, name): what "Find my chat" fills in.
pub fn find_chat(token: &str) -> Result<(String, String), String> {
    let url = format!("https://api.telegram.org/bot{}/getUpdates", token.trim());
    let reply: Value = client()?.get(url).send().map_err(|e| net_error(&e))?.json().map_err(|e| e.to_string())?;
    if reply["ok"].as_bool() != Some(true) {
        return Err(reply["description"].as_str().unwrap_or("Telegram refused the token").to_string());
    }
    let chat = reply["result"]
        .as_array()
        .and_then(|a| a.iter().rev().find_map(|u| u.get("message").or(u.get("channel_post")).or(u.get("my_chat_member")).and_then(|m| m.get("chat")).cloned()))
        .ok_or("No message yet: send your bot any message in Telegram, then try again")?;
    let id = match &chat["id"] {
        Value::Number(n) => n.to_string(),
        Value::String(s) => s.clone(),
        _ => return Err("Telegram sent a chat without an id".into()),
    };
    let name = ["first_name", "title", "username"].iter().find_map(|k| chat[*k].as_str()).unwrap_or("your chat").to_string();
    Ok((id, name))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_and_links() {
        assert!(telegram_token_ok("123456789:AAHdqTcvCH1vGWJxfSeofSAs0K5PALDsaw"));
        assert!(!telegram_token_ok("AAHdqTcvCH1vGWJxfSeofSAs0K5PALDsaw"));
        assert!(!telegram_token_ok("12x:AAHdqTcvCH1vGWJxfSeofSAs0K5PALDsaw"));
        assert!(discord_url_ok("https://discord.com/api/webhooks/123/abc"));
        assert!(discord_url_ok("https://ptb.discord.com/api/webhooks/123/abc"));
        assert!(discord_url_ok("https://discordapp.com/api/webhooks/1/x"));
        assert!(!discord_url_ok("http://discord.com/api/webhooks/1/x"));
        assert!(!discord_url_ok("https://evil.com/api/webhooks/1/x"));
        assert!(!discord_url_ok("https://discord.com.evil.com/api/webhooks/1/x"));
        assert_eq!(escape("a<b>&"), "a&lt;b&gt;&amp;");
    }

    const PNG_MAGIC: &[u8] = &[0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a];

    #[test]
    fn hits_send_their_card_and_the_webhook_gets_the_icon() {
        let card = crate::ui::hitcard::Card {
            game: "Limbo".into(),
            byline: "Originals · Stake.com".into(),
            multiplier: 1000.0,
            win: "$10.00".into(),
            bet: "$0.01".into(),
            when: "18 Oct 2026 · 19:39".into(),
            demo: false,
            hit: true,
        };
        let png = crate::ui::hits::card_png(&card, "").unwrap();
        assert_eq!(&png[..8], PNG_MAGIC);
        if let Ok(out) = std::env::var("FASTBET_CARD_OUT") {
            std::fs::write(out, &png).unwrap();
        }
        assert_eq!(&ICON[..8], PNG_MAGIC);
    }
}
