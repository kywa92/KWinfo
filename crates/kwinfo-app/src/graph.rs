//! The graph of one sensor, drawn like HWiNFO's (PLAN.md 4.2): the readings of the last
//! few minutes as a filled area under a line on a grid, and beside it a column with the
//! top of the range, the current value, Auto Fit, Reset and the bottom of the range.
//!
//! It is painted with egui's painter rather than a plotting library: the look is fixed,
//! there is no zooming or panning to switch off, and the pictures are deterministic for
//! the snapshot tests. Everything here works in the unit the user sees (°F when chosen);
//! only [`GraphChange::range`] goes back to the channel's base unit.

use egui::{
    Align2, Button, Color32, CornerRadius, FontId, Id, Key, Label, Mesh, Pos2, Rect, RichText,
    Role, Sense, Shape, Stroke, StrokeKind, TextEdit, Ui, WidgetInfo, pos2, vec2,
};
use kwinfo_core::config::{DEFAULT_SPAN_S, GraphEntry, SPAN_CHOICES_S};
use kwinfo_core::{
    ChannelId, Kind, TemperatureUnit, Unit, format_input, format_value, from_display, parse_input,
    to_display,
};

use crate::theme::{TEXT_CONTRAST, ThemeTokens};

/// Width of the column beside the plot.
pub const COLUMN_WIDTH: f32 = 92.0;
/// The smallest height at which the column's controls all fit.
pub const MIN_HEIGHT: f32 = 128.0;
/// Space between the plot and the column.
const COLUMN_GAP: f32 = 6.0;
/// Height of the column's fields and buttons.
const CONTROL_HEIGHT: f32 = 20.0;
const LINE_WIDTH: f32 = 1.5;
/// Opacity of the area under the line.
const FILL_OPACITY: f32 = 0.45;
/// Horizontal grid lines divide the range into this many bands.
const BANDS: usize = 10;
/// Time between vertical grid lines, chosen so that a graph shows at most about a dozen.
const GRID_STEPS_S: [f64; 11] = [
    5.0, 10.0, 15.0, 30.0, 60.0, 120.0, 300.0, 600.0, 900.0, 1800.0, 3600.0,
];

/// What a graph shows; the caller collects it from the sampler and the settings.
pub struct GraphInput<'a> {
    /// Distinguishes this graph's widgets from other graphs' in the same window.
    pub id: Id,
    /// The sensor's name, for the accessible names of the controls.
    pub name: &'a str,
    /// Readings as `(seconds since the sampler started, value in the base unit)`, oldest
    /// first; `NaN` marks a reading that failed.
    pub points: &'a [(f64, f32)],
    /// The time at the graph's right edge, in the same seconds.
    pub now: f64,
    pub current: Option<f64>,
    pub kind: Kind,
    pub unit: Unit,
    pub decimals: u8,
    pub temperature: TemperatureUnit,
    /// The series colour before it is adapted to the theme: the kind's colour or the
    /// user's custom colour.
    pub color: Color32,
    /// How much history the sampler keeps, in seconds; longer spans are not offered.
    pub history_s: u32,
    /// Monitoring is switched off for the sensor.
    pub disabled: bool,
}

/// What the user did in a graph. The caller applies it to the graph's settings.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GraphChange {
    pub autofit: Option<bool>,
    /// A new fixed range, in the channel's base unit. Implies Auto Fit off.
    pub range: Option<(f64, f64)>,
    pub span_s: Option<u32>,
    /// Clear the sensor's history and statistics.
    pub reset: bool,
    pub close: bool,
}

impl GraphChange {
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }

    /// Applies the change to `entry`; `reset` and `close` are left to the caller.
    pub fn apply_to(&self, entry: &mut GraphEntry) {
        if let Some((lo, hi)) = self.range {
            entry.y_min = Some(lo);
            entry.y_max = Some(hi);
            entry.autofit = false;
        }
        if let Some(on) = self.autofit {
            entry.autofit = on;
        }
        if let Some(span) = self.span_s {
            entry.span_s = span;
        }
    }
}

