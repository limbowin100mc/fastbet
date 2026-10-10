//! Diagnostic: does a second socket authenticating the same guest token break
//! the first socket's login? Run with `cargo run --example probe`.

use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio_tungstenite::tungstenite::Message;

type Ws = tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

async fn call(ws: &mut Ws, id: u64, channel: &str, action: &str, args: Vec<Value>) -> Result<Value, String> {
    let frame = json!([channel, id, action, args]).to_string();
    ws.send(Message::Text(frame.into())).await.map_err(|e| e.to_string())?;
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(15);
    loop {
        let msg = tokio::time::timeout_at(deadline, ws.next()).await.map_err(|_| "timeout".to_string())?;
        let Some(Ok(Message::Text(t))) = msg else { continue };
        let Ok(Value::Array(msgs)) = serde_json::from_str::<Value>(&t) else { continue };
        for m in msgs {
            if m.get(1).and_then(|v| v.as_u64()) == Some(id) {
                return match m.get(3) {
                    Some(err) if !err.is_null() => Err(err.get(0).map(|e| e.to_string()).unwrap_or(err.to_string())),
                    _ => Ok(m.get(2).cloned().unwrap_or(Value::Null)),
                };
            }
        }
    }
}

#[tokio::main]
async fn main() {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let url = "wss://api.chips.gg/prod/socket";

    let (mut a, _) = tokio_tungstenite::connect_async(url).await.expect("connect A");
    let token = call(&mut a, 1, "auth", "token", vec![]).await.expect("auth.token");
    let token = token.as_str().unwrap_or_default().to_string();
    println!("guest token: {}…", &token[..12.min(token.len())]);
    let uid = call(&mut a, 2, "auth", "authenticate", vec![json!(token)]).await;
    println!("A authenticate -> {uid:?}");

    let base = |captcha: Value| vec![json!({"email": "nobody@example.com", "password": "wrong-password", "code": "", "captcha": captcha})];
    let pause = || tokio::time::sleep(std::time::Duration::from_secs(6));
    println!("A login (captcha object)  -> {:?}", call(&mut a, 3, "auth", "login", base(json!({"token": "bogus", "ekey": "x"}))).await);
    pause().await;
    println!("A login (captcha null)    -> {:?}", call(&mut a, 4, "auth", "login", base(Value::Null)).await);
    pause().await;
    println!("A login (captcha string)  -> {:?}", call(&mut a, 7, "auth", "login", base(json!("bogus"))).await);
    pause().await;
    let login_args = || vec![json!({"email": "nobody@example.com", "password": "wrong-password", "code": "", "captcha": {"token": "bogus", "ekey": "x"}})];

    let (mut b, _) = tokio_tungstenite::connect_async(url).await.expect("connect B");
    println!("B authenticate same token -> {:?}", call(&mut b, 1, "auth", "authenticate", vec![json!(token)]).await);
    println!("A login (after B)         -> {:?}", call(&mut a, 5, "auth", "login", login_args()).await);
    println!("A private.me (after B)    -> {:?}", call(&mut a, 6, "private", "me", vec![]).await.map(|v| v.is_object()));
}
