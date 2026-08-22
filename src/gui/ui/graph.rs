// ============================================================================
// Module:       gui::ui::graph
// Description:  The area and multi-series graphs the Performance view is built
//               from, painted directly rather than through a plotting crate.
//
// Dependencies: egui; crate::model::history::Series, super::theme
// ============================================================================

//! The graphs.
//!
//! Painted from [`crate::model::history::Series`] rings straight onto the
//! painter. No plotting crate: these are four chart types with no axes to
//! configure, no legends to lay out, and no interaction beyond a hover
//! readout — and every one of them has to take its colours from the theme
//! and its gridlines from the spacing scale, which is most of what a
//! plotting crate would otherwise decide for us.
//!
//! ## The gradient is not decoration
//!
//! [`area`] fills under its line with a vertical gradient from the series
//! colour down to transparent. That reads as a filled area at a glance
//! while leaving the gridlines behind it visible, which a flat fill does
//! not — and being able to read a value off a busy chart is the whole
//! point of having gridlines.
//!
//! ## Time runs left to right and the newest sample is at the right edge
//!
//! Every graph here draws its oldest sample at the left and its newest at
//! the right, and a partly-filled ring is drawn **right-aligned** — so a
//! graph that has been running for ten seconds shows ten seconds of data
//! against the right edge with empty space to the left, rather than ten
//! seconds stretched across the whole panel.
//!
//! That matters because the stretch is invisible. A graph that
//! rescales its own time base as it fills looks like a graph of a machine
//! whose behaviour is changing, and there is no way to tell the
//! difference from the picture.

use super::theme::{self, RADIUS, SPACE_SM, SPACE_XS};
use crate::color::Rgb;
use crate::model::history::Series;
use crate::theme::Palette;
use egui::{
    Align2, Color32, CornerRadius, Mesh, Pos2, Rect, Sense, Shape, Stroke, TextStyle, Ui, Vec2,
};

/// How many horizontal gridlines a graph draws.
///
/// Four bands, so the lines fall at 25%, 50% and 75% of the axis — the
/// three positions a reader can locate without counting.
const GRID_BANDS: usize = 4;

/// The alpha the area fill starts at, directly under the line.
const FILL_TOP_ALPHA: u8 = 110;

/// A graph's own configuration.
pub struct Graph<'a> {
    /// The samples, oldest first.
    pub series: &'a Series,
    /// The line's colour.
    pub color: Rgb,
    /// The smallest the y axis may be. 100 for a percentage graph, which
    /// then never rescales; 0 for a rate graph, which has no natural
    /// ceiling.
    pub floor: f32,
    /// How the y-axis maximum is labelled.
    pub unit: Unit,
}

/// How a graph labels its axis.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Unit {
    /// A percentage. The axis is fixed at 100 and labelled "100%".
    Percent,
    /// A byte rate. The axis is quantised and labelled "12.0 MB/s".
    Rate,
    /// A byte count.
    Bytes,
}

impl Unit {
    /// Renders an axis maximum.
    #[must_use]
    pub fn label(self, value: f32) -> String {
        match self {
            Self::Percent => "100%".to_string(),
            Self::Rate => crate::format::rate(f64::from(value)),
            Self::Bytes => crate::format::bytes(value.max(0.0) as u64),
        }
    }
}

/// Draws a filled area graph into `rect`.
pub fn area(ui: &Ui, theme: &Palette, rect: Rect, graph: &Graph<'_>) {
    let painter = ui.painter();
    painter.rect_filled(rect, CornerRadius::same(RADIUS), theme::rgb(theme.app));
    grid(ui, theme, rect);

    let scale = graph.series.scale(graph.floor);
    let points = plot_points(graph.series, rect, scale);
    if points.len() < 2 {
        // One point is not a line. Drawing nothing is right: the graph's
        // frame and gridlines are already on screen, so it reads as
        // "waiting for data" rather than as broken.
        axis_label(ui, theme, rect, graph.unit.label(scale));
        return;
    }

    fill_under(painter, rect, &points, graph.color);
    painter.add(Shape::line(
        points,
        Stroke::new(1.5, theme::rgb(graph.color)),
    ));
    frame(ui, theme, rect);
    axis_label(ui, theme, rect, graph.unit.label(scale));
}

