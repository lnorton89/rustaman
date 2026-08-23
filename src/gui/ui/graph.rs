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

/// A painter that cannot draw outside `rect`.
///
/// Every graph here paints its data through one of these. Clamping the
/// *values* is not enough, and that is the part that is easy to get
/// wrong: `plot_points` already clamps every sample to the rect, and
/// the line still escaped the frame.
///
/// It escapes at the joins. A stroke is tessellated by offsetting each
/// vertex along the average of its two adjacent segment normals, scaled
/// by the reciprocal of the half-angle's cosine — and a spike that goes
/// up and immediately back down is very nearly a 180 degree reversal, so
/// that reciprocal is very nearly a division by zero. The join is
/// projected tens of points past a vertex that is sitting correctly on
/// the axis maximum. A rate graph is mostly flat with occasional spikes,
/// which is precisely the shape that produces them.
///
/// Note `Painter::with_clip_rect` *intersects* with the clip already in
/// force rather than replacing it, which is what is wanted here: the
/// graph must not escape its own rect, and it must not escape whatever
/// the panel had already restricted it to either.
fn clipped(ui: &Ui, rect: Rect) -> egui::Painter {
    ui.painter().with_clip_rect(rect)
}

/// Draws a filled area graph into `rect`.
pub fn area(ui: &Ui, theme: &Palette, rect: Rect, graph: &Graph<'_>) {
    area_with(ui, theme, rect, graph, Axis::Labelled);
}

/// Whether an area graph states the value at the top of its own axis.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Axis {
    /// The usual case: a graph standing on its own needs its scale
    /// stated, because the shape means nothing without it.
    Labelled,
    /// For a graph in a grid of identical ones. Sixteen core tiles share
    /// one fixed axis, so sixteen copies of "100%" state a fact once and
    /// then repeat it fifteen times in the corner of every tile — the
    /// grid's own heading already says what these are.
    Shared,
}

/// [`area`], with the axis label optional.
fn area_with(ui: &Ui, theme: &Palette, rect: Rect, graph: &Graph<'_>, axis: Axis) {
    let painter = ui.painter();
    painter.rect_filled(rect, CornerRadius::same(RADIUS), theme::rgb(theme.app));
    grid(ui, theme, rect);

    let scale = graph.series.scale(graph.floor);
    let points = plot_points(graph.series, rect, scale);
    if points.len() < 2 {
        // One point is not a line. Drawing nothing is right: the graph's
        // frame and gridlines are already on screen, so it reads as
        // "waiting for data" rather than as broken.
        if axis == Axis::Labelled {
            axis_label(ui, theme, rect, graph.unit.label(scale));
        }
        return;
    }

    let plot = clipped(ui, rect);
    fill_under(&plot, rect, &points, graph.color);
    plot.add(Shape::line(
        points,
        Stroke::new(1.5, theme::rgb(graph.color)),
    ));
    frame(ui, theme, rect);
    if axis == Axis::Labelled {
        axis_label(ui, theme, rect, graph.unit.label(scale));
    }
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

    let plot = clipped(ui, rect);
    if total_points.len() >= 2 {
        fill_under(&plot, rect, &total_points, total.color);
    }
    if under_points.len() >= 2 {
        fill_under(&plot, rect, &under_points, under.color);
    }
    if total_points.len() >= 2 {
        plot.add(Shape::line(
            total_points,
            Stroke::new(1.5, theme::rgb(total.color)),
        ));
    }
    if under_points.len() >= 2 {
        plot.add(Shape::line(
            under_points,
            Stroke::new(1.0, theme::rgb(under.color)),
        ));
    }

    frame(ui, theme, rect);
    axis_label(ui, theme, rect, total.unit.label(scale));
}

