//! Sensor values in the system tray (PLAN.md 4.3): one StatusNotifierItem per sensor the
//! user adds with "Add to tray", whose icon is the current value drawn as text, with the
//! sensor's name in the tooltip and a menu to show or hide KWinfo, remove the item, or
//! quit. While any item is in the tray, closing the main window hides it instead.
//!
//! The icons are rendered here, on any OS (and unit-tested); the items themselves need a
//! Linux desktop with a StatusNotifierItem host (KDE, or GNOME with the AppIndicator
//! extension), through `ksni`.

use ab_glyph::{Font as _, FontRef, Glyph, PxScale, ScaleFont as _, point};
use egui::Color32;
use kwinfo_core::{ChannelId, TemperatureUnit, Unit};

use crate::theme::{DARK, TEXT_CONTRAST};

/// Icon sizes handed to the tray host, which picks the closest.
pub const ICON_SIZES: [u32; 2] = [24, 48];
/// The icons' backdrop: dark, so the value reads on light and dark panels alike.
const BACKDROP: [u8; 4] = [24, 24, 24, 235];

/// What a tray item shows for one sensor.
#[derive(Clone, Debug, PartialEq)]
pub struct TrayView {
    pub channel: ChannelId,
    /// `CPU Package`.
    pub title: String,
    /// `48.3 °C · CPU [#0]: Intel Core i7-8700K`.
    pub tooltip: String,
    /// `48`.
    pub text: String,
    /// The sensor's colour before adapting it to the backdrop.
    pub color: Color32,
}

/// A value in at most about four characters, for an icon: `48`, `4.3` (GHz), `1.20`.
pub fn tray_text(value: Option<f64>, unit: Unit, temperature: TemperatureUnit) -> String {
    let Some(v) = value.filter(|v| v.is_finite()) else {
        return "-".to_string();
    };
    let short = |v: f64| {
        if v.abs() >= 100.0 {
            format!("{v:.0}")
        } else {
            format!("{v:.1}")
        }
    };
    match unit {
        Unit::Celsius => format!("{:.0}", temperature.convert(v)),
        Unit::Percent => format!("{v:.0}"),
        Unit::MegaHertz if v >= 1000.0 => format!("{:.1}", v / 1000.0),
        Unit::MegaHertz | Unit::Rpm if v < 10_000.0 => format!("{v:.0}"),
        Unit::Rpm => format!("{:.0}k", v / 1000.0),
        Unit::Volt => format!("{v:.2}"),
        Unit::Watt | Unit::Ampere if v.abs() >= 10.0 => format!("{v:.0}"),
        Unit::Watt | Unit::Ampere => format!("{v:.1}"),
        Unit::Bool => (if v != 0.0 { "Y" } else { "N" }).to_string(),
        Unit::Bytes | Unit::BytesPerSecond => {
            let mut value = v.abs();
            let mut suffix = "";
            for next in ["K", "M", "G", "T"] {
                if value < 1000.0 {
                    break;
                }
                value /= 1024.0;
                suffix = next;
            }
            let digits = if value >= 10.0 || suffix.is_empty() {
                format!("{value:.0}")
            } else {
                format!("{value:.1}")
            };
            format!("{digits}{suffix}")
        }
        _ => short(v),
    }
}

/// Signed distance from `(x, y)` to a rounded square of side `size` and corner radius `r`.
fn rounded_square_distance(x: f32, y: f32, size: f32, r: f32) -> f32 {
    let half = size / 2.0;
    let (qx, qy) = ((x - half).abs() - (half - r), (y - half).abs() - (half - r));
    let outside = (qx.max(0.0).powi(2) + qy.max(0.0).powi(2)).sqrt();
    outside + qx.max(qy).min(0.0) - r
}

