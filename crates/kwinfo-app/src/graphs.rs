//! Graph windows (PLAN.md sections 2 and 4.2): which graphs are open, showing each one
//! as a separate native window (an egui deferred viewport) or all of them in a panel on
//! the right of the main window, tiling and closing them, and keeping `config.graphs` in
//! step so that they come back after a restart.
//!
//! Graph windows are drawn in egui passes of their own, also while the main window
//! sleeps, so what they share with it lives in [`GraphHub`] behind a mutex: each graph's
//! settings, what the main window last learnt about its sensor (name, colour, unit), and
//! what the window reports back (its size and position, a request to close).

use std::collections::HashSet;
use std::ops::RangeInclusive;
use std::sync::Arc;

use egui::{
    Align, CentralPanel, Color32, Context, Frame, Label, Layout, Margin, Pos2, Rect, Response,
    RichText, ScrollArea, Stroke, Ui, Vec2, ViewportBuilder, ViewportClass, ViewportCommand,
    ViewportId, pos2, vec2,
};
use kwinfo_core::config::{GraphEntry, SPAN_CHOICES_S};
use kwinfo_core::{ChannelId, Config, GraphMode, Kind, Sampler, Snapshot, TemperatureUnit, Unit};
use kwinfo_diag::action;
use parking_lot::Mutex;

use crate::app::{APP_ID, KwinfoApp};
use crate::graph::{self, GraphChange, GraphInput};
use crate::icons::{self, Icon};
use crate::theme::{Palette, ThemeTokens};

/// A new graph window's inner size, and the size Tile gives every window.
pub const WINDOW_SIZE: Vec2 = vec2(520.0, 190.0);
/// Below this a graph window's controls stop fitting.
pub const MIN_WINDOW_SIZE: Vec2 = vec2(300.0, graph::MIN_HEIGHT + 12.0);
/// The graph panel's width until the user drags it.
pub const DEFAULT_PANEL_WIDTH: f32 = 430.0;
const PANEL_WIDTH_RANGE: RangeInclusive<f32> = 260.0..=2400.0;
/// Cards in the panel are at least this wide; a wider panel gets more columns.
const CARD_MIN_WIDTH: f32 = 330.0;
const CARD_GRAPH_HEIGHT: f32 = 140.0;
const GAP: f32 = 8.0;
/// How far Tile shifts each round of windows that no longer fit on the monitor.
const CASCADE: f32 = 24.0;

/// Where the graphs are shown.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Placement {
    /// Each in its own window.
    Windows,
    /// All in a panel on the right of the main window.
    Panel,
}

/// Where graphs go for the setting `mode`. On Wayland the compositor, not the
/// application, places windows, so Automatic means the panel there; an egui backend
/// that cannot open more windows (the tests' harness) always gets the panel.
pub fn placement(mode: GraphMode, wayland: bool, embedded: bool) -> Placement {
    if embedded {
        return Placement::Panel;
    }
    match mode {
        GraphMode::Windows => Placement::Windows,
        GraphMode::Panel => Placement::Panel,
        GraphMode::Auto if wayland => Placement::Panel,
        GraphMode::Auto => Placement::Windows,
    }
}

/// Whether windows open on Wayland, where applications cannot choose window positions.
/// winit, under eframe, uses Wayland whenever `WAYLAND_DISPLAY` is set.
pub fn is_wayland() -> bool {
    cfg!(target_os = "linux") && std::env::var_os("WAYLAND_DISPLAY").is_some_and(|d| !d.is_empty())
}

/// The viewport of the graph of `channel`; stable, so a reopened graph is the same window.
pub fn viewport_id(channel: &ChannelId) -> ViewportId {
    ViewportId::from_hash_of(("kwinfo-graph", channel.as_str()))
}

