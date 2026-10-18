//! Follows the desktop's dark/light preference.
//!
//! Change notifications come from the `dark-light` crate (xdg-desktop-portal on Linux,
//! `NSDistributedNotificationCenter` on macOS). Because notifications are not available
//! on every desktop, a slow polling thread acts as a backstop.

use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};
use std::thread;
use std::time::Duration;

use dark_light::Mode;
use tracing::{debug, info};

use crate::theme::Palette;

const UNKNOWN: u8 = 0;
const DARK: u8 = 1;
const LIGHT: u8 = 2;

const POLL_EVERY: Duration = Duration::from_secs(3);

pub struct SystemTheme {
    state: Arc<AtomicU8>,
}

impl SystemTheme {
    /// Detects the current preference and starts watching for changes. `ctx` is asked to
    /// repaint whenever the preference changes.
    pub fn start(ctx: egui::Context) -> Self {
        let state = Arc::new(AtomicU8::new(encode(detect())));
        spawn("kwinfo-theme-watch", {
            let state = Arc::clone(&state);
            let ctx = ctx.clone();
            move || watch(&state, &ctx)
        });
        spawn("kwinfo-theme-poll", {
            let state = Arc::clone(&state);
            move || poll(&state, &ctx)
        });
        Self { state }
    }

    /// `None` when the desktop expressed no preference or detection failed.
    pub fn current(&self) -> Option<Palette> {
        decode(self.state.load(Ordering::Relaxed))
    }
}

fn spawn(name: &str, f: impl FnOnce() + Send + 'static) {
    if let Err(err) = thread::Builder::new().name(name.to_string()).spawn(f) {
        debug!("could not start {name}: {err}");
    }
}

fn detect() -> Option<Palette> {
    match dark_light::detect() {
        Ok(mode) => from_mode(mode),
        Err(err) => {
            debug!("system theme detection failed: {err}");
            None
        }
    }
}

fn from_mode(mode: Mode) -> Option<Palette> {
    match mode {
        Mode::Dark => Some(Palette::Dark),
        Mode::Light => Some(Palette::Light),
        Mode::Unspecified => None,
    }
}

fn encode(palette: Option<Palette>) -> u8 {
    match palette {
        None => UNKNOWN,
        Some(Palette::Dark) => DARK,
        Some(Palette::Light) => LIGHT,
    }
}

fn decode(value: u8) -> Option<Palette> {
    match value {
        DARK => Some(Palette::Dark),
        LIGHT => Some(Palette::Light),
        _ => None,
    }
}

fn store(state: &AtomicU8, palette: Option<Palette>, ctx: &egui::Context) {
    let encoded = encode(palette);
    if state.swap(encoded, Ordering::Relaxed) != encoded {
        info!("system theme changed to {palette:?}");
        ctx.request_repaint();
    }
}

fn watch(state: &AtomicU8, ctx: &egui::Context) {
    match dark_light::subscribe() {
        Ok(watcher) => {
            for mode in watcher.iter() {
                store(state, from_mode(mode), ctx);
            }
            debug!("system theme watcher ended; polling continues");
        }
        Err(err) => debug!("system theme notifications unavailable ({err}); polling instead"),
    }
}

fn poll(state: &AtomicU8, ctx: &egui::Context) {
    loop {
        thread::sleep(POLL_EVERY);
        store(state, detect(), ctx);
    }
}
