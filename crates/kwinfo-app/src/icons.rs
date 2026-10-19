//! Small vector icons, painted rather than taken from an icon font.
//!
//! PLAN.md picked Phosphor glyphs through `egui-phosphor`, but no release of that crate
//! (nor of its forks) supports egui 0.37 yet, so the row, device and toolbar icons are
//! drawn with egui's painter. They scale with the UI, take any colour from the theme
//! tokens and need no font. Each icon is designed on a 16 × 16 grid.

use std::f32::consts::{PI, TAU};

use egui::{
    Color32, Painter, Pos2, Rect, Response, Role, Sense, Shape, Stroke, StrokeKind, TextStyle,
    TextWrapMode, Ui, Vec2, WidgetInfo, WidgetText, pos2, vec2,
};
use kwinfo_core::{Category, Kind, Unit};

/// Width and height of a painted icon, in points.
pub const ICON_SIZE: f32 = 14.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Icon {
    // Kinds of reading.
    Thermometer,
    Bolt,
    Wave,
    Plug,
    Battery,
    Fan,
    SquareWave,
    Clock,
    Pie,
    UpDown,
    Stack,
    Hash,
    Toggle,
    Times,
    Diamond,
    // Kinds of device.
    Chip,
    Board,
    MemoryStick,
    GraphicsCard,
    Drive,
    ShieldCheck,
    Globe,
    Sensor,
    // Commands.
    ExpandAll,
    CollapseAll,
    Reset,
    Document,
    Stopwatch,
    Gauge,
    Gear,
    Search,
    Power,
    Close,
    Shield,
    /// A line chart: graphs.
    Graph,
}

impl Icon {
    /// The row icon of a channel.
    pub fn for_channel(kind: Kind, unit: Unit) -> Icon {
        if unit == Unit::Multiplier {
            return Icon::Times;
        }
        match kind {
            Kind::Temperature => Icon::Thermometer,
            Kind::Voltage => Icon::Bolt,
            Kind::Current => Icon::Wave,
            Kind::Power => Icon::Plug,
            Kind::Energy => Icon::Battery,
            Kind::Fan => Icon::Fan,
            Kind::Pwm => Icon::SquareWave,
            Kind::Clock => Icon::Clock,
            Kind::Usage => Icon::Pie,
            Kind::Throughput => Icon::UpDown,
            Kind::Capacity => Icon::Stack,
            Kind::Count => Icon::Hash,
            Kind::Boolean => Icon::Toggle,
            Kind::Other => Icon::Diamond,
        }
    }

    /// The icon of a device group row.
    pub fn for_category(category: Category) -> Icon {
        match category {
            Category::Cpu => Icon::Chip,
            Category::Motherboard => Icon::Board,
            Category::Memory => Icon::MemoryStick,
            Category::Gpu => Icon::GraphicsCard,
            Category::Drive => Icon::Drive,
            Category::Smart => Icon::ShieldCheck,
            Category::Network => Icon::Globe,
            Category::Battery | Category::Ups => Icon::Battery,
            Category::Psu => Icon::Plug,
            Category::Cooler => Icon::Fan,
            Category::Other => Icon::Sensor,
        }
    }
}

/// Draws on the 16 × 16 design grid of one icon, collecting the shapes.
struct Pen {
    shapes: Vec<Shape>,
    origin: Pos2,
    scale: f32,
    color: Color32,
}

impl Pen {
    fn p(&self, (x, y): (f32, f32)) -> Pos2 {
        self.origin + vec2(x, y) * self.scale
    }

    fn stroke(&self, width: f32) -> Stroke {
        Stroke::new(width * self.scale, self.color)
    }

    fn points(&self, points: &[(f32, f32)]) -> Vec<Pos2> {
        points.iter().map(|&q| self.p(q)).collect()
    }

    /// An open polyline.
    fn line(&mut self, width: f32, points: &[(f32, f32)]) {
        let shape = Shape::line(self.points(points), self.stroke(width));
        self.shapes.push(shape);
    }