/// Where Tile puts `count` windows whose outer size is `cell`: in columns to the right of
/// the main window (outer rectangle `main`), level with its top, as far as the monitor
/// (of size `monitor`, assumed to be the one under the main window, with monitors of
/// that size side by side) has room; if no column fits to the right, from the monitor's
/// left edge. Rounds of windows beyond what fits cascade from the first position.
pub fn tile_positions(main: Rect, monitor: Vec2, count: usize, cell: Vec2, gap: f32) -> Vec<Pos2> {
    if count == 0 || monitor.x <= 0.0 || monitor.y <= 0.0 {
        return Vec::new();
    }
    let origin = pos2(
        (main.center().x / monitor.x).floor() * monitor.x,
        (main.center().y / monitor.y).floor() * monitor.y,
    );
    // Negative room casts to 0.
    let fits = |room: f32, size: f32| ((room + gap) / (size + gap)).floor() as usize;
    let right = main.right() + gap;
    let beside = fits(origin.x + monitor.x - right, cell.x);
    let (left, columns) = if beside >= 1 {
        (right, beside)
    } else {
        (origin.x, fits(monitor.x, cell.x).max(1))
    };
    let top = main.top().max(origin.y);
    let rows = fits(origin.y + monitor.y - top, cell.y).max(1);
    let per_round = columns * rows;
    (0..count)
        .map(|i| {
            let (round, slot) = (i / per_round, i % per_round);
            let (column, row) = (slot / rows, slot % rows);
            pos2(
                left + column as f32 * (cell.x + gap) + round as f32 * CASCADE,
                top + row as f32 * (cell.y + gap) + round as f32 * CASCADE,
            )
        })
        .collect()
}

/// What a graph shows about its sensor, refreshed by the main window every frame.
#[derive(Clone, Debug, PartialEq)]
struct View {
    title: String,
    device: String,
    kind: Kind,
    unit: Unit,
    decimals: u8,
    temperature: TemperatureUnit,
    /// The series colour before the theme adapts it.
    color: Color32,
    disabled: bool,
}

struct Graph {
    entry: GraphEntry,
    viewport: ViewportId,
    /// `None` while the sensor is not in the snapshot: before the first enumeration, or
    /// when the hardware is gone. Such graphs stay in the settings but are not shown.
    view: Option<View>,
    /// The settings its window was created with, while it is shown as a window. egui
    /// turns differences from the last builder into window commands, so this stays as
    /// created (apart from the title) instead of following the window's position.
    builder: Option<ViewportBuilder>,
    /// Closed from its window, card or menu (how, for the log); removed next frame.
    closed: Option<&'static str>,
}

impl Graph {
    fn new(entry: GraphEntry) -> Self {
        Self {
            viewport: viewport_id(&entry.channel),
            entry,
            view: None,
            builder: None,
            closed: None,
        }
    }
}

#[derive(Default)]
struct Hub {
    graphs: Vec<Graph>,
    /// The graphs shown as windows, for the sampler thread's repaint requests.
    windows: Vec<ViewportId>,
    history_s: u32,
    /// Window positions can be restored and stored (not on Wayland).
    positions: bool,
}

/// A graph as the panel shows it.
struct Card {
    viewport: ViewportId,
    channel: ChannelId,
    title: String,
    device: String,
}

/// The open graphs, shared by the main window and the graph windows.
#[derive(Clone, Default)]
pub struct GraphHub(Arc<Mutex<Hub>>);

impl GraphHub {
    /// The graphs of `entries`, as the settings file lists them.
    pub fn new(entries: &[GraphEntry]) -> Self {
        let graphs = entries.iter().cloned().map(Graph::new).collect();
        Self(Arc::new(Mutex::new(Hub {
            graphs,
            ..Hub::default()
        })))
    }

    /// The sensors with an open graph, in the order the graphs were opened.
    pub fn channels(&self) -> Vec<ChannelId> {
        self.0
            .lock()
            .graphs
            .iter()
            .map(|g| g.entry.channel.clone())
            .collect()
    }