/// The range a kind of reading starts with, in the base unit, as HWiNFO does for
/// temperatures and loads. `None`: the readings decide (Auto Fit).
pub fn default_range(kind: Kind) -> Option<(f64, f64)> {
    match kind {
        Kind::Temperature | Kind::Usage | Kind::Pwm => Some((0.0, 100.0)),
        Kind::Boolean => Some((0.0, 1.0)),
        _ => None,
    }
}

/// The settings of a new graph: kinds with a natural range start fixed to it, the others
/// follow their readings; the time span fits in the history.
pub fn new_entry(channel: ChannelId, kind: Kind, history_s: u32) -> GraphEntry {
    let mut entry = GraphEntry::new(channel);
    entry.autofit = default_range(kind).is_none();
    entry.span_s = DEFAULT_SPAN_S.min(history_s.max(SPAN_CHOICES_S[0]));
    entry
}

/// `"10 min"`, `"1 h"`, `"45 s"`.
pub fn span_label(seconds: u32) -> String {
    match seconds {
        s if s >= 3600 && s % 3600 == 0 => format!("{} h", s / 3600),
        s if s >= 60 && s % 60 == 0 => format!("{} min", s / 60),
        s => format!("{s} s"),
    }
}

/// The smallest of 1, 2 and 5 times a power of ten that is at least `x`.
pub fn nice_step(x: f64) -> f64 {
    if !(x.is_finite() && x > 0.0) {
        return 1.0;
    }
    let power = 10f64.powf(x.log10().floor());
    [1.0, 2.0, 5.0, 10.0]
        .into_iter()
        .map(|m| m * power)
        .find(|&step| step >= x * (1.0 - 1e-9))
        .unwrap_or(10.0 * power)
}

/// The Auto Fit range for readings between `lo` and `hi`: 5 % of headroom on both sides,
/// rounded outwards to a step of 1, 2 or 5 times a power of ten (about a tenth of the
/// range), and not below zero for readings that are not.
pub fn auto_range(lo: f64, hi: f64) -> (f64, f64) {
    let (lo, hi) = if lo <= hi { (lo, hi) } else { (hi, lo) };
    let pad = if hi > lo {
        (hi - lo) * 0.05
    } else if hi != 0.0 {
        hi.abs() * 0.05
    } else {
        0.5
    };
    let (mut a, b) = (lo - pad, hi + pad);
    if lo >= 0.0 && a < 0.0 {
        a = 0.0;
    }
    let step = nice_step((b - a) / BANDS as f64);
    let tidy = |v: f64| {
        let v = (v / step).round() * step;
        if v == 0.0 { 0.0 } else { v }
    };
    // The epsilon keeps float dust (1.14 / 0.02 = 56.999…) from adding a whole step.
    let (a, b) = (
        (a / step + 1e-9).floor() * step,
        (b / step - 1e-9).ceil() * step,
    );
    (tidy(a), tidy(b))
}

/// The y range to draw, in the unit shown: the fixed range unless Auto Fit is on (or no
/// fixed range is known), otherwise the readings' range, otherwise something sensible.
pub fn display_range(entry: &GraphEntry, input: &GraphInput<'_>, values: &[f32]) -> (f64, f64) {
    let shown = |(lo, hi): (f64, f64)| {
        (
            to_display(lo, input.unit, input.temperature),
            to_display(hi, input.unit, input.temperature),
        )
    };
    let fixed = match (entry.y_min, entry.y_max) {
        (Some(lo), Some(hi)) if lo < hi => Some((lo, hi)),
        _ => default_range(input.kind),
    }
    .map(shown);
    if !entry.autofit
        && let Some(range) = fixed
    {
        return range;
    }
    let finite = values
        .iter()
        .filter(|v| v.is_finite())
        .map(|&v| f64::from(v));
    let (lo, hi) = finite.fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), v| {
        (lo.min(v), hi.max(v))
    });
    if lo <= hi {
        auto_range(lo, hi)
    } else {
        fixed.unwrap_or((0.0, 1.0))
    }
}