    /// A closed polyline.
    fn outline(&mut self, width: f32, points: &[(f32, f32)]) {
        let shape = Shape::closed_line(self.points(points), self.stroke(width));
        self.shapes.push(shape);
    }

    /// A filled convex polygon.
    fn fill(&mut self, points: &[(f32, f32)]) {
        let shape = Shape::convex_polygon(self.points(points), self.color, Stroke::NONE);
        self.shapes.push(shape);
    }

    fn circle(&mut self, width: f32, center: (f32, f32), radius: f32) {
        let shape = Shape::circle_stroke(self.p(center), radius * self.scale, self.stroke(width));
        self.shapes.push(shape);
    }

    fn disc(&mut self, center: (f32, f32), radius: f32) {
        let shape = Shape::circle_filled(self.p(center), radius * self.scale, self.color);
        self.shapes.push(shape);
    }

    fn rect(&mut self, width: f32, min: (f32, f32), max: (f32, f32), radius: f32) {
        let shape = Shape::rect_stroke(
            Rect::from_min_max(self.p(min), self.p(max)),
            radius * self.scale,
            self.stroke(width),
            StrokeKind::Middle,
        );
        self.shapes.push(shape);
    }

    fn block(&mut self, min: (f32, f32), max: (f32, f32), radius: f32) {
        let shape = Shape::rect_filled(
            Rect::from_min_max(self.p(min), self.p(max)),
            radius * self.scale,
            self.color,
        );
        self.shapes.push(shape);
    }

    /// Points on a circle, from `from` to `to` degrees, clockwise from east (y points
    /// down, as on screen).
    fn arc_points(center: (f32, f32), radius: f32, from: f32, to: f32) -> Vec<(f32, f32)> {
        let steps = ((to - from).abs() / 12.0).ceil().max(2.0) as usize;
        (0..=steps)
            .map(|i| {
                let a = (from + (to - from) * i as f32 / steps as f32).to_radians();
                (center.0 + radius * a.cos(), center.1 + radius * a.sin())
            })
            .collect()
    }

    fn arc(&mut self, width: f32, center: (f32, f32), radius: f32, from: f32, to: f32) {
        self.line(width, &Self::arc_points(center, radius, from, to));
    }

    /// A filled arrowhead whose tip is `length` beyond `at` in direction `angle`
    /// (degrees, clockwise from east).
    fn arrowhead(&mut self, at: (f32, f32), angle: f32, length: f32, half_width: f32) {
        let (dx, dy) = (angle.to_radians().cos(), angle.to_radians().sin());
        let tip = (at.0 + dx * length, at.1 + dy * length);
        let back = (at.0 - dx * 0.4, at.1 - dy * 0.4);
        self.fill(&[
            tip,
            (back.0 - dy * half_width, back.1 + dx * half_width),
            (back.0 + dy * half_width, back.1 - dx * half_width),
        ]);
    }
}

/// Paints `icon` in a square of [`ICON_SIZE`] centred in `rect`.
pub fn paint(painter: &Painter, rect: Rect, icon: Icon, color: Color32) {
    painter.extend(shapes(rect, icon, color));
}

/// The shapes of `icon` in a square of [`ICON_SIZE`] centred in `rect`.
pub fn shapes(rect: Rect, icon: Icon, color: Color32) -> Vec<Shape> {
    let square = Rect::from_center_size(rect.center(), Vec2::splat(ICON_SIZE));
    let mut pen = Pen {
        shapes: Vec::with_capacity(8),
        origin: square.min,
        scale: ICON_SIZE / 16.0,
        color,
    };
    draw(&mut pen, icon);
    pen.shapes
}