/// Draws several independent series into one rect, on one shared axis.
///
/// For a quantity that is really two — disk read against disk write,
/// bytes sent against bytes received. The single combined line these
/// replace answered "is the disk busy" and hid the only thing worth
/// knowing about a busy disk, which is *which way* the traffic is
/// going: a machine paging is reading, a machine writing a backup is
/// writing, and summed they are the same graph.
///
/// Two graphs side by side do not fix it either. Each would be scaled
/// to its own maximum, so 2 MB/s of write and 200 MB/s of read draw the
/// identical shape at the identical height — the reader compares them
/// because they are adjacent, and every such comparison is wrong. One
/// axis is the entire point of drawing them together.
///
/// Distinct from [`banded`], which draws a *part* underneath its
/// *whole* and so takes the whole's scale. These series are independent
/// of each other, so the scale is the largest any one of them needs.
pub fn layered(ui: &Ui, theme: &Palette, rect: Rect, series: &[Graph<'_>]) {
    let painter = ui.painter();
    painter.rect_filled(rect, CornerRadius::same(RADIUS), theme::rgb(theme.app));
    grid(ui, theme, rect);

    let Some(first) = series.first() else {
        // No series at all still gets its frame, so the panel keeps its
        // shape rather than showing a hole where a graph should be.
        frame(ui, theme, rect);
        return;
    };

    // One scale, taken from whichever series needs the most room.
    let scale = series
        .iter()
        .map(|graph| graph.series.scale(graph.floor))
        .fold(first.floor, f32::max);

    let plotted: Vec<(&Graph<'_>, Vec<Pos2>)> = series
        .iter()
        .map(|graph| (graph, plot_points(graph.series, rect, scale)))
        .collect();

    // Every fill, then every line. Interleaving them buries the first
    // series' line under the second series' fill exactly where the two
    // cross — which is the moment the graph exists to show.
    let plot = clipped(ui, rect);
    for (graph, points) in &plotted {
        if points.len() >= 2 {
            fill_under(&plot, rect, points, graph.color);
        }
    }
    for (graph, points) in &plotted {
        if points.len() >= 2 {
            plot.add(Shape::line(
                points.clone(),
                Stroke::new(1.5, theme::rgb(graph.color)),
            ));
        }
    }

    frame(ui, theme, rect);
    axis_label(ui, theme, rect, first.unit.label(scale));
}

/// Draws a grid of small per-core graphs.
///
/// Laid out in as square a grid as the count allows, because a 64-core
/// machine in one row gives each core four pixels of width. Each core
/// gets its own colour from the theme's rainbow ramp, so a core in the
/// grid is the same colour as its line in the combined graph above.
///
/// `kinds` says which tiles are performance cores and which are
/// efficiency cores, and may be shorter than `cores` or empty — an
/// unreadable or uniform topology draws the grid exactly as it drew
/// before there was such a thing as a hybrid CPU. Where the machine
/// *is* hybrid, the marker is what turns "why are those eight always
/// busy and those sixteen never" into a fact about the scheduler rather
/// than a puzzle.
pub fn core_grid(
    ui: &mut Ui,
    theme: &Palette,
    rect: Rect,
    cores: &[Series],
    kinds: &[crate::model::CoreKind],
) {
    if cores.is_empty() {
        return;
    }
    let count = cores.len();
    let (columns, rows) = core_grid_layout(count);

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
        area_with(
            ui,
            theme,
            cell_rect,
            &Graph {
                series,
                color: theme.series(index, count),
                floor: 100.0,
                unit: Unit::Percent,
            },
            // One axis, shared by every tile — see `Axis::Shared`.
            Axis::Shared,
        );
        // Each tile identifies itself and carries the current value. The
        // graph still owns most of the pixels; these two short labels turn
        // sixteen otherwise-anonymous shapes into actual logical processors.
        let kind = kinds.get(index).copied().unwrap_or_default();
        if cell_rect.width() >= 46.0 && cell_rect.height() >= 24.0 {
            let font = TextStyle::Small.resolve(ui.style());
            // The marker goes in the tile's own label rather than in a
            // second line or a tinted background: a hybrid machine has
            // two dozen of these on screen at once, and anything that
            // costs a tile more than one character costs the graph the
            // room it was drawn for.
            let label = match kind.marker() {
                Some(marker) => format!("CPU {index} · {marker}"),
                None => format!("CPU {index}"),
            };
            ui.painter().text(
                cell_rect.left_top() + Vec2::new(SPACE_XS, SPACE_XS),
                Align2::LEFT_TOP,
                label,
                font.clone(),
                theme::rgb(theme.text_faint),
            );
            ui.painter().text(
                cell_rect.right_top() + Vec2::new(-SPACE_XS, SPACE_XS),
                Align2::RIGHT_TOP,
                crate::format::percent(f64::from(series.latest())),
                font,
                theme::rgb(theme.text),
            );
        }
        // Hover carries the window statistics that would be too dense to
        // print on every tile at once.
        ui.interact(cell_rect, ui.id().with("core").with(index), Sense::hover())
            .on_hover_text(format!(
                "{} {index}\nCurrent {}\nRecent average {}\nRecent peak {}\n{} samples",
                kind.label(),
                crate::format::percent(f64::from(series.latest())),
                crate::format::percent(f64::from(series.mean())),
                crate::format::percent(f64::from(series.max())),
                series.len(),
            ));
    }
}

/// The column and row count for `count` cores, as square as it allows.
///
/// Rounded so wide-and-short beats tall-and-narrow — a monitor is wider
/// than it is tall. `pub(crate)`, not private: [`super::performance`]
/// has to reserve a rect tall enough for this grid *before* it is drawn,
/// and calling this rather than keeping its own copy of the formula is
/// what stops the reservation from silently drifting out of step with
/// what the grid actually lays out.
pub(crate) fn core_grid_layout(count: usize) -> (usize, usize) {
    let columns = (count as f32).sqrt().ceil().max(1.0) as usize;
    let rows = count.div_ceil(columns);
    (columns, rows)
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
        ui.add_space(SPACE_XS);
        // A plain label, not `Layout::right_to_left` — that layout
        // justifies against the *whole remaining row*, not against this
        // legend entry's own few pixels. Harmless when `legend` is the
        // only thing in its row; call it twice in one `ui.horizontal`,
        // as the CPU panel does for Total and Kernel, and the first
        // entry's value claims every pixel left in the row to
        // right-align against, leaving the second entry nothing to draw
        // into and inflating the row's own measured width far past what
        // was actually available — which is what was cutting the panel
        // off against the window's real edge two entries down the tree.
        ui.label(
            egui::RichText::new(value)
                .color(theme::rgb(theme.text))
                .text_style(TextStyle::Monospace),
        );
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
    // A row's sparkline is 16 points tall and sits inches from the row
    // above it, so an unclipped join lands in a neighbour's text.
    let plot = clipped(ui, rect);
    fill_under(&plot, rect, &points, color);
    plot.add(Shape::line(points, Stroke::new(1.0, theme::rgb(color))));
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

    /// Every rect actually painted on screen, in the order drawn.
    ///
    /// A primitive's mesh is *not* clipped geometrically — epaint hands
    /// the clip to the GPU as a scissor and leaves the vertices where
    /// they are. So what reaches the screen is the mesh's bounds
    /// intersected with its own clip rect, and a test that checks either
    /// one alone passes on the broken version.
    fn painted(ctx: &egui::Context, shapes: Vec<egui::epaint::ClippedShape>) -> Vec<Rect> {
        ctx.tessellate(shapes, 1.0)
            .into_iter()
            .filter_map(|primitive| {
                let egui::epaint::Primitive::Mesh(mesh) = primitive.primitive else {
                    return None;
                };
                let bounds = mesh.vertices.iter().fold(Rect::NOTHING, |bounds, vertex| {
                    bounds.union(Rect::from_min_size(vertex.pos, Vec2::ZERO))
                });
                let visible = bounds.intersect(primitive.clip_rect);
                visible.is_positive().then_some(visible)
            })
            .collect()
    }

    #[test]
    fn a_spike_cannot_paint_outside_the_graph() -> anyhow::Result<()> {
        // A rate graph is flat with occasional spikes, and that is the
        // one shape that escapes: see `clipped`.
        let mut series = Series::new(64);
        for index in 0..64 {
            series.push(if index % 8 == 0 { 100.0 } else { 0.0 });
        }

        let app = crate::gui::app::App::new(crate::config::Config::default());
        let theme = app.theme.clone();
        // Well inside the window, so an escape has somewhere to go and
        // the test can see it.
        let window = Rect::from_min_size(Pos2::ZERO, Vec2::new(400.0, 400.0));
        let plot = Rect::from_min_size(Pos2::new(100.0, 100.0), Vec2::new(200.0, 100.0));

        let ctx = egui::Context::default();
        theme::apply(&ctx, &theme);
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(window),
                ..Default::default()
            },
            |ui| {
                area(
                    ui,
                    &theme,
                    plot,
                    &Graph {
                        series: &series,
                        color: theme.series(0, 5),
                        // A percentage axis is pinned at exactly 100
                        // with no headroom — deliberately, so two CPU
                        // graphs are comparable — so a process at 100%
                        // puts the line *on* `rect.top()`. That is the
                        // only configuration where a join has anywhere
                        // to escape to, and so the only one worth
                        // testing.
                        floor: 100.0,
                        unit: Unit::Percent,
                    },
                );
            },
        );
        output.textures_delta.clear();

        // Feathering widens every edge by about a pixel either side.
        let allowed = plot.expand(2.0);
        for visible in painted(&ctx, output.shapes) {
            assert!(
                allowed.contains_rect(visible),
                "the graph painted {visible:?}, outside its own rect {plot:?} —                  a spike's join is projected past the vertex, so clamping the                  sample is not enough on its own"
            );
        }
        Ok(())
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
        // Calls the real `core_grid_layout` rather than reimplementing
        // its formula — a copy here would keep passing after a change to
        // the real one drifted the two apart, which is exactly the kind
        // of test that looks like coverage and is not.
        for count in [1usize, 2, 4, 8, 12, 16, 32, 64, 128] {
            let (columns, rows) = core_grid_layout(count);
            assert!(columns * rows >= count, "{count} cores do not fit");
            assert!(
                columns.abs_diff(rows) <= 2,
                "{count} cores laid out {columns}x{rows}, which is not square"
            );
        }
    }
}