/// `text` in `color` on a dark rounded backdrop, as a `size` × `size` ARGB32 image in
/// network byte order (what the StatusNotifierItem protocol carries).
pub fn render_icon(text: &str, color: Color32, size: u32) -> Vec<u8> {
    let font = FontRef::try_from_slice(epaint_default_fonts::HACK_REGULAR)
        .expect("egui's bundled Hack font parses");
    let side = size as f32;
    let n = (size * size) as usize;

    // Fit the text: about two thirds of the height, and the width with a small margin.
    let reference = font.as_scaled(PxScale::from(100.0));
    let ids: Vec<_> = text.chars().map(|c| font.glyph_id(c)).collect();
    let width_at_100: f32 = ids
        .iter()
        .map(|&id| reference.h_advance(id))
        .sum::<f32>()
        .max(1.0);
    let px = (side * 0.72).min(side * 0.90 * 100.0 / width_at_100);
    let scale = PxScale::from(px);
    let scaled = font.as_scaled(scale);
    let mut x = 0.0;
    let glyphs: Vec<Glyph> = ids
        .iter()
        .map(|&id| {
            let glyph = id.with_scale_and_position(scale, point(x, 0.0));
            x += scaled.h_advance(id);
            glyph
        })
        .collect();
    let outlines: Vec<_> = glyphs
        .into_iter()
        .filter_map(|g| font.outline_glyph(g))
        .collect();
    let mut text_alpha = vec![0f32; n];
    if let Some((min_x, min_y, max_x, max_y)) =
        outlines
            .iter()
            .map(|o| o.px_bounds())
            .fold(None::<(f32, f32, f32, f32)>, |acc, b| {
                Some(match acc {
                    None => (b.min.x, b.min.y, b.max.x, b.max.y),
                    Some((a, b0, c, d)) => (
                        a.min(b.min.x),
                        b0.min(b.min.y),
                        c.max(b.max.x),
                        d.max(b.max.y),
                    ),
                })
            })
    {
        // Centre the ink, not the advance box.
        let dx = ((side - (max_x - min_x)) / 2.0 - min_x).round();
        let dy = ((side - (max_y - min_y)) / 2.0 - min_y).round();
        for outline in &outlines {
            let bounds = outline.px_bounds();
            outline.draw(|gx, gy, coverage| {
                let px = (bounds.min.x + dx) as i32 + gx as i32;
                let py = (bounds.min.y + dy) as i32 + gy as i32;
                if (0..size as i32).contains(&px) && (0..size as i32).contains(&py) {
                    let i = py as usize * size as usize + px as usize;
                    text_alpha[i] = text_alpha[i].max(coverage);
                }
            });
        }
        // A thin font at icon size: thicken by one pixel at larger sizes.
        if size >= 32 {
            let thin = text_alpha.clone();
            for y in 0..size as usize {
                for x in 1..size as usize {
                    let i = y * size as usize + x;
                    text_alpha[i] = thin[i].max(thin[i - 1]);
                }
            }
        }
    }

    let backdrop = Color32::from_rgb(BACKDROP[0], BACKDROP[1], BACKDROP[2]);
    let ink = DARK.readable_on(color, &[backdrop], TEXT_CONTRAST);
    let radius = (side * 0.18).max(2.0);
    let mut out = Vec::with_capacity(n * 4);
    for y in 0..size {
        for x in 0..size {
            let d = rounded_square_distance(x as f32 + 0.5, y as f32 + 0.5, side, radius);
            let shape = (0.5 - d).clamp(0.0, 1.0);
            let t = text_alpha[(y * size + x) as usize].clamp(0.0, 1.0);
            let mix = |a: u8, b: u8| (f32::from(a) * (1.0 - t) + f32::from(b) * t).round() as u8;
            let alpha = (f32::from(BACKDROP[3]) * (1.0 - t) + 255.0 * t) * shape;
            out.extend_from_slice(&[
                alpha.round() as u8,
                mix(BACKDROP[0], ink.r()),
                mix(BACKDROP[1], ink.g()),
                mix(BACKDROP[2], ink.b()),
            ]);
        }
    }
    out
}

#[cfg(target_os = "linux")]
pub use sni::TrayManager;

#[cfg(not(target_os = "linux"))]
pub use elsewhere::TrayManager;

/// What the user did in a tray item.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TrayEvent {
    /// A click on the item: show or hide the main window.
    Toggle,
    Show,
    Hide,
    /// "Remove from tray".
    Remove(ChannelId),
    Quit,
}

#[cfg(not(target_os = "linux"))]
mod elsewhere {
    use super::{TrayEvent, TrayView};

    /// No tray outside Linux.
    pub struct TrayManager;

    impl TrayManager {
        pub fn new(_ctx: egui::Context) -> Self {
            Self
        }

        pub fn sync(&mut self, _wanted: Vec<TrayView>, _window_hidden: bool) {}

        pub fn events(&mut self) -> Vec<TrayEvent> {
            Vec::new()
        }

        /// Whether any item is in the tray (and the window can hide there).
        pub fn active(&self) -> bool {
            false
        }

        /// Why items could not be shown.
        pub fn problem(&self) -> Option<&str> {
            Some("Tray values need a Linux desktop with a system tray (StatusNotifierItem).")
        }
    }
}

#[cfg(target_os = "linux")]
mod sni {
    use std::collections::HashMap;
    use std::sync::mpsc::{self, Receiver, Sender};
    use std::time::{Duration, Instant};

    use ksni::blocking::TrayMethods as _;
    use ksni::menu::StandardItem;
    use ksni::{Icon, MenuItem, ToolTip};
    use kwinfo_core::ChannelId;
    use tracing::{info, warn};

    use super::{ICON_SIZES, TrayEvent, TrayView, render_icon};

    /// After a failure, how long until an item is tried again.
    const RETRY: Duration = Duration::from_secs(30);

