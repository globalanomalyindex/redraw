#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
//! reDraw: a tiny always-on-top, click-through crosshair.
//!
//! It never reads game data. The only things it looks at are its own config
//! file, its own hotkeys, and whether the right mouse button is down.

mod autocolor;
mod color;
mod config;
mod crosshair;
mod display;
mod dynamics;
mod experimental;
#[cfg(windows)]
mod frame;
#[cfg(windows)]
mod http;
#[cfg(windows)]
mod hud;
#[cfg(windows)]
mod lens;
mod menu;
mod morph;
#[cfg(windows)]
mod monitors;
#[cfg(windows)]
mod overlay;
mod paint;
#[cfg(windows)]
mod sample;
mod share;
mod states;
mod trace;
mod update;

#[cfg(windows)]
fn main() {
    let args: Vec<String> = std::env::args().collect();
    if let Some(i) = args.iter().position(|a| a == "--dump") {
        std::process::exit(overlay::dump(args.get(i + 1).map(String::as_str).unwrap_or("redraw-menu")));
    }
    // "Restart now" starts the new copy with the old one's id: wait for that one to be gone first.
    if let Some(i) = args.iter().position(|a| a == "--restart-from") {
        if let Some(pid) = args.get(i + 1).and_then(|p| p.parse::<u32>().ok()) {
            overlay::wait_for_exit(pid);
        }
    }
    let smoke = args.iter().any(|a| a == "--smoke");
    let path = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.join("redraw.cfg")))
        .unwrap_or_else(|| "redraw.cfg".into());
    std::process::exit(overlay::run(path, smoke));
}

#[cfg(not(windows))]
fn main() {
    eprintln!("reDraw is Windows-only.");
    std::process::exit(1);
}
