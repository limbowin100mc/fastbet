//! FastBet — instant Limbo bets straight over casino websocket APIs.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod bridge;
mod casino;
mod cloudflare;
mod currency;
mod engine;
mod login;
mod money;
mod originals;
mod settings;
mod slots;
mod storage;
mod ui;
mod update;

use std::sync::Arc;

fn main() -> eframe::Result<()> {
    login::pick_gtk_backend();
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "fastbet=info,warn".into()))
        .with_writer(std::io::stderr)
        .init();

    // `fastbet --login <casino>`: we are the login window child process.
    let args: Vec<String> = std::env::args().collect();
    if args.len() >= 3 && args[1] == "--bridge" {
        if let Err(e) = bridge::run_child(&args[2], args.get(3).map(String::as_str)) {
            eprintln!("bridge failed: {e}");
            std::process::exit(1);
        }
        return Ok(());
    }
    if args.len() >= 3 && args[1] == "--capture" {
        if let Err(e) = login::run_capture(&args[2]) {
            eprintln!("capture failed: {e}");
            std::process::exit(1);
        }
        return Ok(());
    }
    if args.len() >= 3 && args[1] == "--login" {
        let id = casino::CasinoId::from_key(&args[2]).unwrap_or(casino::CasinoId::ChipsGg);
        if let Err(e) = login::run_window(id, args.get(3).map(String::as_str), args.get(4).map(String::as_str)) {
            eprintln!("login window failed: {e}");
            std::process::exit(1);
        }
        return Ok(());
    }

    // Two rustls back-ends end up in the dependency graph; pick one explicitly.
    let _ = rustls::crypto::ring::default_provider().install_default();

    // `fastbet --bridge-test`: spins a Hacksaw demo through the browser bridge and exits.
    if args.len() >= 2 && args[1] == "--bridge-test" {
        let rt = tokio::runtime::Runtime::new().expect("runtime");
        rt.block_on(slots::hacksaw::self_test(
            args.get(2).and_then(|s| s.parse().ok()).unwrap_or(2564),
            args.get(3).and_then(|s| s.parse().ok()).unwrap_or(6),
        ));
        return Ok(());
    }

    // `fastbet --slot-test <producer> <launch-url> [spins] [feature]`: a slot session the way the app opens it.
    if args.len() >= 4 && args[1] == "--slot-test" {
        let rt = tokio::runtime::Runtime::new().expect("runtime");
        rt.block_on(slots::self_test(&args[2], &args[3], args.get(4).and_then(|s| s.parse().ok()).unwrap_or(5), args.get(5).map(String::as_str)));
        return Ok(());
    }

    // `fastbet --gambulls-test [bets]`: connects to Gambulls the way the app does (see `casino::gambulls::self_test`).
    if args.len() >= 2 && args[1] == "--gambulls-test" {
        let rt = tokio::runtime::Runtime::new().expect("runtime");
        rt.block_on(casino::gambulls::self_test(args.get(2).and_then(|s| s.parse().ok()).unwrap_or(5)));
        return Ok(());
    }

    let icon = eframe::icon_data::from_png_bytes(include_bytes!("../assets/icon.png")).ok();
    let mut viewport = egui::ViewportBuilder::default()
        .with_title("FastBet")
        .with_inner_size([1360.0, 860.0])
        .with_min_inner_size([1040.0, 640.0])
        // FastBet draws its own title bar (ui::titlebar)
        .with_decorations(false)
        .with_app_id("gg.fastbet");
    if let Some(icon) = icon {
        viewport = viewport.with_icon(Arc::new(icon));
    }

    let options = eframe::NativeOptions { viewport, ..Default::default() };
    eframe::run_native("FastBet", options, Box::new(|cc| Ok(Box::new(ui::App::new(cc)))))
}