    pub fn len(&self) -> usize {
        self.0.lock().graphs.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Asks egui to repaint the main window and every graph window: the sampler calls
    /// this after each tick.
    pub fn request_repaints(&self, ctx: &Context) {
        let windows = self.0.lock().windows.clone();
        ctx.request_repaint_of(ViewportId::ROOT);
        for viewport in windows {
            ctx.request_repaint_of(viewport);
        }
    }

    fn windows(&self) -> Vec<ViewportId> {
        self.0.lock().windows.clone()
    }

    fn cards(&self) -> Vec<Card> {
        self.0
            .lock()
            .graphs
            .iter()
            .filter_map(|g| {
                let view = g.view.as_ref()?;
                Some(Card {
                    viewport: g.viewport,
                    channel: g.entry.channel.clone(),
                    title: view.title.clone(),
                    device: view.device.clone(),
                })
            })
            .collect()
    }

    fn get(&self, viewport: ViewportId) -> Option<(GraphEntry, Option<View>, u32)> {
        let hub = self.0.lock();
        let graph = hub.graphs.iter().find(|g| g.viewport == viewport)?;
        Some((graph.entry.clone(), graph.view.clone(), hub.history_s))
    }

    fn close(&self, viewport: ViewportId, via: &'static str, ctx: &Context) {
        if let Some(graph) = self
            .0
            .lock()
            .graphs
            .iter_mut()
            .find(|g| g.viewport == viewport)
        {
            graph.closed = Some(via);
        }
        ctx.request_repaint_of(ViewportId::ROOT);
    }

    /// Records what a graph window reports about itself; returns whether the main window
    /// needs to know (to save the settings or to close the window).
    fn track(
        &self,
        viewport: ViewportId,
        inner: Option<Rect>,
        outer: Option<Rect>,
        close: bool,
    ) -> bool {
        let mut hub = self.0.lock();
        let positions = hub.positions;
        let Some(graph) = hub.graphs.iter_mut().find(|g| g.viewport == viewport) else {
            return false;
        };
        let entry = &mut graph.entry;
        let before = (entry.width, entry.height, entry.x, entry.y);
        if let Some(rect) = inner.filter(|r| r.is_positive()) {
            entry.width = Some(rect.width().round());
            entry.height = Some(rect.height().round());
        }
        if positions && let Some(rect) = outer {
            entry.x = Some(rect.min.x.round());
            entry.y = Some(rect.min.y.round());
        }
        if close {
            graph.closed = Some("window");
        }
        close || before != (entry.width, entry.height, entry.x, entry.y)
    }

    /// Applies what the user did in a graph and logs it.
    fn apply(&self, viewport: ViewportId, change: &GraphChange, sampler: &Sampler, ctx: &Context) {
        let channel = {
            let mut hub = self.0.lock();
            let Some(graph) = hub.graphs.iter_mut().find(|g| g.viewport == viewport) else {
                return;
            };
            change.apply_to(&mut graph.entry);
            if change.close {
                graph.closed = Some("graph menu");
            }
            graph.entry.channel.clone()
        };
        if let Some((min, max)) = change.range {
            action!("graph.range", channel = %channel, min, max);
        }
        if let Some(on) = change.autofit {
            action!("graph.autofit", channel = %channel, on);
        }
        if let Some(seconds) = change.span_s {
            action!("graph.span", channel = %channel, seconds);
        }
        if change.reset {
            action!("graph.reset", channel = %channel);
            sampler.reset_channel(channel);
        }
        // The main window saves the change (and closes the graph); this one redraws.
        ctx.request_repaint_of(ViewportId::ROOT);
        ctx.request_repaint();
    }
}

fn tokens_of(ctx: &Context) -> &'static ThemeTokens {
    ThemeTokens::for_palette(Palette::from_theme(ctx.theme()))
}

/// What the graph of `id` shows about it, if the sensor is in the snapshot.
fn view_of(
    snapshot: &Snapshot,
    config: &Config,
    tokens: &ThemeTokens,
    id: &ChannelId,
) -> Option<View> {
    let (device, entry) = snapshot.devices.iter().find_map(|d| {
        d.channels
            .iter()
            .find(|c| &c.channel.id == id)
            .map(|c| (d, c))
    })?;
    let channel = &entry.channel;
    Some(View {
        title: config.label_of(channel).to_string(),
        device: device.device.label.clone(),
        kind: channel.kind,
        unit: channel.unit,
        decimals: channel.decimals,
        temperature: config.general.temperature_unit,
        color: config.color_of(id).map_or_else(
            || tokens.graph_color(channel.kind),
            |rgb| Color32::from_rgb(rgb.r, rgb.g, rgb.b),
        ),
        disabled: entry.disabled || config.is_disabled(channel),
    })
}

/// How a graph's window is created: its last size, and its last position where the
/// desktop lets applications place windows.
fn window_builder(entry: &GraphEntry, positions: bool) -> ViewportBuilder {
    let size = entry
        .size()
        .map_or(WINDOW_SIZE, |[w, h]| vec2(w, h))
        .max(MIN_WINDOW_SIZE);
    let builder = ViewportBuilder::default()
        .with_app_id(APP_ID)
        .with_inner_size(size)
        .with_min_inner_size(MIN_WINDOW_SIZE);
    match entry.position() {
        Some([x, y]) if positions => builder.with_position(pos2(x, y)),
        _ => builder,
    }
}