    struct ValueTray {
        channel: ChannelId,
        title: String,
        tooltip: String,
        icons: Vec<Icon>,
        window_hidden: bool,
        tx: Sender<TrayEvent>,
        ctx: egui::Context,
    }

    impl ValueTray {
        fn send(&self, event: TrayEvent) {
            let _ = self.tx.send(event);
            // The main window may be hidden; its logic still runs on a repaint request.
            self.ctx.request_repaint();
        }
    }

    fn icons(view: &TrayView) -> Vec<Icon> {
        ICON_SIZES
            .iter()
            .map(|&size| Icon {
                width: size as i32,
                height: size as i32,
                data: render_icon(&view.text, view.color, size),
            })
            .collect()
    }

    impl ksni::Tray for ValueTray {
        fn id(&self) -> String {
            let key: String = self
                .channel
                .as_str()
                .chars()
                .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
                .collect();
            format!("kwinfo-{key}")
        }

        fn title(&self) -> String {
            format!("KWinfo: {}", self.title)
        }

        fn category(&self) -> ksni::Category {
            ksni::Category::Hardware
        }

        fn icon_pixmap(&self) -> Vec<Icon> {
            self.icons.clone()
        }

        fn tool_tip(&self) -> ToolTip {
            ToolTip {
                icon_name: String::new(),
                icon_pixmap: Vec::new(),
                title: self.title.clone(),
                description: self.tooltip.clone(),
            }
        }

        fn activate(&mut self, _x: i32, _y: i32) {
            self.send(TrayEvent::Toggle);
        }

        fn menu(&self) -> Vec<MenuItem<Self>> {
            let show = if self.window_hidden {
                "Show KWinfo"
            } else {
                "Hide KWinfo"
            };
            vec![
                StandardItem {
                    label: show.into(),
                    activate: Box::new(|tray: &mut Self| {
                        let event = if tray.window_hidden {
                            TrayEvent::Show
                        } else {
                            TrayEvent::Hide
                        };
                        tray.send(event);
                    }),
                    ..Default::default()
                }
                .into(),
                StandardItem {
                    label: "Remove from tray".into(),
                    activate: Box::new(|tray: &mut Self| {
                        tray.send(TrayEvent::Remove(tray.channel.clone()))
                    }),
                    ..Default::default()
                }
                .into(),
                MenuItem::Separator,
                StandardItem {
                    label: "Quit KWinfo".into(),
                    activate: Box::new(|tray: &mut Self| tray.send(TrayEvent::Quit)),
                    ..Default::default()
                }
                .into(),
            ]
        }
    }

    struct Item {
        handle: ksni::blocking::Handle<ValueTray>,
        shown: TrayView,
        window_hidden: bool,
    }

    /// The tray items, kept in step with the sensors the settings put in the tray.
    pub struct TrayManager {
        items: HashMap<ChannelId, Item>,
        failed: HashMap<ChannelId, Instant>,
        problem: Option<String>,
        tx: Sender<TrayEvent>,
        rx: Receiver<TrayEvent>,
        ctx: egui::Context,
    }

    impl TrayManager {
        pub fn new(ctx: egui::Context) -> Self {
            let (tx, rx) = mpsc::channel();
            Self {
                items: HashMap::new(),
                failed: HashMap::new(),
                problem: None,
                tx,
                rx,
                ctx,
            }
        }

        /// Adds, updates and removes items so that exactly `wanted` are in the tray.
        pub fn sync(&mut self, wanted: Vec<TrayView>, window_hidden: bool) {
            self.items.retain(|channel, item| {
                let keep = wanted.iter().any(|v| &v.channel == channel) && !item.handle.is_closed();
                if !keep {
                    let _ = item.handle.shutdown();
                    info!(target: "kwinfo::tray", channel = %channel, "tray item removed");
                }
                keep
            });
            for view in wanted {
                match self.items.get_mut(&view.channel) {
                    Some(item) => {
                        if item.shown != view || item.window_hidden != window_hidden {
                            let redraw =
                                item.shown.text != view.text || item.shown.color != view.color;
                            let new_icons = redraw.then(|| icons(&view));
                            let (title, tooltip) = (view.title.clone(), view.tooltip.clone());
                            item.handle.update(move |tray| {
                                tray.title = title;
                                tray.tooltip = tooltip;
                                tray.window_hidden = window_hidden;
                                if let Some(icons) = new_icons {
                                    tray.icons = icons;
                                }
                            });
                            item.shown = view;
                            item.window_hidden = window_hidden;
                        }
                    }
                    None => self.spawn(view, window_hidden),
                }
            }
        }

