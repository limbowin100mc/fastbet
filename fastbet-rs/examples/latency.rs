//! Diagnostic: round-trip time to the chips.gg API, sequential and pipelined.
//! `cargo run --example latency`

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use futures_util::{SinkExt, StreamExt};
use parking_lot::Mutex;
use serde_json::{json, Value};
use tokio::sync::{mpsc, oneshot};
use tokio_tungstenite::tungstenite::Message;

#[tokio::main]
async fn main() {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let url = "wss://api.chips.gg/prod/socket";

    let t0 = Instant::now();
    let (ws, _) = tokio_tungstenite::connect_async(url).await.expect("connect");
    println!("connect + TLS handshake: {} ms", t0.elapsed().as_millis());
    let (mut sink, mut stream) = ws.split();
    let pending: Arc<Mutex<HashMap<u64, oneshot::Sender<Value>>>> = Arc::new(Mutex::new(HashMap::new()));
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
                            let _ = s.send(m.get(2).cloned().unwrap_or(Value::Null));
                        }
                    }
                }
            }
        }
    });
    let seq = Arc::new(std::sync::atomic::AtomicU64::new(1));
    let call = |action: &'static str, args: Value| {
        let id = seq.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let (s, r) = oneshot::channel();
        pending.lock().insert(id, s);
        let _ = tx.send(json!(["public", id, action, [args]]).to_string());
        async move { r.await.ok() }
    };

    // sequential
    let mut times = Vec::new();
    for i in 0..20 {
        let t = Instant::now();
        call("echo", json!({"message": i})).await;
        times.push(t.elapsed().as_secs_f64() * 1000.0);
    }
    times.sort_by(|a, b| a.partial_cmp(b).unwrap());
    println!("sequential echo x20: min {:.0} ms, median {:.0} ms, max {:.0} ms", times[0], times[10], times[19]);

    // pipelined: 10 calls at once
    let t = Instant::now();
    let futs: Vec<_> = (0..10).map(|i| call("echo", json!({"message": i}))).collect();
    futures_util::future::join_all(futs).await;
    let total = t.elapsed().as_secs_f64() * 1000.0;
    println!("pipelined echo x10: {:.0} ms total -> {:.1} calls/s if bets pipeline the same way", total, 10.0 / (total / 1000.0));
    tokio::time::sleep(Duration::from_millis(100)).await;
}