/// Draws a two-band area graph: a total with a second series shaded
/// beneath it.
///
/// The CPU graph's kernel band. Two calls to [`area`] would paint the
/// second's fill over the first's line; this draws both fills, then both
/// lines, so the boundary between them stays visible.
pub fn banded(ui: &Ui, theme: &Palette, rect: Rect, total: &Graph<'_>, under: &Graph<'_>) {
    let painter = ui.painter();
    painter.rect_filled(rect, CornerRadius::same(RADIUS), theme::rgb(theme.app));
    grid(ui, theme, rect);

    // Both bands share the total's scale, or the kernel band would be
    // drawn against its own maximum and appear larger than the total it
    // is part of — which is not merely wrong, it is impossible, and the
    // reader has no way to know which of the two to believe.
    let scale = total.series.scale(total.floor);
    let total_points = plot_points(total.series, rect, scale);
    let under_points = plot_points(under.series, rect, scale);

    if total_points.len() >= 2 {
        fill_under(painter, rect, &total_points, total.color);
    }
    if under_points.len() >= 2 {
        fill_under(painter, rect, &under_points, under.color);
    }
    if total_points.len() >= 2 {
        painter.add(Shape::line(
            total_points,
            Stroke::new(1.5, theme::rgb(total.color)),
        ));
    }
    if under_points.len() >= 2 {
        painter.add(Shape::line(
            under_points,
            Stroke::new(1.0, theme::rgb(under.color)),
        ));
    }

    frame(ui, theme, rect);
    axis_label(ui, theme, rect, total.unit.label(scale));
}

/// Draws a grid of small per-core graphs.
///
/// Laid out in as square a grid as the count allows, because a 64-core
/// machine in one row gives each core four pixels of width. Each core
/// gets its own colour from the theme's rainbow ramp, so a core in the
/// grid is the same colour as its line in the combined graph above.
pub fn core_grid(ui: &mut Ui, theme: &Palette, rect: Rect, cores: &[Series]) {
    if cores.is_empty() {
        return;
    }
    let count = cores.len();
    // As square as the count allows, rounded so wide-and-short beats
    // tall-and-narrow — a monitor is wider than it is tall.
    let columns = (count as f32).sqrt().ceil().max(1.0) as usize;
    let rows = count.div_ceil(columns);

    let cell = Vec2::new(
        (rect.width() - SPACE_XS * (columns.saturating_sub(1)) as f32) / columns as f32,
        (rect.height() - SPACE_XS * (rows.saturating_sub(1)) as f32) / rows as f32,
    );
    if cell.x <= 1.0 || cell.y <= 1.0 {
        return;
    }

    for (index, series) in cores.iter().enumerate() {
        let column = index % columns;
        let row = index / columns;
        let origin = rect.min
            + Vec2::new(
                (cell.x + SPACE_XS) * column as f32,
                (cell.y + SPACE_XS) * row as f32,
            );
        let cell_rect = Rect::from_min_size(origin, cell);
        area(
            ui,
            theme,
            cell_rect,
            &Graph {
                series,
                color: theme.series(index, count),
                floor: 100.0,
                unit: Unit::Percent,
            },
        );
    }
    ui.allocate_rect(rect, Sense::hover());
}

/// Turns a series into screen points, right-aligned in `rect`.
///
/// The right-alignment is the part that matters; see the module docs.
fn plot_points(series: &Series, rect: Rect, scale: f32) -> Vec<Pos2> {
    let capacity = series.capacity().max(1);
    let count = series.len();
    if count == 0 || scale <= 0.0 {
        return Vec::new();
    }
    // One sample's width, from the ring's *capacity* rather than its
    // current length — which is what right-aligns a partly-filled ring
    // instead of stretching it across the panel.
    let step = rect.width() / capacity.saturating_sub(1).max(1) as f32;
    let first = rect.right() - step * count.saturating_sub(1) as f32;

    series
        .iter()
        .enumerate()
        .map(|(index, value)| {
            let x = first + step * index as f32;
            let fraction = (value / scale).clamp(0.0, 1.0);
            Pos2::new(x, rect.bottom() - rect.height() * fraction)
        })
        .collect()
}

/// Fills the area under a line with a vertical gradient.
///
/// Built as a mesh rather than a filled polygon so the gradient is real:
/// a polygon takes one colour, and a stack of horizontal strips to fake a
/// gradient would be dozens of shapes per graph per frame.
fn fill_under(painter: &egui::Painter, rect: Rect, points: &[Pos2], color: Rgb) {
    if points.len() < 2 {
        return;
    }
    let top = theme::translucent(color, FILL_TOP_ALPHA);
    let bottom = theme::translucent(color, 0);

    let mut mesh = Mesh::default();
    // Two vertices per sample — one on the line, one on the baseline —
    // then a quad between each adjacent pair.
    for point in points {
        mesh.colored_vertex(*point, top);
        mesh.colored_vertex(Pos2::new(point.x, rect.bottom()), bottom);
    }
    for index in 0..points.len().saturating_sub(1) {
        let base = u32::try_from(index * 2).unwrap_or(0);
        mesh.add_triangle(base, base + 1, base + 2);
        mesh.add_triangle(base + 1, base + 3, base + 2);
    }
    painter.add(Shape::mesh(mesh));
}

