//! Small modal dialogs: renaming a sensor and choosing its colour.

use egui::{Align, Align2, Color32, DragValue, FontId, Id, Key, Layout, Mesh, Modal, Rect};
use egui::{Response, RichText, Role, Sense, Shape, Stroke, TextEdit, Ui, WidgetInfo, pos2, vec2};
use kwinfo_core::{ChannelId, Rgb};

use crate::theme::{DARK, LIGHT, ThemeTokens};

/// How a dialog closed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome<T> {
    Cancel,
    Apply(T),
}

/// "Rename…" from a sensor's context menu.
pub struct RenameDialog {
    pub channel: ChannelId,
    original: String,
    text: String,
    focus: bool,
}

impl RenameDialog {
    /// `original` is the backend's label, `current` the name shown now.
    pub fn new(channel: ChannelId, original: &str, current: &str) -> Self {
        Self {
            channel,
            original: original.to_string(),
            text: current.to_string(),
            focus: true,
        }
    }

    /// Shows the dialog; returns the outcome when it closes. `Apply(None)` goes back to
    /// the original name.
    pub fn show(
        &mut self,
        ctx: &egui::Context,
        tokens: &ThemeTokens,
    ) -> Option<Outcome<Option<String>>> {
        let mut outcome = None;
        let modal = Modal::new(Id::unique("rename-sensor")).show(ctx, |ui| {
            ui.set_width(340.0);
            ui.heading("Rename sensor");
            ui.add_space(2.0);
            ui.label(
                RichText::new(format!("Original name: {}", self.original))
                    .color(tokens.text_secondary),
            );
            ui.add_space(8.0);
            let label = ui.label("New name");
            let edit = ui
                .add(
                    TextEdit::singleline(&mut self.text)
                        .hint_text(self.original.as_str())
                        .desired_width(f32::INFINITY),
                )
                .labelled_by(label.id);
            if std::mem::take(&mut self.focus) {
                edit.request_focus();
            }
            let entered = edit.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                if ui
                    .button("Original name")
                    .on_hover_text("Go back to the name the sensor came with")
                    .clicked()
                {
                    outcome = Some(Outcome::Apply(None));
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if ui.button("OK").clicked() || entered {
                        outcome = Some(self.apply());
                    }
                    if ui.button("Cancel").clicked() {
                        outcome = Some(Outcome::Cancel);
                    }
                });
            });
        });
        if outcome.is_none() && modal.should_close() {
            outcome = Some(Outcome::Cancel);
        }
        outcome
    }

    fn apply(&self) -> Outcome<Option<String>> {
        let name = self.text.trim();
        Outcome::Apply((!name.is_empty() && name != self.original).then(|| name.to_string()))
    }
}

/// "Other colour…" from the Custom colour menu.
pub struct ColorDialog {
    pub channel: ChannelId,
    label: String,
    hsv: Hsv,
    /// The hex field's text while the user types in it.
    hex: String,
}

impl ColorDialog {
    /// `start` is the colour the picker opens with.
    pub fn new(channel: ChannelId, label: &str, start: Color32) -> Self {
        let hsv = Hsv::from_color(start);
        Self {
            channel,
            label: label.to_string(),
            hex: hsv.rgb().to_string(),
            hsv,
        }
    }

    /// Shows the dialog; returns the outcome when it closes. `Apply(None)` goes back to
    /// the default colour.
    pub fn show(
        &mut self,
        ctx: &egui::Context,
        tokens: &ThemeTokens,
    ) -> Option<Outcome<Option<Rgb>>> {
        let mut outcome = None;
        let modal = Modal::new(Id::unique("sensor-colour")).show(ctx, |ui| {
            ui.set_width(300.0);
            ui.heading("Sensor colour");
            ui.label(RichText::new(&self.label).color(tokens.text_secondary));
            ui.add_space(6.0);
            self.picker(ui, tokens);
            ui.add_space(8.0);
            ui.label("Each theme adjusts the lightness so the text stays readable:");
            let rgb = self.hsv.rgb();
            preview(ui, &DARK, rgb, &self.label, "Dark theme");
            preview(ui, &LIGHT, rgb, &self.label, "Light theme");
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                if ui.button("Default colour").clicked() {
                    outcome = Some(Outcome::Apply(None));
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if ui.button("OK").clicked() {
                        outcome = Some(Outcome::Apply(Some(rgb)));
                    }
                    if ui.button("Cancel").clicked() {
                        outcome = Some(Outcome::Cancel);
                    }
                });
            });
        });
        if outcome.is_none() && modal.should_close() {
            outcome = Some(Outcome::Cancel);
        }
        outcome
    }
}