/// The points to draw when the plot, covering `start..end`, is `columns` pixels wide:
/// all of them when that is at most two per column, otherwise each column's lowest and
/// highest reading in the order they came, so that spikes survive. Gaps are kept.
pub fn decimate(points: &[(f64, f32)], start: f64, end: f64, columns: usize) -> Vec<(f64, f32)> {
    let columns = columns.max(1);
    if points.len() <= columns * 2 {
        return points.to_vec();
    }
    struct Bucket {
        column: i64,
        low: (f64, f32),
        high: (f64, f32),
    }
    fn flush(bucket: &mut Option<Bucket>, out: &mut Vec<(f64, f32)>) {
        if let Some(b) = bucket.take() {
            let (first, second) = if b.low.0 <= b.high.0 {
                (b.low, b.high)
            } else {
                (b.high, b.low)
            };
            out.push(first);
            if second != first {
                out.push(second);
            }
        }
    }
    let width = (end - start).max(1e-9) / columns as f64;
    let mut out = Vec::with_capacity(columns * 2 + 4);
    let mut bucket: Option<Bucket> = None;
    for &(t, v) in points {
        if v.is_nan() {
            flush(&mut bucket, &mut out);
            if out.last().is_none_or(|p| !p.1.is_nan()) {
                out.push((t, f32::NAN));
            }
            continue;
        }
        let column = ((t - start) / width).floor() as i64;
        match &mut bucket {
            Some(b) if b.column == column => {
                if v < b.low.1 {
                    b.low = (t, v);
                }
                if v > b.high.1 {
                    b.high = (t, v);
                }
            }
            _ => {
                flush(&mut bucket, &mut out);
                bucket = Some(Bucket {
                    column,
                    low: (t, v),
                    high: (t, v),
                });
            }
        }
    }
    flush(&mut bucket, &mut out);
    out
}

/// `"now"` or `"1:23 ago"`.
fn ago(seconds: f64) -> String {
    let s = seconds.max(0.0).round() as u64;
    match s {
        0 => "now".to_string(),
        s if s >= 3600 => format!("{}:{:02}:{:02} ago", s / 3600, (s / 60) % 60, s % 60),
        s => format!("{}:{:02} ago", s / 60, s % 60),
    }
}

/// Shows the graph in all of the space `ui` offers and reports what the user did.
pub fn show(
    ui: &mut Ui,
    input: &GraphInput<'_>,
    entry: &GraphEntry,
    tokens: &ThemeTokens,
) -> GraphChange {
    let mut change = GraphChange::default();
    let size = ui
        .available_size()
        .max(vec2(COLUMN_WIDTH * 2.0, MIN_HEIGHT));
    let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
    let plot = Rect::from_min_max(
        rect.min,
        pos2(rect.right() - COLUMN_WIDTH - COLUMN_GAP, rect.bottom()),
    );
    let column = Rect::from_min_max(pos2(rect.right() - COLUMN_WIDTH, rect.top()), rect.max);

    let span = f64::from(entry.span_s.max(1));
    let (start, end) = (input.now - span, input.now);
    let shown: Vec<(f64, f32)> = input
        .points
        .iter()
        .map(|&(t, v)| {
            let v = if v.is_finite() {
                to_display(f64::from(v), input.unit, input.temperature) as f32
            } else {
                f32::NAN
            };
            (t, v)
        })
        .collect();
    let visible: Vec<f32> = shown
        .iter()
        .filter(|&&(t, _)| t >= start)
        .map(|&(_, v)| v)
        .collect();
    let range = display_range(entry, input, &visible);
    let line = tokens.graph_line(input.color);
    let fill = input.color.gamma_multiply(FILL_OPACITY);

    let columns = (plot.width() * ui.ctx().pixels_per_point()).round() as usize;
    let drawn = decimate(&shown, start, end, columns);
    paint_plot(ui, plot, &drawn, (start, end), range, line, fill, tokens);
    if visible.iter().all(|v| !v.is_finite()) {
        let note = if input.disabled {
            "Monitoring is disabled"
        } else {
            "Waiting for readings"
        };
        ui.painter_at(plot).text(
            plot.center(),
            Align2::CENTER_CENTER,
            note,
            FontId::proportional(12.0),
            tokens.text_secondary,
        );
    }

    let response = ui.interact(plot, input.id.with("plot"), Sense::click());
    response
        .widget_info(|| WidgetInfo::labeled(Role::Image, true, format!("Graph of {}", input.name)));
    if let Some(pointer) = response.hover_pos() {
        hover(
            ui,
            &response,
            pointer,
            plot,
            &shown,
            (start, end),
            range,
            line,
            input,
        );
    }
    let base_range = (
        from_display(range.0, input.unit, input.temperature),
        from_display(range.1, input.unit, input.temperature),
    );
    response.context_menu(|ui| {
        menu(ui, input, entry, base_range, &mut change);
    });

    // The value sits on the window (or the panel card), not on the plot.
    let text = tokens.readable_on(
        input.color,
        &[tokens.window_bg, tokens.table_bg],
        TEXT_CONTRAST,
    );
    controls(ui, column, input, entry, range, text, &mut change);
    change
}

