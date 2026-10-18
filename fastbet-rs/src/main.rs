//! FastBet on a PC: everything lives in the library (src/lib.rs).
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() -> eframe::Result<()> {
    fastbet::desktop_main()
}
