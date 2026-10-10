//! Diagnostic: call any public chips.gg action.
//! `cargo run --example games -- <action> '<json params>' [max chars]`

use std::collections::HashMap;
use std::sync::Arc;

use futures_util::{SinkExt, StreamExt};
use parking_lot::Mutex;
use serde_json::{json, Value};
use tokio::sync::{mpsc, oneshot};
use tokio_tungstenite::tungstenite::Message;

#[tokio::main]
async fn main() {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let args: Vec<String> = std::env::args().collect();
    let action = args.get(1).cloned().unwrap_or_else(|| "listGameProducers".into());
    let params: Value = args.get(2).map(|s| serde_json::from_str(s).expect("params must be JSON")).unwrap_or(json!({}));
    let max: usize = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(4000);

    let (ws, _) = tokio_tungstenite::connect_async("wss://api.chips.gg/prod/socket").await.expect("connect");
    let (mut sink, mut stream) = ws.split();
    let pending: Arc<Mutex<HashMap<u64, oneshot::Sender<Result<Value, String>>>>> = Arc::new(Mutex::new(HashMap::new()));
    let (tx, mut rx) = mpsc::unbounded_channel::<String>();
    tokio::spawn(async move {
        while let Some(s) = rx.recv().await {
            if sink.send(Message::Text(s.into())).await.is_err() {
                break;
            }
        }
    });
    let p2 = pending.clone();
    tokio::spawn(async move {
        while let Some(Ok(Message::Text(t))) = stream.next().await {
            if let Ok(Value::Array(msgs)) = serde_json::from_str::<Value>(&t) {
                for m in msgs {
                    if let Some(id) = m.get(1).and_then(|v| v.as_u64()) {
                        if let Some(s) = p2.lock().remove(&id) {
                            let r = match m.get(3) {
                                Some(e) if !e.is_null() => Err(e.to_string()),
                                _ => Ok(m.get(2).cloned().unwrap_or(Value::Null)),
                            };
                            let _ = s.send(r);
                        }
                    }
                }
            }
        }
    });
    // FASTBET_TOKEN=<api key> authenticates and sends the action on the private channel
    let mut channel = "public";
    if let Ok(token) = std::env::var("FASTBET_TOKEN") {
        let (s, r) = oneshot::channel();
        pending.lock().insert(99, s);
        let _ = tx.send(json!(["auth", 99, "authenticate", [token]]).to_string());
        match tokio::time::timeout(std::time::Duration::from_secs(20), r).await {
            Ok(Ok(Ok(v))) => println!("authenticated as {v}"),
            other => {
                println!("auth failed: {other:?}");
                return;
            }
        }
        channel = "private";
    }
    let (s, r) = oneshot::channel();
    pending.lock().insert(1, s);
    let _ = tx.send(json!([channel, 1, action, [params]]).to_string());
    match tokio::time::timeout(std::time::Duration::from_secs(90), r).await {
        Ok(Ok(Ok(v))) => {
            let s = serde_json::to_string_pretty(&v).unwrap_or_default();
            if s.len() > max {
                println!("{}…\n[{} chars total]", &s[..max], s.len());
            } else {
                println!("{s}");
            }
        }
        Ok(Ok(Err(e))) => println!("ERROR {e}"),
        _ => println!("timeout"),
    }
}