#[allow(clippy::too_many_arguments)]
fn paint_plot(
    ui: &Ui,
    rect: Rect,
    points: &[(f64, f32)],
    (start, end): (f64, f64),
    (lo, hi): (f64, f64),
    line: Color32,
    fill: Color32,
    tokens: &ThemeTokens,
) {
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, CornerRadius::ZERO, tokens.graph_bg);
    let grid = Stroke::new(1.0, tokens.graph_grid);
    for band in 1..BANDS {
        let y =
            painter.round_to_pixel_center(rect.top() + rect.height() * band as f32 / BANDS as f32);
        painter.hline(rect.x_range(), y, grid);
    }
    let span = end - start;
    let step = GRID_STEPS_S
        .into_iter()
        .find(|&s| span / s <= 12.0)
        .unwrap_or(span / 12.0)
        .max(1e-3);
    let x_of = |t: f64| rect.left() + ((t - start) / span) as f32 * rect.width();
    // Vertical lines sit on whole multiples of the step, so they move with the readings.
    let mut t = (start / step).ceil() * step;
    while t < end {
        painter.vline(painter.round_to_pixel_center(x_of(t)), rect.y_range(), grid);
        t += step;
    }

    let height = (hi - lo).max(f64::MIN_POSITIVE);
    let y_of = |v: f32| {
        let y = rect.bottom() - ((f64::from(v) - lo) / height) as f32 * rect.height();
        y.clamp(rect.top() - 2.0, rect.bottom() + 2.0)
    };
    let mut mesh = Mesh::default();
    let mut segment: Vec<Pos2> = Vec::new();
    let mut lines: Vec<Vec<Pos2>> = Vec::new();
    let base = rect.bottom();
    for pair in points.windows(2) {
        let ((t0, v0), (t1, v1)) = (pair[0], pair[1]);
        if v0.is_finite() && v1.is_finite() {
            let (x0, x1) = (x_of(t0), x_of(t1));
            let (y0, y1) = (y_of(v0), y_of(v1));
            let i = mesh.vertices.len() as u32;
            mesh.colored_vertex(pos2(x0, y0), fill);
            mesh.colored_vertex(pos2(x1, y1), fill);
            mesh.colored_vertex(pos2(x1, base), fill);
            mesh.colored_vertex(pos2(x0, base), fill);
            mesh.add_triangle(i, i + 1, i + 2);
            mesh.add_triangle(i, i + 2, i + 3);
        }
    }
    for &(t, v) in points {
        if v.is_finite() {
            segment.push(pos2(x_of(t), y_of(v)));
        } else if !segment.is_empty() {
            lines.push(std::mem::take(&mut segment));
        }
    }
    if !segment.is_empty() {
        lines.push(segment);
    }
    painter.add(Shape::mesh(mesh));
    for points in lines {
        if points.len() == 1 {
            painter.circle_filled(points[0], LINE_WIDTH, line);
        } else {
            painter.add(Shape::line(points, Stroke::new(LINE_WIDTH, line)));
        }
    }
    painter.rect_stroke(rect, CornerRadius::ZERO, grid, StrokeKind::Inside);
    painter.text(
        rect.left_bottom() + vec2(4.0, -3.0),
        Align2::LEFT_BOTTOM,
        span_label(span.round() as u32),
        FontId::proportional(10.5),
        tokens.text_secondary,
    );
}