        fn spawn(&mut self, view: TrayView, window_hidden: bool) {
            if self
                .failed
                .get(&view.channel)
                .is_some_and(|t| t.elapsed() < RETRY)
            {
                return;
            }
            let tray = ValueTray {
                channel: view.channel.clone(),
                title: view.title.clone(),
                tooltip: view.tooltip.clone(),
                icons: icons(&view),
                window_hidden,
                tx: self.tx.clone(),
                ctx: self.ctx.clone(),
            };
            match tray.spawn() {
                Ok(handle) => {
                    info!(target: "kwinfo::tray", channel = %view.channel, "tray item added");
                    self.failed.remove(&view.channel);
                    self.problem = None;
                    self.items.insert(
                        view.channel.clone(),
                        Item {
                            handle,
                            shown: view,
                            window_hidden,
                        },
                    );
                }
                Err(err) => {
                    let problem = match err {
                        ksni::Error::Watcher(_) | ksni::Error::WontShow => {
                            "No system tray is running (a StatusNotifierItem host): KDE has one; \
                             on GNOME, install the AppIndicator extension."
                                .to_string()
                        }
                        other => format!("The tray item could not be created: {other}"),
                    };
                    warn!(target: "kwinfo::tray", channel = %view.channel, "{problem}");
                    self.problem = Some(problem);
                    self.failed.insert(view.channel, Instant::now());
                }
            }
        }

        pub fn events(&mut self) -> Vec<TrayEvent> {
            self.rx.try_iter().collect()
        }

        /// Whether any item is in the tray (and the window can hide there).
        pub fn active(&self) -> bool {
            !self.items.is_empty()
        }

        /// Why items could not be shown.
        pub fn problem(&self) -> Option<&str> {
            self.problem.as_deref()
        }
    }

    impl Drop for TrayManager {
        fn drop(&mut self) {
            for item in self.items.values() {
                let _ = item.handle.shutdown();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values_fit_in_a_few_characters() {
        let c = TemperatureUnit::Celsius;
        assert_eq!(tray_text(Some(48.4), Unit::Celsius, c), "48");
        assert_eq!(
            tray_text(Some(48.4), Unit::Celsius, TemperatureUnit::Fahrenheit),
            "119"
        );
        assert_eq!(tray_text(Some(17.4), Unit::Percent, c), "17");
        assert_eq!(tray_text(Some(4312.0), Unit::MegaHertz, c), "4.3");
        assert_eq!(tray_text(Some(800.0), Unit::MegaHertz, c), "800");
        assert_eq!(tray_text(Some(1.2049), Unit::Volt, c), "1.20");
        assert_eq!(tray_text(Some(37.6), Unit::Watt, c), "38");
        assert_eq!(tray_text(Some(2.64), Unit::Watt, c), "2.6");
        assert_eq!(tray_text(Some(1100.0), Unit::Rpm, c), "1100");
        assert_eq!(tray_text(Some(12_400.0), Unit::Rpm, c), "12k");
        assert_eq!(tray_text(Some(1.0), Unit::Bool, c), "Y");
        assert_eq!(
            tray_text(Some(2.5 * 1024.0 * 1024.0), Unit::BytesPerSecond, c),
            "2.5M"
        );
        assert_eq!(tray_text(Some(512.0), Unit::Bytes, c), "512");
        assert_eq!(tray_text(None, Unit::Volt, c), "-");
    }

    #[test]
    fn icons_are_argb_squares_with_the_value_in_the_middle() {
        for size in ICON_SIZES {
            let data = render_icon("48", Color32::from_rgb(0xe5, 0x73, 0x73), size);
            assert_eq!(data.len(), (size * size * 4) as usize);
            let pixel = |x: u32, y: u32| {
                let i = ((y * size + x) * 4) as usize;
                [data[i], data[i + 1], data[i + 2], data[i + 3]]
            };
            assert_eq!(pixel(0, 0)[0], 0, "the rounded corner is transparent");
            assert!(pixel(size / 2, 1)[0] > 200, "the backdrop is nearly opaque");
            // Some pixel near the middle is the (light) text colour, not the backdrop.
            let inked = (size / 4..size * 3 / 4)
                .flat_map(|y| (size / 4..size * 3 / 4).map(move |x| (x, y)))
                .any(|(x, y)| {
                    let [a, r, ..] = pixel(x, y);
                    a == 255 && r > 180
                });
            assert!(inked, "{size} px icon has no text");
            assert_eq!(
                data,
                render_icon("48", Color32::from_rgb(0xe5, 0x73, 0x73), size),
                "deterministic"
            );
        }
        // Long values shrink to fit rather than spill.
        let wide = render_icon("4096", Color32::WHITE, 24);
        let edge_ink = (0..24).any(|y| {
            let i = (y * 24) as usize * 4;
            wide[i] == 255 && wide[i + 1] > 180
        });
        assert!(!edge_ink, "nothing is drawn on the left edge");
    }
}