impl ColorDialog {
    /// egui's own colour pickers include a button labelled with an emoji that egui's
    /// bundled fonts lack, so the dialog has its own: a saturation and brightness area,
    /// a hue bar, and hex and R/G/B fields.
    fn picker(&mut self, ui: &mut Ui, tokens: &ThemeTokens) {
        let before = self.hsv;
        sv_area(ui, &mut self.hsv, tokens);
        ui.add_space(4.0);
        hue_bar(ui, &mut self.hsv, tokens);
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            let label = ui.label("Hex");
            let field = ui
                .add(TextEdit::singleline(&mut self.hex).desired_width(76.0))
                .labelled_by(label.id);
            if field.changed()
                && let Ok(rgb) = self.hex.parse::<Rgb>()
            {
                self.hsv = Hsv::from_color(Color32::from_rgb(rgb.r, rgb.g, rgb.b));
            }
            ui.add_space(6.0);
            let rgb = self.hsv.rgb();
            let mut channels = [rgb.r, rgb.g, rgb.b];
            let mut edited = false;
            for (name, value) in ["R", "G", "B"].into_iter().zip(&mut channels) {
                let label = ui.label(name);
                edited |= ui
                    .add(DragValue::new(value).range(0..=255).speed(0.5))
                    .labelled_by(label.id)
                    .changed();
            }
            if edited {
                let [r, g, b] = channels;
                self.hsv = Hsv::from_color(Color32::from_rgb(r, g, b));
            }
            if !field.has_focus() && (self.hsv != before || edited) {
                self.hex = self.hsv.rgb().to_string();
            }
        });
    }
}

/// A colour as hue, saturation and value, all 0–1. Kept apart from RGB so that the hue
/// survives while saturation or value are zero.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Hsv {
    h: f32,
    s: f32,
    v: f32,
}

impl Hsv {
    fn from_color(c: Color32) -> Self {
        let [r, g, b] = [c.r(), c.g(), c.b()].map(|v| f32::from(v) / 255.0);
        let max = r.max(g).max(b);
        let d = max - r.min(g).min(b);
        let h = if d == 0.0 {
            0.0
        } else if max == r {
            ((g - b) / d).rem_euclid(6.0) / 6.0
        } else if max == g {
            ((b - r) / d + 2.0) / 6.0
        } else {
            ((r - g) / d + 4.0) / 6.0
        };
        let s = if max == 0.0 { 0.0 } else { d / max };
        Self { h, s, v: max }
    }

    fn color(self) -> Color32 {
        let h6 = self.h.rem_euclid(1.0) * 6.0;
        let c = self.v * self.s;
        let x = c * (1.0 - (h6.rem_euclid(2.0) - 1.0).abs());
        let (r, g, b) = match h6 as u32 {
            0 => (c, x, 0.0),
            1 => (x, c, 0.0),
            2 => (0.0, c, x),
            3 => (0.0, x, c),
            4 => (x, 0.0, c),
            _ => (c, 0.0, x),
        };
        let m = self.v - c;
        let byte = |v: f32| ((v + m).clamp(0.0, 1.0) * 255.0).round() as u8;
        Color32::from_rgb(byte(r), byte(g), byte(b))
    }

    fn rgb(self) -> Rgb {
        let c = self.color();
        Rgb::new(c.r(), c.g(), c.b())
    }
}

/// A rectangle filled with a grid of coloured vertices, interpolated between them.
fn gradient(
    rect: Rect,
    columns: usize,
    rows: usize,
    color_at: impl Fn(f32, f32) -> Color32,
) -> Shape {
    let mut mesh = Mesh::default();
    for row in 0..=rows {
        for column in 0..=columns {
            let (x, y) = (column as f32 / columns as f32, row as f32 / rows as f32);
            let pos = pos2(
                rect.left() + x * rect.width(),
                rect.top() + y * rect.height(),
            );
            mesh.colored_vertex(pos, color_at(x, y));
        }
    }
    let stride = columns as u32 + 1;
    for row in 0..rows as u32 {
        for column in 0..columns as u32 {
            let i = row * stride + column;
            mesh.add_triangle(i, i + 1, i + stride);
            mesh.add_triangle(i + 1, i + stride + 1, i + stride);
        }
    }
    Shape::mesh(mesh)
}