/// The reading under the pointer: a marker on the line and a tooltip.
#[allow(clippy::too_many_arguments)]
fn hover(
    ui: &Ui,
    response: &egui::Response,
    pointer: Pos2,
    rect: Rect,
    points: &[(f64, f32)],
    (start, end): (f64, f64),
    (lo, hi): (f64, f64),
    line: Color32,
    input: &GraphInput<'_>,
) {
    let t = start + f64::from((pointer.x - rect.left()) / rect.width()) * (end - start);
    let after = points.partition_point(|p| p.0 < t);
    let nearest = [after.checked_sub(1), Some(after)]
        .into_iter()
        .flatten()
        .filter_map(|i| points.get(i))
        .filter(|p| p.0 >= start && p.1.is_finite())
        .min_by(|a, b| (a.0 - t).abs().total_cmp(&(b.0 - t).abs()));
    let Some(&(pt, pv)) = nearest else {
        return;
    };
    let x = rect.left() + ((pt - start) / (end - start)) as f32 * rect.width();
    let y = rect.bottom()
        - ((f64::from(pv) - lo) / (hi - lo).max(f64::MIN_POSITIVE)) as f32 * rect.height();
    let painter = ui.painter_at(rect);
    painter.vline(
        x,
        rect.y_range(),
        Stroke::new(1.0, line.gamma_multiply(0.6)),
    );
    painter.circle_filled(pos2(x, y.clamp(rect.top(), rect.bottom())), 3.0, line);
    let base = from_display(f64::from(pv), input.unit, input.temperature);
    let value = format_value(Some(base), input.unit, input.decimals, input.temperature);
    let when = ago(end - pt);
    response.clone().on_hover_ui_at_pointer(|ui| {
        ui.label(RichText::new(value).strong());
        ui.label(when);
    });
}

/// The plot's context menu. `range` is the range shown, in the base unit: switching Auto
/// Fit off keeps it, as the button does.
fn menu(
    ui: &mut Ui,
    input: &GraphInput<'_>,
    entry: &GraphEntry,
    range: (f64, f64),
    change: &mut GraphChange,
) {
    ui.set_min_width(170.0);
    ui.label(RichText::new("Show the last").small());
    let longest = input.history_s.max(SPAN_CHOICES_S[0]);
    for span in SPAN_CHOICES_S.into_iter().filter(|&s| s <= longest) {
        if ui.radio(entry.span_s == span, span_label(span)).clicked() && entry.span_s != span {
            change.span_s = Some(span);
        }
    }
    ui.separator();
    if ui.selectable_label(entry.autofit, "Auto Fit").clicked() {
        change.autofit = Some(!entry.autofit);
        if entry.autofit {
            change.range = Some(range);
        }
    }
    if ui.button("Reset").clicked() {
        change.reset = true;
    }
    ui.separator();
    if ui.button("Close graph").clicked() {
        change.close = true;
    }
}

/// The column beside the plot: maximum, current value, Auto Fit, Reset, minimum.
fn controls(
    ui: &mut Ui,
    column: Rect,
    input: &GraphInput<'_>,
    entry: &GraphEntry,
    (lo, hi): (f64, f64),
    value_color: Color32,
    change: &mut GraphChange,
) {
    let row =
        |y: f32| Rect::from_min_size(pos2(column.left(), y), vec2(column.width(), CONTROL_HEIGHT));
    let to_base = |v: f64| from_display(v, input.unit, input.temperature);
    let (lo_base, hi_base) = (to_base(lo), to_base(hi));
    let name = input.name;

    // A bound that would leave an empty range is ignored (the field shows the old one).
    if let Some(max) = bound_field(
        ui,
        row(column.top()),
        input,
        hi_base,
        &format!("Maximum of {name}"),
    ) && max > lo_base
    {
        change.range = Some((lo_base, max));
    }
    if let Some(min) = bound_field(
        ui,
        row(column.bottom() - CONTROL_HEIGHT),
        input,
        lo_base,
        &format!("Minimum of {name}"),
    ) && min < hi_base
    {
        change.range = Some((min, change.range.map_or(hi_base, |r| r.1)));
    }

    // The value and the two buttons, centred between the fields.
    let middle = column.center().y;
    let gap = 4.0;
    let value_rect = row(middle - CONTROL_HEIGHT * 1.5 - gap);
    let current = if input.disabled {
        RichText::new("Disabled").italics()
    } else {
        RichText::new(format_value(
            input.current,
            input.unit,
            input.decimals,
            input.temperature,
        ))
        .strong()
        .color(value_color)
    };
    ui.put(value_rect, Label::new(current).truncate());

    let autofit = ui.put(
        row(middle - CONTROL_HEIGHT * 0.5),
        Button::new("Auto Fit").selected(entry.autofit),
    );
    let on = entry.autofit;
    autofit
        .widget_info(|| WidgetInfo::selected(Role::Button, true, on, format!("Auto Fit {name}")));
    if autofit
        .on_hover_text("Fit the range to the readings as they come")
        .clicked()
    {
        change.autofit = Some(!on);
        if on {
            // Keep what is shown now as the fixed range, so nothing jumps.
            change.range = Some((lo_base, hi_base));
        }
    }
    let reset = ui.put(
        row(middle + CONTROL_HEIGHT * 0.5 + gap),
        Button::new("Reset"),
    );
    reset.widget_info(|| WidgetInfo::labeled(Role::Button, true, format!("Reset {name}")));
    if reset
        .on_hover_text("Clear this sensor's graph, minimum, maximum and average")
        .clicked()
    {
        change.reset = true;
    }
}