/// Draws a graph's gridlines.
fn grid(ui: &Ui, theme: &Palette, rect: Rect) {
    let painter = ui.painter();
    let stroke = Stroke::new(1.0, theme::rgb(theme.grid));
    for band in 1..GRID_BANDS {
        let y = rect.top() + rect.height() * band as f32 / GRID_BANDS as f32;
        painter.hline(rect.x_range(), y, stroke);
    }
    // Vertical lines at the same spacing, so the grid reads as a grid
    // rather than as a set of rules. Same count as the horizontal bands,
    // which keeps the cells roughly square on a wide panel.
    for band in 1..GRID_BANDS {
        let x = rect.left() + rect.width() * band as f32 / GRID_BANDS as f32;
        painter.vline(x, rect.y_range(), stroke);
    }
}

/// Draws a graph's outline.
fn frame(ui: &Ui, theme: &Palette, rect: Rect) {
    ui.painter().rect_stroke(
        rect,
        CornerRadius::same(RADIUS),
        Stroke::new(1.0, theme::rgb(theme.border)),
        egui::StrokeKind::Inside,
    );
}

/// Labels a graph's axis maximum, in its top-left corner.
fn axis_label(ui: &Ui, theme: &Palette, rect: Rect, text: String) {
    ui.painter().text(
        rect.left_top() + Vec2::new(SPACE_XS, SPACE_XS),
        Align2::LEFT_TOP,
        text,
        TextStyle::Small.resolve(ui.style()),
        theme::rgb(theme.text_faint),
    );
}

/// Draws a legend entry: a colour swatch and a label.
pub fn legend(ui: &mut Ui, theme: &Palette, color: Rgb, label: &str, value: &str) {
    /// The swatch's size.
    const SWATCH: f32 = 10.0;

    ui.horizontal(|ui| {
        let (rect, _) = ui.allocate_exact_size(Vec2::splat(SWATCH), Sense::hover());
        ui.painter()
            .rect_filled(rect, CornerRadius::same(2), theme::rgb(color));
        ui.add_space(SPACE_XS);
        ui.label(
            egui::RichText::new(label)
                .color(theme::rgb(theme.text_muted))
                .text_style(TextStyle::Small),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(
                egui::RichText::new(value)
                    .color(theme::rgb(theme.text))
                    .text_style(TextStyle::Monospace),
            );
        });
    });
    ui.add_space(SPACE_XS);
}

/// A sparkline: a bare line with no frame, grid, or label.
///
/// For a table cell or a compact summary, where the shape of the last
/// minute is the whole message and any chrome around it would dominate a
/// graph twenty pixels tall.
pub fn sparkline(ui: &Ui, rect: Rect, series: &Series, color: Rgb, floor: f32) {
    let scale = series.scale(floor);
    let points = plot_points(series, rect, scale);
    if points.len() < 2 {
        return;
    }
    fill_under(ui.painter(), rect, &points, color);
    ui.painter()
        .add(Shape::line(points, Stroke::new(1.0, theme::rgb(color))));
}

/// A colour at the opacity a graph's fill uses, for a legend swatch that
/// has to match the chart.
#[must_use]
pub fn fill_color(color: Rgb) -> Color32 {
    theme::translucent(color, FILL_TOP_ALPHA)
}

/// The gap a graph leaves below itself.
pub const GRAPH_GAP: f32 = SPACE_SM;

#[cfg(test)]
mod tests {
    use super::*;

    fn filled(capacity: usize, count: usize, value: f32) -> Series {
        let mut series = Series::new(capacity);
        for _ in 0..count {
            series.push(value);
        }
        series
    }

    fn panel() -> Rect {
        Rect::from_min_size(Pos2::new(0.0, 0.0), Vec2::new(200.0, 100.0))
    }

    #[test]
    fn a_full_series_spans_the_whole_panel() {
        let series = filled(100, 100, 50.0);
        let points = plot_points(&series, panel(), 100.0);
        assert_eq!(points.len(), 100);
        let (Some(first), Some(last)) = (points.first(), points.last()) else {
            return;
        };
        assert!(
            (first.x - 0.0).abs() < 0.01,
            "the oldest sample sits at the left edge, got {}",
            first.x
        );
        assert!(
            (last.x - 200.0).abs() < 0.01,
            "the newest sits at the right edge, got {}",
            last.x
        );
    }