fn draw(pen: &mut Pen, icon: Icon) {
    const W: f32 = 1.5;
    match icon {
        Icon::Thermometer => {
            pen.rect(1.4, (6.2, 1.2), (9.8, 10.6), 1.8);
            pen.disc((8.0, 12.2), 3.1);
            pen.line(1.8, &[(8.0, 10.5), (8.0, 4.8)]);
        }
        Icon::Bolt => {
            let p = [
                (9.8, 0.8),
                (3.4, 9.0),
                (7.4, 9.0),
                (6.2, 15.2),
                (12.6, 6.6),
                (8.6, 6.6),
            ];
            // Concave, so filled as two convex halves under a thin outline.
            pen.fill(&[p[0], p[1], p[2], p[5]]);
            pen.fill(&[p[5], p[2], p[3], p[4]]);
            pen.outline(0.6, &p);
        }
        Icon::Wave => {
            let points: Vec<(f32, f32)> = (0..=26)
                .map(|i| {
                    let x = 1.5 + 13.0 * i as f32 / 26.0;
                    (x, 8.0 - 3.6 * (TAU * (x - 1.5) / 13.0).sin())
                })
                .collect();
            pen.line(W, &points);
        }
        Icon::Plug => {
            pen.line(1.7, &[(6.0, 1.0), (6.0, 5.0)]);
            pen.line(1.7, &[(10.0, 1.0), (10.0, 5.0)]);
            pen.block((3.4, 4.8), (12.6, 10.4), 2.2);
            pen.line(1.7, &[(8.0, 10.0), (8.0, 15.2)]);
        }
        Icon::Battery => {
            pen.rect(1.4, (1.0, 4.4), (13.4, 11.6), 1.5);
            pen.block((13.4, 6.4), (15.2, 9.6), 0.6);
            pen.block((2.9, 6.3), (9.4, 9.7), 0.6);
        }
        Icon::Fan => {
            let c = (8.0, 8.0);
            for k in 0..3 {
                let a = -PI / 2.0 + k as f32 * TAU / 3.0;
                let at = |r: f32, da: f32| (c.0 + r * (a + da).cos(), c.1 + r * (a + da).sin());
                pen.fill(&[c, at(2.6, -0.75), at(7.2, -0.15), at(6.6, 0.45)]);
            }
            pen.disc(c, 1.9);
        }
        Icon::SquareWave => pen.line(
            W,
            &[
                (1.0, 11.5),
                (4.5, 11.5),
                (4.5, 4.5),
                (8.0, 4.5),
                (8.0, 11.5),
                (11.5, 11.5),
                (11.5, 4.5),
                (15.0, 4.5),
            ],
        ),
        Icon::Clock => {
            pen.circle(W, (8.0, 8.0), 6.6);
            pen.line(W, &[(8.0, 3.9), (8.0, 8.0), (11.0, 9.8)]);
        }
        Icon::Pie => {
            pen.circle(W, (8.0, 8.0), 6.6);
            let mut wedge = vec![(8.0, 8.0)];
            wedge.extend(Pen::arc_points((8.0, 8.0), 6.6, -90.0, 30.0));
            pen.fill(&wedge);
        }
        Icon::UpDown => {
            pen.line(W, &[(5.0, 14.6), (5.0, 4.0)]);
            pen.arrowhead((5.0, 4.4), -90.0, 3.0, 2.7);
            pen.line(W, &[(11.0, 1.4), (11.0, 12.0)]);
            pen.arrowhead((11.0, 11.6), 90.0, 3.0, 2.7);
        }
        Icon::Stack => {
            pen.rect(1.4, (1.5, 2.4), (14.5, 7.0), 1.2);
            pen.rect(1.4, (1.5, 9.0), (14.5, 13.6), 1.2);
            pen.disc((11.8, 4.7), 0.95);
            pen.disc((11.8, 11.3), 0.95);
        }
        Icon::Hash => {
            pen.line(1.4, &[(6.4, 1.8), (5.0, 14.2)]);
            pen.line(1.4, &[(11.0, 1.8), (9.6, 14.2)]);
            pen.line(1.4, &[(2.4, 5.8), (14.0, 5.8)]);
            pen.line(1.4, &[(2.0, 10.2), (13.6, 10.2)]);
        }
        Icon::Toggle => {
            pen.rect(1.4, (1.0, 4.0), (15.0, 12.0), 4.0);
            pen.disc((11.0, 8.0), 2.6);
        }
        Icon::Times => {
            pen.line(1.9, &[(4.0, 4.0), (12.0, 12.0)]);
            pen.line(1.9, &[(12.0, 4.0), (4.0, 12.0)]);
        }
        Icon::Diamond => pen.fill(&[(8.0, 2.4), (13.6, 8.0), (8.0, 13.6), (2.4, 8.0)]),
        Icon::Chip => {
            pen.rect(1.4, (3.8, 3.8), (12.2, 12.2), 1.2);
            pen.block((6.4, 6.4), (9.6, 9.6), 0.4);
            for v in [6.3, 9.7] {
                pen.line(1.3, &[(v, 1.0), (v, 3.8)]);
                pen.line(1.3, &[(v, 12.2), (v, 15.0)]);
                pen.line(1.3, &[(1.0, v), (3.8, v)]);
                pen.line(1.3, &[(12.2, v), (15.0, v)]);
            }
        }
        Icon::Board => {
            pen.rect(1.4, (1.4, 2.0), (14.6, 14.0), 1.2);
            pen.block((3.6, 4.2), (8.0, 8.6), 0.6);
            pen.line(1.4, &[(10.0, 4.8), (12.8, 4.8)]);
            pen.line(1.4, &[(10.0, 7.8), (12.8, 7.8)]);
            pen.line(1.4, &[(3.6, 11.4), (12.8, 11.4)]);
        }
        Icon::MemoryStick => {
            pen.rect(1.4, (1.0, 3.8), (15.0, 10.6), 0.8);
            for x in [2.9, 6.6, 10.3] {
                pen.block((x, 5.5), (x + 2.8, 8.9), 0.3);
            }
            for x in [3.0, 5.5, 8.0, 10.5, 13.0] {
                pen.line(1.2, &[(x, 10.6), (x, 13.2)]);
            }
        }
        Icon::GraphicsCard => {
            pen.rect(1.4, (1.0, 3.0), (15.0, 11.4), 1.0);
            pen.circle(1.3, (5.6, 7.2), 2.6);
            pen.disc((5.6, 7.2), 0.8);
            pen.line(1.4, &[(10.0, 5.6), (13.0, 5.6)]);
            pen.line(1.4, &[(10.0, 8.8), (13.0, 8.8)]);
            pen.block((4.4, 11.4), (11.6, 13.4), 0.3);
        }
        Icon::Drive => {
            pen.rect(1.4, (1.4, 3.0), (14.6, 13.0), 1.6);
            pen.line(1.4, &[(1.4, 9.4), (14.6, 9.4)]);
            pen.disc((11.9, 11.2), 0.95);
        }
        Icon::ShieldCheck | Icon::Shield => {
            pen.outline(
                1.4,
                &[
                    (8.0, 1.2),
                    (14.0, 3.4),
                    (13.4, 9.0),
                    (8.0, 14.8),
                    (2.6, 9.0),
                    (2.0, 3.4),
                ],
            );
            if icon == Icon::ShieldCheck {
                pen.line(W, &[(5.0, 8.0), (7.2, 10.2), (11.2, 5.6)]);
            }
        }
        Icon::Globe => {
            pen.circle(1.4, (8.0, 8.0), 6.6);
            let lens: Vec<(f32, f32)> = (0..=24)
                .map(|i| {
                    let a = TAU * i as f32 / 24.0;
                    (8.0 + 2.8 * a.cos(), 8.0 + 6.6 * a.sin())
                })
                .collect();
            pen.line(1.2, &lens);
            pen.line(1.2, &[(1.4, 8.0), (14.6, 8.0)]);
        }
        Icon::Sensor => {
            pen.circle(1.4, (8.0, 8.0), 6.0);
            pen.disc((8.0, 8.0), 2.2);
        }
        Icon::ExpandAll => {
            pen.line(1.7, &[(3.5, 3.5), (8.0, 8.0), (12.5, 3.5)]);
            pen.line(1.7, &[(3.5, 8.5), (8.0, 13.0), (12.5, 8.5)]);
        }
        Icon::CollapseAll => {
            pen.line(1.7, &[(3.5, 8.0), (8.0, 3.5), (12.5, 8.0)]);
            pen.line(1.7, &[(3.5, 13.0), (8.0, 8.5), (12.5, 13.0)]);
        }
        Icon::Reset => {
            // An arc with a gap at the top left and an arrow pointing back along it.
            pen.arc(1.6, (8.0, 8.6), 5.6, -80.0, 200.0);
            let end = (
                8.0 + 5.6 * (-80.0f32).to_radians().cos(),
                8.6 + 5.6 * (-80.0f32).to_radians().sin(),
            );
            pen.arrowhead(end, 190.0, 3.0, 2.6);
        }
        Icon::Document => {
            pen.rect(1.4, (3.0, 1.2), (13.0, 14.8), 1.2);
            pen.line(1.4, &[(5.6, 5.0), (10.4, 5.0)]);
            pen.line(1.4, &[(5.6, 8.0), (10.4, 8.0)]);
            pen.line(1.4, &[(5.6, 11.0), (8.6, 11.0)]);
        }
        Icon::Stopwatch => {
            pen.circle(W, (8.0, 9.2), 5.6);
            pen.line(1.8, &[(6.2, 1.4), (9.8, 1.4)]);
            pen.line(W, &[(8.0, 1.4), (8.0, 3.6)]);
            pen.line(W, &[(8.0, 9.2), (10.4, 6.8)]);
        }
        Icon::Gauge => {
            pen.arc(W, (8.0, 10.0), 6.4, 180.0, 360.0);
            pen.line(1.6, &[(8.0, 10.0), (11.6, 5.8)]);
            pen.disc((8.0, 10.0), 1.4);
            pen.line(W, &[(1.6, 10.0), (1.6, 12.4)]);
            pen.line(W, &[(14.4, 10.0), (14.4, 12.4)]);
        }
        Icon::Gear => {
            pen.circle(2.0, (8.0, 8.0), 4.3);
            for k in 0..8 {
                let a = k as f32 * TAU / 8.0;
                let (c, s) = (a.cos(), a.sin());
                pen.line(
                    2.2,
                    &[
                        (8.0 + 5.0 * c, 8.0 + 5.0 * s),
                        (8.0 + 7.0 * c, 8.0 + 7.0 * s),
                    ],
                );
            }
        }
        Icon::Search => {
            pen.circle(1.7, (6.6, 6.6), 4.6);
            pen.line(2.3, &[(10.1, 10.1), (14.4, 14.4)]);
        }
        Icon::Power => {
            pen.arc(1.7, (8.0, 8.8), 5.6, -55.0, 235.0);
            pen.line(1.7, &[(8.0, 1.2), (8.0, 8.0)]);
        }
        Icon::Close => {
            pen.line(1.7, &[(4.0, 4.0), (12.0, 12.0)]);
            pen.line(1.7, &[(12.0, 4.0), (4.0, 12.0)]);
        }
        Icon::Graph => {
            pen.line(1.4, &[(2.0, 1.8), (2.0, 14.2), (14.4, 14.2)]);
            pen.line(1.7, &[(4.4, 11.2), (7.0, 6.8), (9.6, 9.4), (13.4, 3.6)]);
        }
    }
}