/// A field showing one end of the range; returns the value the user entered (base unit)
/// when they press Enter or leave the field, unless they pressed Escape.
fn bound_field(
    ui: &mut Ui,
    rect: Rect,
    input: &GraphInput<'_>,
    value: f64,
    name: &str,
) -> Option<f64> {
    let id = input.id.with(name);
    let shown = format_input(value, input.unit, input.decimals, input.temperature);
    let mut text = if ui.memory(|m| m.has_focus(id)) {
        ui.data(|d| d.get_temp::<String>(id))
            .unwrap_or_else(|| shown.clone())
    } else {
        shown.clone()
    };
    let response = ui.put(
        rect,
        TextEdit::singleline(&mut text)
            .id(id)
            .align(Align2::RIGHT_CENTER)
            .margin(vec2(4.0, 2.0)),
    );
    let current = text.clone();
    response.widget_info(|| {
        let mut info = WidgetInfo::labeled(Role::TextInput, true, name);
        info.current_text_value = Some(current.clone());
        info
    });
    if response.has_focus() {
        ui.data_mut(|d| d.insert_temp(id, text.clone()));
    }
    if response.lost_focus() {
        ui.data_mut(|d| d.remove::<String>(id));
        if ui.input(|i| i.key_pressed(Key::Escape)) || text.trim() == shown.trim() {
            return None;
        }
        return parse_input(&text, input.unit, input.temperature);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: (f64, f64), b: (f64, f64)) -> bool {
        (a.0 - b.0).abs() < 1e-9 && (a.1 - b.1).abs() < 1e-9
    }

    #[test]
    fn steps_are_one_two_or_five_times_a_power_of_ten() {
        assert_eq!(nice_step(1.903), 2.0);
        assert_eq!(nice_step(0.012), 0.02);
        assert_eq!(nice_step(315.0), 500.0);
        assert_eq!(nice_step(7.0), 10.0);
        assert_eq!(nice_step(1.0), 1.0);
        assert_eq!(nice_step(0.0), 1.0);
        assert_eq!(nice_step(f64::NAN), 1.0);
    }

    #[test]
    fn auto_fit_pads_and_rounds_outwards() {
        assert!(close(auto_range(35.1, 52.4), (34.0, 54.0)));
        assert!(
            close(auto_range(52.4, 35.1), (34.0, 54.0)),
            "order does not matter"
        );
        assert!(
            close(auto_range(1.2, 1.2), (1.14, 1.26)),
            "a flat line gets 5 %"
        );
        assert!(close(auto_range(0.0, 0.0), (0.0, 0.5)));
        assert!(
            close(auto_range(0.0, 3000.0), (0.0, 3500.0)),
            "fans never go negative"
        );
        assert!(
            close(auto_range(-12.0, 4.0), (-14.0, 6.0)),
            "negative readings may"
        );
    }

    #[test]
    fn kinds_with_a_natural_range_start_fixed() {
        let id = ChannelId::from_raw("cpu:0/temp1");
        let temp = new_entry(id.clone(), Kind::Temperature, 3600);
        assert!(!temp.autofit);
        assert_eq!(default_range(Kind::Temperature), Some((0.0, 100.0)));
        let volts = new_entry(id.clone(), Kind::Voltage, 3600);
        assert!(volts.autofit);
        assert_eq!(volts.span_s, DEFAULT_SPAN_S);
        assert_eq!(
            new_entry(id, Kind::Fan, 120).span_s,
            120,
            "the span fits the history"
        );
    }

    fn input<'a>(kind: Kind, unit: Unit, temperature: TemperatureUnit) -> GraphInput<'a> {
        GraphInput {
            id: Id::NULL,
            name: "Test",
            points: &[],
            now: 0.0,
            current: None,
            kind,
            unit,
            decimals: 1,
            temperature,
            color: Color32::RED,
            history_s: 3600,
            disabled: false,
        }
    }

    #[test]
    fn the_range_is_fixed_unless_auto_fit_is_on() {
        let c = TemperatureUnit::Celsius;
        let temp = input(Kind::Temperature, Unit::Celsius, c);
        let mut entry = new_entry(ChannelId::from_raw("x/t"), Kind::Temperature, 3600);
        assert!(close(
            display_range(&entry, &temp, &[40.0, 60.0]),
            (0.0, 100.0)
        ));
        entry.y_min = Some(20.0);
        entry.y_max = Some(80.0);
        assert!(close(
            display_range(&entry, &temp, &[40.0, 60.0]),
            (20.0, 80.0)
        ));
        entry.autofit = true;
        assert!(close(
            display_range(&entry, &temp, &[40.0, 60.0]),
            (35.0, 65.0)
        ));
        assert!(
            close(display_range(&entry, &temp, &[]), (20.0, 80.0)),
            "no readings yet"
        );

        let fahrenheit = input(
            Kind::Temperature,
            Unit::Celsius,
            TemperatureUnit::Fahrenheit,
        );
        entry.autofit = false;
        assert!(close(
            display_range(&entry, &fahrenheit, &[]),
            (68.0, 176.0)
        ));

        let volts = input(Kind::Voltage, Unit::Volt, c);
        let auto = new_entry(ChannelId::from_raw("x/v"), Kind::Voltage, 3600);
        assert!(close(display_range(&auto, &volts, &[f32::NAN]), (0.0, 1.0)));
    }

    #[test]
    fn changes_apply_range_then_auto_fit() {
        let mut entry = new_entry(ChannelId::from_raw("x/v"), Kind::Voltage, 3600);
        GraphChange {
            range: Some((1.0, 2.0)),
            ..GraphChange::default()
        }
        .apply_to(&mut entry);
        assert!(!entry.autofit);
        assert_eq!((entry.y_min, entry.y_max), (Some(1.0), Some(2.0)));
        GraphChange {
            autofit: Some(true),
            span_s: Some(60),
            ..GraphChange::default()
        }
        .apply_to(&mut entry);
        assert!(entry.autofit);
        assert_eq!(entry.span_s, 60);
        assert!(GraphChange::default().is_empty());
    }

    #[test]
    fn decimation_keeps_spikes_gaps_and_order() {
        let points: Vec<(f64, f32)> = (0..1000)
            .map(|i| {
                let v = match i {
                    500 => 99.0,
                    700 => f32::NAN,
                    _ => 10.0,
                };
                (f64::from(i), v)
            })
            .collect();
        let few = decimate(&points, 0.0, 1000.0, 100);
        assert!(few.len() <= 2 * 100 + 4, "{} points", few.len());
        assert!(few.iter().any(|p| p.1 == 99.0), "the spike survives");
        assert_eq!(few.iter().filter(|p| p.1.is_nan()).count(), 1, "one gap");
        assert!(few.windows(2).all(|w| w[0].0 <= w[1].0), "time order");

        let small = &points[..50];
        assert_eq!(
            decimate(small, 0.0, 50.0, 100),
            small.to_vec(),
            "nothing to drop"
        );
    }

    #[test]
    fn labels() {
        assert_eq!(span_label(600), "10 min");
        assert_eq!(span_label(3600), "1 h");
        assert_eq!(span_label(7200), "2 h");
        assert_eq!(span_label(45), "45 s");
        assert_eq!(ago(0.2), "now");
        assert_eq!(ago(83.0), "1:23 ago");
        assert_eq!(ago(3725.0), "1:02:05 ago");
    }
}