/// A graph window's whole content. Runs in the window's own egui pass.
fn window_ui(
    ui: &mut Ui,
    class: ViewportClass,
    hub: &GraphHub,
    sampler: &Sampler,
    viewport: ViewportId,
) {
    let ctx = ui.ctx().clone();
    let tokens = tokens_of(&ctx);
    if class == ViewportClass::EmbeddedWindow {
        // The backend cannot open windows and egui wrapped this one in an egui::Window.
        // KWinfo shows the panel instead in that case, so this is only a fallback.
        ui.set_min_size(WINDOW_SIZE);
        graph_body(ui, hub, sampler, viewport, tokens);
        return;
    }
    let (inner, outer, minimized, close) = ctx.input(|i| {
        let v = i.viewport();
        (v.inner_rect, v.outer_rect, v.minimized, v.close_requested())
    });
    let (inner, outer) = if minimized == Some(true) {
        (None, None)
    } else {
        (inner, outer)
    };
    if hub.track(viewport, inner, outer, close) {
        ctx.request_repaint_of(ViewportId::ROOT);
    }
    CentralPanel::default()
        .frame(
            Frame::new()
                .fill(tokens.window_bg)
                .inner_margin(Margin::same(6)),
        )
        .show(ui, |ui| graph_body(ui, hub, sampler, viewport, tokens));
}

/// The graph itself, in the space `ui` offers: in a window or a panel card.
fn graph_body(
    ui: &mut Ui,
    hub: &GraphHub,
    sampler: &Sampler,
    viewport: ViewportId,
    tokens: &ThemeTokens,
) {
    let Some((entry, view, history_s)) = hub.get(viewport) else {
        return;
    };
    let Some(view) = view else {
        ui.centered_and_justified(|ui| {
            ui.label(RichText::new("The sensor is not available").color(tokens.text_secondary));
        });
        return;
    };
    let snapshot = sampler.snapshot();
    let now = snapshot
        .taken_at
        .saturating_duration_since(snapshot.started_at)
        .as_secs_f64();
    let start = now - f64::from(entry.span_s);
    let points = sampler
        .with_history(&entry.channel, |h| h.points_since(start))
        .unwrap_or_default();
    let current = snapshot
        .find(&entry.channel)
        .and_then(|c| c.stats.current());
    let input = GraphInput {
        id: ui.scope_id().with(("graph", entry.channel.as_str())),
        name: &view.title,
        points: &points,
        now,
        current,
        kind: view.kind,
        unit: view.unit,
        decimals: view.decimals,
        temperature: view.temperature,
        color: view.color,
        history_s,
        disabled: view.disabled,
    };
    let change = graph::show(ui, &input, &entry, tokens);
    if !change.is_empty() {
        hub.apply(viewport, &change, sampler, ui.ctx());
    }
}

/// One graph in the panel: its name, its device, a close button and the graph.
fn graph_card(
    ui: &mut Ui,
    hub: &GraphHub,
    sampler: &Sampler,
    card: &Card,
    tokens: &ThemeTokens,
) -> Response {
    Frame::new()
        .fill(tokens.table_bg)
        .stroke(Stroke::new(1.0, tokens.grid))
        .corner_radius(4)
        .inner_margin(Margin::same(6))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new(&card.title).strong());
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let name = format!("Close the graph of {}", card.title);
                    if icons::button(ui, Icon::Close, None, &name)
                        .on_hover_text("Close this graph")
                        .clicked()
                    {
                        hub.close(card.viewport, "panel", ui.ctx());
                    }
                    ui.add(
                        Label::new(
                            RichText::new(&card.device)
                                .small()
                                .color(tokens.text_secondary),
                        )
                        .truncate(),
                    );
                });
            });
            let size = vec2(ui.available_width(), CARD_GRAPH_HEIGHT);
            ui.allocate_ui(size, |ui| {
                graph_body(ui, hub, sampler, card.viewport, tokens)
            });
        })
        .response
}

impl KwinfoApp {
    /// Where graphs are shown right now.
    pub fn graph_placement(&self, ctx: &Context) -> Placement {
        placement(self.config.graphs.mode, self.wayland, ctx.embed_viewports())
    }