/// Saturation left to right, brightness bottom to top, at the current hue.
fn sv_area(ui: &mut Ui, hsv: &mut Hsv, tokens: &ThemeTokens) -> Response {
    let (rect, response) =
        ui.allocate_exact_size(vec2(ui.available_width(), 120.0), Sense::click_and_drag());
    response.widget_info(|| WidgetInfo::labeled(Role::Slider, true, "Saturation and brightness"));
    if let Some(pos) = response.interact_pointer_pos() {
        hsv.s = ((pos.x - rect.left()) / rect.width()).clamp(0.0, 1.0);
        hsv.v = 1.0 - ((pos.y - rect.top()) / rect.height()).clamp(0.0, 1.0);
    }
    if ui.is_rect_visible(rect) {
        let h = hsv.h;
        ui.painter().add(gradient(rect, 16, 8, |s, y| {
            Hsv { h, s, v: 1.0 - y }.color()
        }));
        ui.painter().rect_stroke(
            rect,
            0.0,
            Stroke::new(1.0, tokens.grid),
            egui::StrokeKind::Outside,
        );
        let at = pos2(
            rect.left() + hsv.s * rect.width(),
            rect.top() + (1.0 - hsv.v) * rect.height(),
        );
        let ring = if hsv.v > 0.5 {
            Color32::BLACK
        } else {
            Color32::WHITE
        };
        ui.painter().circle_stroke(at, 5.0, Stroke::new(1.5, ring));
    }
    response
}

/// The hue, left to right.
fn hue_bar(ui: &mut Ui, hsv: &mut Hsv, tokens: &ThemeTokens) -> Response {
    let (rect, response) =
        ui.allocate_exact_size(vec2(ui.available_width(), 16.0), Sense::click_and_drag());
    response.widget_info(|| WidgetInfo::labeled(Role::Slider, true, "Hue"));
    if let Some(pos) = response.interact_pointer_pos() {
        hsv.h = ((pos.x - rect.left()) / rect.width()).clamp(0.0, 1.0);
    }
    if ui.is_rect_visible(rect) {
        ui.painter().add(gradient(rect, 36, 1, |h, _| {
            Hsv { h, s: 1.0, v: 1.0 }.color()
        }));
        ui.painter().rect_stroke(
            rect,
            0.0,
            Stroke::new(1.0, tokens.grid),
            egui::StrokeKind::Outside,
        );
        let x = rect.left() + hsv.h * rect.width();
        let marker = Rect::from_min_max(
            pos2(x - 2.0, rect.top() - 2.0),
            pos2(x + 2.0, rect.bottom() + 2.0),
        );
        ui.painter().rect_stroke(
            marker,
            1.0,
            Stroke::new(1.5, tokens.text_primary),
            egui::StrokeKind::Middle,
        );
    }
    response
}

/// A sample row in one palette, with the colour as that palette shows it.
fn preview(ui: &mut Ui, tokens: &ThemeTokens, rgb: Rgb, label: &str, theme: &str) {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 22.0), Sense::hover());
    let painter = ui.painter();
    painter.rect_filled(rect, 2.0, tokens.table_bg);
    let font = FontId::proportional(13.0);
    painter.text(
        rect.left_center() + vec2(8.0, 0.0),
        Align2::LEFT_CENTER,
        label,
        font.clone(),
        tokens.custom_text(rgb),
    );
    painter.text(
        rect.right_center() - vec2(8.0, 0.0),
        Align2::RIGHT_CENTER,
        theme,
        font,
        tokens.text_secondary,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hsv_round_trips_and_keeps_its_hue_at_zero_saturation() {
        for c in [
            Color32::from_rgb(0xff, 0x8c, 0x00),
            Color32::from_rgb(0x1e, 0x88, 0xe5),
            Color32::from_rgb(0x12, 0x34, 0x56),
            Color32::BLACK,
            Color32::WHITE,
        ] {
            assert_eq!(Hsv::from_color(c).color(), c);
        }
        let grey = Hsv {
            h: 0.6,
            s: 0.0,
            v: 0.5,
        };
        assert_eq!(grey.color(), Color32::from_rgb(128, 128, 128));
        assert_eq!(grey.h, 0.6);
    }

    #[test]
    fn renaming_to_nothing_or_the_original_restores_the_default() {
        let id = ChannelId::from_raw("cpu:0/clock.core0");
        let mut dialog = RenameDialog::new(id.clone(), "Core #0 Clock", "Core #0 Clock");
        dialog.text = "  P-core 0 ".into();
        assert_eq!(dialog.apply(), Outcome::Apply(Some("P-core 0".into())));
        dialog.text = "   ".into();
        assert_eq!(dialog.apply(), Outcome::Apply(None));
        dialog.text = "Core #0 Clock".into();
        assert_eq!(dialog.apply(), Outcome::Apply(None));
    }
}