/// Takes room for an icon in the current layout and paints it there. Decorative, so it
/// adds nothing to the accessibility tree.
pub fn icon(ui: &mut Ui, icon: Icon, color: Color32) {
    let height = ui.available_height().clamp(ICON_SIZE, ICON_SIZE + 8.0);
    let (rect, _) = ui.allocate_exact_size(vec2(ICON_SIZE + 2.0, height), Sense::hover());
    if ui.is_rect_visible(rect) {
        paint(ui.painter(), rect, icon, color);
    }
}

/// A button with an icon and, optionally, text. `name` is what screen readers and tests
/// call it; give icon-only buttons a tooltip with the same words.
pub fn button(ui: &mut Ui, icon: Icon, text: Option<&str>, name: &str) -> Response {
    let padding = ui.spacing().button_padding;
    let gap = 5.0;
    let galley = text.map(|text| {
        WidgetText::from(text).into_galley(
            ui,
            Some(TextWrapMode::Extend),
            f32::INFINITY,
            TextStyle::Button,
        )
    });
    let pad_x = if galley.is_some() {
        padding.x
    } else {
        padding.y + 2.0
    };
    let text_size = galley.as_ref().map_or(Vec2::ZERO, |g| g.size());
    let content = ICON_SIZE + galley.as_ref().map_or(0.0, |g| gap + g.size().x);
    let size = vec2(
        content + 2.0 * pad_x,
        (ICON_SIZE.max(text_size.y) + 2.0 * padding.y).max(ui.spacing().interact_size.y),
    );
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());
    let enabled = ui.is_enabled();
    response.widget_info(|| WidgetInfo::labeled(Role::Button, enabled, name));
    if ui.is_rect_visible(rect) {
        let visuals = ui.style().interact(&response);
        ui.painter().rect(
            rect.expand(visuals.expansion),
            visuals.corner_radius,
            visuals.weak_bg_fill,
            visuals.bg_stroke,
            StrokeKind::Inside,
        );
        let color = visuals.text_color();
        let icon_rect = Rect::from_center_size(
            pos2(rect.left() + pad_x + ICON_SIZE / 2.0, rect.center().y),
            Vec2::splat(ICON_SIZE),
        );
        paint(ui.painter(), icon_rect, icon, color);
        if let Some(galley) = galley {
            let at = pos2(
                icon_rect.right() + gap,
                rect.center().y - galley.size().y / 2.0,
            );
            ui.painter().galley(at, galley, color);
        }
    }
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_kind_and_category_has_an_icon() {
        assert_eq!(
            Icon::for_channel(Kind::Temperature, Unit::Celsius),
            Icon::Thermometer
        );
        assert_eq!(
            Icon::for_channel(Kind::Other, Unit::Multiplier),
            Icon::Times
        );
        assert_eq!(Icon::for_category(Category::Cpu), Icon::Chip);
        assert_eq!(Icon::for_category(Category::Smart), Icon::ShieldCheck);
    }

    #[test]
    fn icons_paint_inside_their_square() {
        let rect = Rect::from_min_size(pos2(100.0, 100.0), Vec2::splat(ICON_SIZE));
        for icon in ALL {
            let shapes = shapes(rect, icon, Color32::WHITE);
            assert!(!shapes.is_empty(), "{icon:?} paints nothing");
            let bounds = shapes
                .iter()
                .map(Shape::visual_bounding_rect)
                .fold(Rect::NOTHING, |a, b| a.union(b));
            assert!(
                rect.expand(1.0).contains_rect(bounds),
                "{icon:?} spills out of its square: {bounds:?}"
            );
        }
    }

    const ALL: [Icon; 35] = [
        Icon::Thermometer,
        Icon::Bolt,
        Icon::Wave,
        Icon::Plug,
        Icon::Battery,
        Icon::Fan,
        Icon::SquareWave,
        Icon::Clock,
        Icon::Pie,
        Icon::UpDown,
        Icon::Stack,
        Icon::Hash,
        Icon::Toggle,
        Icon::Times,
        Icon::Diamond,
        Icon::Chip,
        Icon::Board,
        Icon::MemoryStick,
        Icon::GraphicsCard,
        Icon::Drive,
        Icon::ShieldCheck,
        Icon::Globe,
        Icon::Sensor,
        Icon::ExpandAll,
        Icon::CollapseAll,
        Icon::Reset,
        Icon::Document,
        Icon::Stopwatch,
        Icon::Gauge,
        Icon::Gear,
        Icon::Search,
        Icon::Power,
        Icon::Close,
        Icon::Shield,
        Icon::Graph,
    ];
}