    /// The sensors with an open graph, in the order the graphs were opened.
    pub fn open_graphs(&self) -> Vec<ChannelId> {
        self.graphs.channels()
    }

    /// The open graphs' sensors, for the table's graph markers.
    pub(crate) fn graphed(&self) -> HashSet<ChannelId> {
        self.graphs.channels().into_iter().collect()
    }

    /// Opens the graph of `id`, or brings it forward if it is open.
    pub fn open_graph(&mut self, ctx: &Context, id: &ChannelId) {
        let placement = self.graph_placement(ctx);
        let mut hub = self.graphs.0.lock();
        if let Some(graph) = hub.graphs.iter().find(|g| &g.entry.channel == id) {
            match placement {
                Placement::Windows => {
                    ctx.send_viewport_cmd_to(graph.viewport, ViewportCommand::Focus);
                }
                Placement::Panel => self.graph_scroll_to = Some(id.clone()),
            }
            return;
        }
        let kind = self
            .snapshot
            .find(id)
            .map_or(Kind::Other, |c| c.channel.kind);
        let entry = graph::new_entry(id.clone(), kind, self.config.graphs.history_s());
        hub.graphs.push(Graph::new(entry));
        drop(hub);
        action!("graph.open", channel = %id, placement = ?placement);
        self.graph_scroll_to = Some(id.clone());
    }

    pub(crate) fn close_all_graphs(&mut self) {
        let count = {
            let mut hub = self.graphs.0.lock();
            let count = hub.graphs.len();
            hub.graphs.clear();
            count
        };
        if count > 0 {
            action!("graph.close_all", count);
        }
    }

    pub(crate) fn set_graph_mode(&mut self, mode: GraphMode) {
        if self.config.graphs.mode != mode {
            action!("graph.mode", mode = ?mode);
            self.config.graphs.mode = mode;
        }
    }

    /// The user picked a history length: the sampler keeps that much, and graphs showing
    /// more than that show the longest span that fits.
    pub(crate) fn set_history_minutes(&mut self, minutes: u32) {
        if self.config.graphs.history_minutes == minutes {
            return;
        }
        action!("settings.graph_history", minutes);
        self.config.graphs.history_minutes = minutes;
        self.sync_history_capacity();
        let longest = self.config.graphs.history_s();
        let fit = SPAN_CHOICES_S
            .into_iter()
            .filter(|&s| s <= longest)
            .max()
            .unwrap_or(SPAN_CHOICES_S[0]);
        for graph in &mut self.graphs.0.lock().graphs {
            if graph.entry.span_s > longest {
                graph.entry.span_s = fit;
            }
        }
    }

    /// Tells the sampler how much history to keep for the current settings.
    pub(crate) fn sync_history_capacity(&self) {
        self.sampler
            .set_history_capacity(self.config.graphs.history_points(self.interval_ms));
    }

    /// Gives every graph window the default size and, where the desktop allows it,
    /// lines them up beside the main window.
    pub(crate) fn tile_graphs(&mut self, ctx: &Context) {
        let windows = self.graphs.windows();
        if windows.is_empty() {
            return;
        }
        let (outer, inner, monitor) = ctx.input(|i| {
            let v = i.viewport();
            (v.outer_rect, v.inner_rect, v.monitor_size)
        });
        // Graph windows get the same title bar and frame as the main window.
        let decorations = match (outer, inner) {
            (Some(o), Some(i)) => (o.size() - i.size()).max(Vec2::ZERO),
            _ => vec2(0.0, 28.0),
        };
        let positions = match (outer, monitor) {
            (Some(main), Some(monitor)) if !self.wayland => {
                tile_positions(main, monitor, windows.len(), WINDOW_SIZE + decorations, GAP)
            }
            _ => Vec::new(),
        };
        for (i, viewport) in windows.iter().enumerate() {
            ctx.send_viewport_cmd_to(*viewport, ViewportCommand::InnerSize(WINDOW_SIZE));
            if let Some(&position) = positions.get(i) {
                ctx.send_viewport_cmd_to(*viewport, ViewportCommand::OuterPosition(position));
            }
        }
        action!(
            "graph.tile",
            count = windows.len(),
            positioned = !positions.is_empty()
        );
    }