    #[test]
    fn a_partly_filled_series_is_right_aligned_rather_than_stretched() {
        // The bug this exists to avoid: a graph that rescales its own
        // time base as it fills looks like a graph of a machine whose
        // behaviour is changing, and there is no way to tell from the
        // picture.
        let series = filled(100, 10, 50.0);
        let points = plot_points(&series, panel(), 100.0);
        assert_eq!(points.len(), 10);
        let (Some(first), Some(last)) = (points.first(), points.last()) else {
            return;
        };
        assert!(
            (last.x - 200.0).abs() < 0.01,
            "the newest sample must still be at the right edge, got {}",
            last.x
        );
        assert!(
            first.x > 150.0,
            "ten of a hundred samples should occupy a tenth of the width \
             at the right, but the oldest is at {}",
            first.x
        );
    }

    #[test]
    fn a_value_at_the_axis_maximum_reaches_the_top() {
        let series = filled(10, 10, 100.0);
        let points = plot_points(&series, panel(), 100.0);
        assert!(
            points.iter().all(|point| (point.y - 0.0).abs() < 0.01),
            "a saturated series should touch the top of the panel"
        );
    }

    #[test]
    fn a_zero_series_sits_on_the_baseline() {
        let series = filled(10, 10, 0.0);
        let points = plot_points(&series, panel(), 100.0);
        assert!(
            points.iter().all(|point| (point.y - 100.0).abs() < 0.01),
            "an idle series should sit on the bottom of the panel"
        );
    }

    #[test]
    fn a_value_past_the_axis_is_clamped_rather_than_drawn_outside() {
        // A sample above the scale would otherwise be painted above the
        // panel, over whatever is there.
        let series = filled(10, 10, 500.0);
        let points = plot_points(&series, panel(), 100.0);
        assert!(
            points.iter().all(|point| point.y >= -0.01),
            "no point may be drawn above the panel"
        );
    }

    #[test]
    fn an_empty_series_produces_no_points() {
        let series = Series::new(100);
        assert!(plot_points(&series, panel(), 100.0).is_empty());
    }

    #[test]
    fn a_degenerate_scale_produces_no_points_rather_than_infinities() {
        // A zero scale would divide every value by zero, and the
        // resulting infinity would be painted as a vertical line to
        // nowhere.
        let series = filled(10, 10, 50.0);
        assert!(plot_points(&series, panel(), 0.0).is_empty());
        assert!(plot_points(&series, panel(), -1.0).is_empty());
    }

    #[test]
    fn every_point_lands_inside_the_panel() {
        let mut series = Series::new(50);
        for value in [0.0, 25.0, 50.0, 75.0, 100.0, f32::NAN, -10.0, 1e9] {
            series.push(value);
        }
        let rect = panel();
        for point in plot_points(&series, rect, 100.0) {
            assert!(
                point.x.is_finite() && point.y.is_finite(),
                "a non-finite coordinate would be painted as a line to \
                 nowhere: {point:?}"
            );
            assert!(
                point.y >= rect.top() - 0.01 && point.y <= rect.bottom() + 0.01,
                "{point:?} is outside {rect:?}"
            );
        }
    }

    #[test]
    fn a_single_sample_is_not_drawn_as_a_line() {
        let series = filled(100, 1, 50.0);
        let points = plot_points(&series, panel(), 100.0);
        assert_eq!(points.len(), 1, "one point is not a line");
    }

    #[test]
    fn a_percentage_axis_is_always_labelled_the_same() {
        // Two CPU graphs side by side are only comparable at a glance if
        // they share an axis.
        assert_eq!(Unit::Percent.label(100.0), "100%");
        assert_eq!(Unit::Percent.label(50.0), "100%");
    }

    #[test]
    fn a_rate_axis_is_labelled_with_its_own_maximum() {
        let label = Unit::Rate.label(1_048_576.0);
        assert!(label.contains("MB/s"), "got {label}");
    }

    #[test]
    fn the_core_grid_stays_roughly_square() {
        // A 64-core machine in one row gives each core four pixels.
        for count in [1usize, 2, 4, 8, 12, 16, 32, 64, 128] {
            let columns = (count as f32).sqrt().ceil().max(1.0) as usize;
            let rows = count.div_ceil(columns);
            assert!(columns * rows >= count, "{count} cores do not fit");
            assert!(
                columns.abs_diff(rows) <= 2,
                "{count} cores laid out {columns}x{rows}, which is not square"
            );
        }
    }
}