    /// Once per main-window frame, before the central panel: brings every graph up to
    /// date with the snapshot and the settings, shows the graph windows or the panel,
    /// and writes the graphs back into the settings.
    pub(crate) fn graphs_frame(&mut self, ui: &mut Ui, snapshot: &Snapshot) {
        let ctx = ui.ctx().clone();
        let tokens = self.tokens();
        let placement = self.graph_placement(&ctx);
        let mut windows = Vec::new();
        {
            let mut hub = self.graphs.0.lock();
            hub.history_s = self.config.graphs.history_s();
            hub.positions = !self.wayland;
            hub.graphs.retain(|g| match g.closed {
                Some(via) => {
                    action!("graph.close", channel = %g.entry.channel, via);
                    false
                }
                None => true,
            });
            let positions = hub.positions;
            for graph in &mut hub.graphs {
                graph.view = view_of(snapshot, &self.config, tokens, &graph.entry.channel);
                match (placement, &graph.view) {
                    (Placement::Windows, Some(view)) => {
                        let title = format!("{} – {}", view.title, view.device);
                        let builder = graph
                            .builder
                            .get_or_insert_with(|| window_builder(&graph.entry, positions));
                        if builder.title.as_deref() != Some(title.as_str()) {
                            *builder = builder.clone().with_title(title);
                        }
                        windows.push((graph.viewport, builder.clone()));
                    }
                    _ => graph.builder = None,
                }
            }
            hub.windows = windows.iter().map(|(viewport, _)| *viewport).collect();
        }
        for (viewport, builder) in windows {
            let hub = self.graphs.clone();
            let sampler = Arc::clone(&self.sampler);
            ctx.show_viewport_deferred(viewport, builder, move |ui, class| {
                window_ui(ui, class, &hub, &sampler, viewport);
            });
            ctx.request_repaint_of(viewport);
        }
        if placement == Placement::Panel {
            self.graph_panel(ui, tokens);
        }
        let entries: Vec<GraphEntry> = self
            .graphs
            .0
            .lock()
            .graphs
            .iter()
            .map(|g| g.entry.clone())
            .collect();
        if self.config.graphs.open != entries {
            self.config.graphs.open = entries;
        }
    }

    /// The panel on the right of the main window, with every shown graph as a card.
    fn graph_panel(&mut self, ui: &mut Ui, tokens: &ThemeTokens) {
        let cards = self.graphs.cards();
        if cards.is_empty() {
            return;
        }
        let configured = self
            .config
            .graphs
            .panel_width
            .unwrap_or(DEFAULT_PANEL_WIDTH);
        let scroll_to = self.graph_scroll_to.take();
        let (hub, sampler) = (self.graphs.clone(), Arc::clone(&self.sampler));
        let mut close_all = false;
        let panel = egui::Panel::right("graph-panel")
            .resizable(true)
            .default_size(configured)
            .size_range(PANEL_WIDTH_RANGE)
            .frame(
                Frame::new()
                    .fill(tokens.window_bg)
                    .inner_margin(Margin::same(6)),
            )
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    icons::icon(ui, Icon::Graph, tokens.text_secondary);
                    ui.label(RichText::new("Graphs").strong());
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if icons::button(ui, Icon::Close, None, "Close all graphs")
                            .on_hover_text("Close all graphs")
                            .clicked()
                        {
                            close_all = true;
                        }
                    });
                });
                ui.add_space(4.0);
                ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        let per_row = (((ui.available_width() + GAP) / (CARD_MIN_WIDTH + GAP))
                            .floor() as usize)
                            .clamp(1, cards.len());
                        for row in cards.chunks(per_row) {
                            ui.columns(per_row, |columns| {
                                for (column, card) in columns.iter_mut().zip(row) {
                                    let response = graph_card(column, &hub, &sampler, card, tokens);
                                    if scroll_to.as_ref() == Some(&card.channel) {
                                        response.scroll_to_me(Some(Align::Center));
                                    }
                                }
                            });
                            ui.add_space(GAP);
                        }
                    });
            });
        if close_all {
            self.close_all_graphs();
        }
        // Only widths the user drags are settings; egui may also clamp the panel when the
        // window is narrow.
        let width = panel.response.rect.width().round();
        if ui.input(|i| i.pointer.primary_down()) && (width - configured).abs() >= 1.0 {
            self.config.graphs.panel_width =
                ((width - DEFAULT_PANEL_WIDTH).abs() >= 1.0).then_some(width);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn automatic_means_windows_except_on_wayland() {
        assert_eq!(placement(GraphMode::Auto, false, false), Placement::Windows);
        assert_eq!(placement(GraphMode::Auto, true, false), Placement::Panel);
        assert_eq!(
            placement(GraphMode::Windows, true, false),
            Placement::Windows
        );
        assert_eq!(placement(GraphMode::Panel, false, false), Placement::Panel);
        assert_eq!(
            placement(GraphMode::Windows, false, true),
            Placement::Panel,
            "no windows without a backend that opens them"
        );
    }

    #[test]
    fn tiles_fill_columns_beside_the_main_window() {
        let main = Rect::from_min_size(pos2(0.0, 0.0), vec2(780.0, 720.0));
        let cell = vec2(520.0, 220.0);
        let p = tile_positions(main, vec2(1920.0, 1080.0), 9, cell, 8.0);
        assert_eq!(p.len(), 9);
        assert_eq!(p[0], pos2(788.0, 0.0));
        assert_eq!(p[1], pos2(788.0, 228.0));
        assert_eq!(p[3], pos2(788.0, 684.0));
        assert_eq!(p[4], pos2(1316.0, 0.0), "second column");
        assert_eq!(p[8], pos2(788.0 + CASCADE, CASCADE), "the ninth cascades");
    }

    #[test]
    fn tiles_start_at_the_left_edge_when_nothing_fits_beside() {
        let main = Rect::from_min_size(pos2(1400.0, 100.0), vec2(500.0, 600.0));
        let p = tile_positions(main, vec2(1920.0, 1080.0), 2, vec2(520.0, 220.0), 8.0);
        assert_eq!(p[0], pos2(0.0, 100.0));
        assert_eq!(p[1], pos2(0.0, 328.0));
    }

    #[test]
    fn tiles_stay_on_the_main_windows_monitor() {
        let main = Rect::from_min_size(pos2(1920.0 + 100.0, 0.0), vec2(780.0, 720.0));
        let p = tile_positions(main, vec2(1920.0, 1080.0), 1, vec2(520.0, 220.0), 8.0);
        assert_eq!(p[0], pos2(1920.0 + 888.0, 0.0));
        assert!(tile_positions(main, Vec2::ZERO, 3, vec2(1.0, 1.0), 8.0).is_empty());
        assert!(tile_positions(main, vec2(1920.0, 1080.0), 0, vec2(1.0, 1.0), 8.0).is_empty());
    }

    #[test]
    fn windows_reopen_at_their_size_and_place() {
        let mut entry = GraphEntry::new(ChannelId::from_raw("cpu:0/temp1"));
        let fresh = window_builder(&entry, true);
        assert_eq!(fresh.inner_size, Some(WINDOW_SIZE));
        assert_eq!(fresh.position, None);
        entry.width = Some(100.0);
        entry.height = Some(400.0);
        entry.x = Some(10.0);
        entry.y = Some(20.0);
        let restored = window_builder(&entry, true);
        assert_eq!(
            restored.inner_size,
            Some(vec2(MIN_WINDOW_SIZE.x, 400.0)),
            "never below the minimum"
        );
        assert_eq!(restored.position, Some(pos2(10.0, 20.0)));
        assert_eq!(window_builder(&entry, false).position, None, "Wayland");
    }

    #[test]
    fn hub_tracks_window_geometry_and_close_requests() {
        let id = ChannelId::from_raw("cpu:0/temp1");
        let hub = GraphHub::new(&[GraphEntry::new(id.clone())]);
        hub.0.lock().positions = true;
        let viewport = viewport_id(&id);
        let inner = Rect::from_min_size(pos2(110.0, 230.0), vec2(500.4, 180.6));
        let outer = Rect::from_min_size(pos2(100.0, 200.0), vec2(520.0, 220.0));
        assert!(hub.track(viewport, Some(inner), Some(outer), false));
        assert!(
            !hub.track(viewport, Some(inner), Some(outer), false),
            "nothing new"
        );
        let (entry, _, _) = hub.get(viewport).unwrap();
        assert_eq!(entry.size(), Some([500.0, 181.0]));
        assert_eq!(entry.position(), Some([100.0, 200.0]));
        assert!(hub.track(viewport, None, None, true));
        assert_eq!(hub.0.lock().graphs[0].closed, Some("window"));
        assert!(!hub.track(viewport_id(&ChannelId::from_raw("x/y")), None, None, true));
    }
}
