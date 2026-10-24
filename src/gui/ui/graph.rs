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
//! painter. The main resource panels share [`timeline`]: labelled value
//! and time axes, a wrapping legend, and pointer/keyboard inspection of
//! actual recorded samples. Compact core graphs and sparklines keep the
//! same scale and clipping math without repeating the full chart chrome.
//! All surfaces, strokes and typography follow the active theme.
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

use super::theme::{self, RADIUS, SPACE_LG, SPACE_MD, SPACE_SM, SPACE_XS};
use super::widgets;
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

/// The width of a plotted data line.
///
/// One value rather than a literal at each call site because
/// [`LINE_CAP`] is derived from it, and an inset computed from a width
/// the strokes no longer use is worse than no inset at all.
const LINE_WIDTH: f32 = 1.5;

/// Half a data line's stroke: how far it reaches either side of its path.
///
/// A stroke is centred on the path, so a sample mapped exactly onto
/// `rect.top()` has half its width above the rect — and every graph here
/// paints through a clip, which cuts it (see [`clipped`]). A saturated
/// series then draws along the frame as a flat line of half the
/// thickness, which is what the CPU panel showed at 100%.
///
/// So the top of the plot is pulled in by this much. It costs under a
/// point of a graph's height, which no reader can see, and it is the
/// difference between a peak that touches the frame and one that is
/// shaved off by it.
const LINE_CAP: f32 = LINE_WIDTH / 2.0;

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

    /// Formats an actual reading or tick, including percentages below the maximum.
    #[must_use]
    pub fn value(self, value: f32) -> String {
        if self == Self::Percent {
            crate::format::percent(f64::from(value))
        } else {
            self.label(value)
        }
    }

    /// Keeps rate ticks round in the same binary units as their labels.
    /// Quantising raw bytes to decimal steps would print a 5 MB axis as 4.77 MB.
    fn axis_ceiling(self, value: f32) -> f32 {
        if self == Self::Percent {
            return value;
        }
        // Every tick should be at least one byte apart, including an idle graph.
        let value = value.max(GRID_BANDS as f32);
        let unit = 1024.0_f32.powf((value.log(1024.0).floor()).clamp(0.0, 6.0));
        let scaled = value / unit;
        let magnitude = 10.0_f32.powf(scaled.log10().floor());
        let step = [1.0, 2.0, 5.0, 10.0]
            .into_iter()
            .find(|step| *step * magnitude >= scaled)
            .unwrap_or(10.0);
        (step * magnitude * unit).min(f32::MAX)
    }
}

/// One named line in an inspectable timeline.
pub struct TimelineLine<'a> {
    /// The quantity represented by this line.
    pub label: &'a str,
    /// Samples, unit, color, and scale floor.
    pub graph: Graph<'a>,
}

/// The relationship between two lines sharing an axis.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Composition {
    /// Independent quantities such as disk read and write.
    Independent,
    /// A whole followed by a part, such as total CPU and kernel CPU.
    PartOfTotal,
}

/// The shared main-chart contract for every resource panel.
pub struct Timeline<'a> {
    /// Metric and accessible name, independent of the device heading.
    pub label: &'a str,
    /// Named lines in their legend order. The second uses a dashed stroke.
    pub lines: &'a [TimelineLine<'a>],
    /// Nominal sampling interval. Ages are approximate; readings are actual samples.
    pub interval: std::time::Duration,
    /// Whether the second line is part of the first.
    pub composition: Composition,
}

/// Draws labelled axes, named lines, sample inspection, and a wrapping legend.
/// Returns the axes rectangle; the legend below it is measured by the caller.
pub fn timeline(ui: &mut Ui, theme: &Palette, height: f32, chart: &Timeline<'_>) -> Rect {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), height), Sense::hover());
    let Some(first) = chart.lines.first() else {
        return rect;
    };
    let scale = if chart.composition == Composition::PartOfTotal {
        axis_maximum(&first.graph)
    } else {
        chart
            .lines
            .iter()
            .map(|line| axis_maximum(&line.graph))
            .fold(0.0, f32::max)
    }
    .max(f32::EPSILON);
    let ticks: Vec<_> = (0..=GRID_BANDS)
        .map(|band| {
            let value = scale * band as f32 / GRID_BANDS as f32;
            let galley = ui.painter().layout_no_wrap(
                first.graph.unit.value(value),
                TextStyle::Small.resolve(ui.style()),
                theme::rgb(theme.text_muted),
            );
            (value, galley)
        })
        .collect();
    let axis_width = ticks
        .iter()
        .map(|(_, label)| label.size().x)
        .fold(0.0, f32::max)
        + SPACE_SM;
    let plot = Rect::from_min_max(
        rect.min + Vec2::new(axis_width.min(rect.width() / 3.0), SPACE_LG + SPACE_SM),
        rect.max - Vec2::new(SPACE_XS, SPACE_LG + SPACE_XS),
    );
    ui.painter().text(
        rect.min,
        Align2::LEFT_TOP,
        chart.label,
        TextStyle::Small.resolve(ui.style()),
        theme::rgb(theme.text_muted),
    );
    if rect.width() >= 400.0 {
        ui.painter().text(
            rect.right_top(),
            Align2::RIGHT_TOP,
            if first.graph.unit == Unit::Percent {
                "Fixed 0–100%"
            } else {
                "Auto scale"
            },
            TextStyle::Small.resolve(ui.style()),
            theme::rgb(theme.text_muted),
        );
    }
    ui.painter()
        .rect_filled(plot, CornerRadius::same(RADIUS), theme::rgb(theme.app));
    grid(ui, theme, plot);
    let painter = clipped(ui, plot);
    for line in chart.lines {
        fill_under(
            &painter,
            plot,
            &plot_points(line.graph.series, plot, scale),
            line.graph.color,
        );
    }
    for (index, line) in chart.lines.iter().enumerate() {
        let points = plot_points(line.graph.series, plot, scale);
        let stroke = Stroke::new(LINE_WIDTH, theme::rgb(line.graph.color));
        if index % 2 == 1 {
            painter.extend(Shape::dashed_line(&points, stroke, SPACE_XS, SPACE_XS));
        } else if points.len() >= 2 {
            painter.add(Shape::line(points, stroke));
        }
    }
    frame(ui, theme, plot);
    for (value, label) in ticks {
        let y = plot.bottom() - plot.height() * value / scale;
        ui.painter().galley(
            egui::pos2(
                plot.left() - SPACE_SM - label.size().x,
                y - label.size().y / 2.0,
            ),
            label,
            theme::rgb(theme.text_muted),
        );
    }
    let capacity = first.graph.series.capacity();
    let span = capacity.saturating_sub(1) as f64 * chart.interval.as_secs_f64();
    let left_tick = ui
        .painter()
        .layout_no_wrap(
            age_label(span),
            TextStyle::Small.resolve(ui.style()),
            theme::rgb(theme.text_muted),
        )
        .size()
        .x;
    let middle_tick = ui
        .painter()
        .layout_no_wrap(
            age_label(span / 2.0),
            TextStyle::Small.resolve(ui.style()),
            theme::rgb(theme.text_muted),
        )
        .size()
        .x;
    for (fraction, alignment) in [
        (0.0, Align2::LEFT_TOP),
        (0.5, Align2::CENTER_TOP),
        (1.0, Align2::RIGHT_TOP),
    ] {
        if fraction == 0.5 && plot.width() < left_tick * 2.0 + middle_tick + SPACE_SM * 2.0 {
            continue;
        }
        let label = age_label(span * (1.0 - fraction));
        ui.painter().text(
            egui::pos2(
                plot.left() + plot.width() * fraction as f32,
                plot.bottom() + SPACE_XS,
            ),
            alignment,
            label,
            TextStyle::Small.resolve(ui.style()),
            theme::rgb(theme.text_muted),
        );
    }
    let id = ui.make_persistent_id(("timeline", chart.label));
    let response = ui.interact(plot, id, Sense::click());
    response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::Role::Unknown, ui.is_enabled(), chart.label)
    });
    let recorded = chart
        .lines
        .iter()
        .map(|line| line.graph.series.len())
        .max()
        .unwrap_or(0);
    let key = id.with("sample-age");
    let mode_key = id.with("keyboard-inspection");
    let pointer_age = response
        .hover_pos()
        .map(|position| nearest_age(plot, capacity, position.x));
    let mut keyboard_mode = ui
        .data(|data| data.get_temp::<bool>(mode_key))
        .unwrap_or(false);
    let mut keyboard_age = ui
        .data(|data| data.get_temp::<usize>(key))
        .unwrap_or(0)
        .min(recorded.saturating_sub(1));
    // A discarded layout pass replays the same input, not another key press.
    let frame_key = id.with("inspection-input-frame");
    let frame = ui.ctx().cumulative_frame_nr();
    let new_input = ui.data(|data| data.get_temp::<u64>(frame_key)) != Some(frame);
    if new_input {
        if ui.input(|input| {
            input
                .events
                .iter()
                .any(|event| matches!(event, egui::Event::PointerMoved(_)))
        }) || response.clicked()
        {
            keyboard_mode = false;
            if let Some(age) = pointer_age {
                keyboard_age = age.min(recorded.saturating_sub(1));
            }
        }
        if response.has_focus() {
            ui.input(|input| {
                if input.key_pressed(egui::Key::ArrowLeft) {
                    keyboard_mode = true;
                    keyboard_age = keyboard_age
                        .saturating_add(1)
                        .min(recorded.saturating_sub(1));
                }
                if input.key_pressed(egui::Key::ArrowRight) {
                    keyboard_mode = true;
                    keyboard_age = keyboard_age.saturating_sub(1);
                }
                if input.key_pressed(egui::Key::Home) {
                    keyboard_mode = true;
                    keyboard_age = recorded.saturating_sub(1);
                }
                if input.key_pressed(egui::Key::End) {
                    keyboard_mode = true;
                    keyboard_age = 0;
                }
                if input.key_pressed(egui::Key::Escape) {
                    response.surrender_focus();
                }
            });
        }
        ui.data_mut(|data| data.insert_temp(frame_key, frame));
    }
    ui.data_mut(|data| {
        data.insert_temp(key, keyboard_age);
        data.insert_temp(mode_key, keyboard_mode);
    });
    let age = if response.has_focus() && keyboard_mode {
        Some(keyboard_age)
    } else {
        pointer_age.or_else(|| response.has_focus().then_some(keyboard_age))
    };
    if recorded > 0 && recorded < capacity {
        let empty_width = sample_x(plot, capacity, recorded - 1) - plot.left();
        if empty_width >= 180.0 {
            ui.painter().text(
                egui::pos2(plot.left() + empty_width / 2.0, plot.center().y),
                Align2::CENTER_CENTER,
                "No earlier samples yet",
                TextStyle::Small.resolve(ui.style()),
                theme::rgb(theme.text_muted),
            );
        }
    }
    if let Some(age) = age {
        let x = sample_x(plot, capacity, age);
        painter.vline(
            x,
            plot.y_range(),
            Stroke::new(1.0, theme::rgb(theme.text_muted)),
        );
        for line in chart.lines {
            if let Some(value) = line.graph.series.from_latest(age) {
                let y = sample_y(plot, value, scale);
                painter.circle_filled(egui::pos2(x, y), SPACE_XS, theme::rgb(theme.app));
                painter.circle_stroke(
                    egui::pos2(x, y),
                    SPACE_XS,
                    Stroke::new(2.0, theme::rgb(line.graph.color)),
                );
            }
        }
        egui::Tooltip::always_open(
            ui.ctx().clone(),
            response.layer_id,
            id,
            egui::PopupAnchor::Position(if response.has_focus() && keyboard_mode {
                egui::pos2(x, plot.center().y)
            } else {
                response
                    .hover_pos()
                    .unwrap_or(egui::pos2(x, plot.center().y))
            }),
        )
        .show(|ui| {
            ui.set_max_width(280.0);
            ui.label(
                egui::RichText::new(age_label(age as f64 * chart.interval.as_secs_f64()))
                    .color(theme::rgb(theme.text)),
            );
            for (index, line) in chart.lines.iter().enumerate() {
                timeline_legend(ui, theme, line, index, line.graph.series.from_latest(age));
            }
            if chart
                .lines
                .iter()
                .all(|line| line.graph.series.from_latest(age).is_none())
            {
                ui.label("No sample recorded here yet.");
            }
        });
    } else if recorded < 2 {
        ui.painter().text(
            plot.center(),
            Align2::CENTER_CENTER,
            "Collecting history…",
            TextStyle::Body.resolve(ui.style()),
            theme::rgb(theme.text_muted),
        );
    }
    widgets::focus_ring(ui, theme, &response);
    theme::gap(ui, SPACE_SM);
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = Vec2::new(SPACE_LG, SPACE_SM);
        for (index, line) in chart.lines.iter().enumerate() {
            let value = match age {
                Some(age) => line.graph.series.from_latest(age),
                None => line.graph.series.from_latest(0),
            };
            timeline_legend(ui, theme, line, index, value);
        }
    });
    rect
}

fn axis_maximum(graph: &Graph<'_>) -> f32 {
    if graph.unit == Unit::Percent {
        graph.series.scale(graph.floor)
    } else {
        graph
            .unit
            .axis_ceiling((graph.series.max() * 1.15).max(graph.floor))
    }
}

fn sample_x(rect: Rect, capacity: usize, age: usize) -> f32 {
    rect.right()
        - rect.width() * age.min(capacity.saturating_sub(1)) as f32
            / capacity.saturating_sub(1).max(1) as f32
}

fn nearest_age(rect: Rect, capacity: usize, x: f32) -> usize {
    (((rect.right() - x) / rect.width().max(1.0)).clamp(0.0, 1.0)
        * capacity.saturating_sub(1) as f32)
        .round() as usize
}

fn age_label(seconds: f64) -> String {
    if seconds < 0.001 {
        return "Latest".to_string();
    }
    if seconds < 1.0 {
        return format!("~{} ms earlier", (seconds * 1_000.0).round() as u64);
    }
    let seconds = seconds.round() as u64;
    if seconds < 60 {
        format!("~{seconds}s earlier")
    } else if seconds < 3600 {
        format!("~{}m {:02}s earlier", seconds / 60, seconds % 60)
    } else {
        format!("~{}h {:02}m earlier", seconds / 3600, (seconds / 60) % 60)
    }
}

fn timeline_legend(
    ui: &mut Ui,
    theme: &Palette,
    line: &TimelineLine<'_>,
    index: usize,
    value: Option<f32>,
) {
    let label = ui.painter().layout_no_wrap(
        line.label.to_string(),
        TextStyle::Small.resolve(ui.style()),
        theme::rgb(theme.text_muted),
    );
    let text = value.map_or_else(
        || crate::format::DASH.to_string(),
        |value| line.graph.unit.value(value),
    );
    let value = ui.painter().layout_no_wrap(
        text,
        TextStyle::Monospace.resolve(ui.style()),
        theme::rgb(theme.text),
    );
    let width = SPACE_MD + SPACE_XS * 2.0 + label.size().x + value.size().x;
    let (rect, _) = ui.allocate_exact_size(
        Vec2::new(
            width.min(ui.max_rect().width()),
            label.size().y.max(value.size().y),
        ),
        Sense::hover(),
    );
    let points = [
        rect.left_center(),
        rect.left_center() + Vec2::new(SPACE_MD, 0.0),
    ];
    let stroke = Stroke::new(2.0, theme::rgb(line.graph.color));
    if index % 2 == 1 {
        ui.painter()
            .extend(Shape::dashed_line(&points, stroke, SPACE_XS, SPACE_XS));
    } else {
        ui.painter().line_segment(points, stroke);
    }
    ui.painter().galley(
        egui::pos2(
            rect.left() + SPACE_MD + SPACE_XS,
            rect.center().y - label.size().y / 2.0,
        ),
        label.clone(),
        theme::rgb(theme.text_muted),
    );
    ui.painter().galley(
        egui::pos2(
            rect.left() + SPACE_MD + SPACE_XS * 2.0 + label.size().x,
            rect.center().y - value.size().y / 2.0,
        ),
        value,
        theme::rgb(theme.text),
    );
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

/// Draws a filled area graph into `rect`, with its axis maximum labelled.
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
        frame(ui, theme, rect);
        axis_label(ui, theme, rect, graph.unit.label(scale));
        return;
    }

    let plot = clipped(ui, rect);
    fill_under(&plot, rect, &points, graph.color);
    plot.add(Shape::line(
        points,
        Stroke::new(LINE_WIDTH, theme::rgb(graph.color)),
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
            Stroke::new(LINE_WIDTH, theme::rgb(total.color)),
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
                Stroke::new(LINE_WIDTH, theme::rgb(graph.color)),
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
    interval: std::time::Duration,
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
        ui.painter()
            .rect_filled(cell_rect, CornerRadius::same(RADIUS), theme::rgb(theme.app));
        let plot_rect = Rect::from_min_max(
            cell_rect.min + Vec2::new(SPACE_XS, SPACE_LG),
            cell_rect.max - Vec2::splat(SPACE_XS),
        );
        let plot = clipped(ui, plot_rect);
        grid(ui, theme, plot_rect);
        let points = plot_points(series, plot_rect, 100.0);
        fill_under(&plot, plot_rect, &points, theme.series(index, count));
        if points.len() >= 2 {
            plot.add(Shape::line(
                points,
                Stroke::new(LINE_WIDTH, theme::rgb(theme.series(index, count))),
            ));
        }
        frame(ui, theme, cell_rect);
        let kind = kinds.get(index).copied().unwrap_or_default();
        let label = match kind.marker() {
            Some(marker) => format!("CPU {index} · {marker}"),
            None => format!("CPU {index}"),
        };
        let font = TextStyle::Small.resolve(ui.style());
        let value = ui.painter().layout_no_wrap(
            crate::format::percent(series.latest().into()),
            font.clone(),
            theme::rgb(theme.text),
        );
        let show_value = cell_rect.width() >= 104.0;
        let label_width = cell_rect.width()
            - SPACE_XS * 2.0
            - if show_value {
                value.size().x + SPACE_SM
            } else {
                0.0
            };
        ui.painter().galley(
            cell_rect.min + Vec2::splat(SPACE_XS),
            widgets::truncated(ui, &label, font, theme.text_muted, label_width),
            theme::rgb(theme.text_muted),
        );
        if show_value {
            ui.painter().galley(
                egui::pos2(
                    cell_rect.right() - SPACE_XS - value.size().x,
                    cell_rect.top() + SPACE_XS,
                ),
                value,
                theme::rgb(theme.text),
            );
        }
        let response = ui.interact(
            cell_rect,
            ui.scope_id().with("core").with(index),
            Sense::hover(),
        );
        if let Some(position) = response.hover_pos() {
            let age = nearest_age(plot_rect, series.capacity(), position.x);
            let value = series.from_latest(age).map_or_else(
                || "Not recorded yet".to_string(),
                |value| Unit::Percent.value(value),
            );
            response.on_hover_text(format!(
                "{} {index}\n{}: {value}\nWindow average {}\nWindow peak {}",
                kind.label(),
                age_label(age as f64 * interval.as_secs_f64()),
                Unit::Percent.value(series.mean()),
                Unit::Percent.value(series.max())
            ));
        }
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
        .map(|(index, value)| Pos2::new(first + step * index as f32, sample_y(rect, value, scale)))
        .collect()
}

/// Maps one sample onto its vertical position inside `rect`.
///
/// Shared by the plotted path and by the hover marker, which is drawn
/// from a value rather than from a point: two copies of this arithmetic
/// is how a dot ends up sitting beside the line it is supposed to be
/// marking.
///
/// The axis maximum lands a [`LINE_CAP`] below the top rather than on it,
/// for the reason given there. The baseline is *not* pulled in to match:
/// a zero sample is meant to rest exactly on the line the area fill ends
/// at, and lifting it off that line would leave a sliver of fill under an
/// idle graph.
fn sample_y(rect: Rect, value: f32, scale: f32) -> f32 {
    let fraction = (value / scale).clamp(0.0, 1.0);
    let top = rect.top() + LINE_CAP;
    rect.bottom() - (rect.bottom() - top).max(0.0) * fraction
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
        theme::rgb(theme.text_muted),
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
        theme::gap(ui, SPACE_XS);
        ui.label(
            egui::RichText::new(label)
                .color(theme::rgb(theme.text_muted))
                .text_style(TextStyle::Small),
        );
        theme::gap(ui, SPACE_XS);
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
    use egui_kittest::kittest::Queryable;

    #[test]
    fn rate_axes_round_in_the_displayed_units_and_keep_peak_headroom() {
        let mut series = Series::new(4);
        series.push(4_200_000.0);
        let graph = Graph {
            series: &series,
            color: Rgb::new(0, 0, 0),
            floor: 0.0,
            unit: Unit::Rate,
        };
        let maximum = axis_maximum(&graph);
        assert!(maximum >= series.max() * 1.15);
        assert_eq!(Unit::Rate.value(maximum), "5.00 MB/s");
        assert_eq!(Unit::Rate.value(maximum / 2.0), "2.50 MB/s");
        assert!(Unit::Rate.axis_ceiling(0.0) >= GRID_BANDS as f32);
        assert!(Unit::Rate.axis_ceiling(f32::MAX).is_finite());
    }

    #[test]
    fn sample_inspection_matches_the_plot_including_unrecorded_history() {
        let rect = panel();
        for capacity in [1, 5, 240] {
            for age in 0..capacity {
                assert_eq!(
                    nearest_age(rect, capacity, sample_x(rect, capacity, age)),
                    age
                );
            }
        }
        assert_eq!(Unit::Percent.label(50.0), "100%");
        assert_eq!(Unit::Percent.value(50.0), "50%");
        assert_eq!(age_label(0.25), "~250 ms earlier");
        assert_eq!(age_label(239.0), "~3m 59s earlier");
    }

    #[test]
    fn keyboard_inspection_keeps_working_with_the_pointer_over_the_chart() -> anyhow::Result<()> {
        let mut series = Series::new(5);
        for value in [10.0, 20.0, 30.0, 40.0, 50.0] {
            series.push(value);
        }
        let mut harness = egui_kittest::Harness::new_ui_state(
            |ui, state: &mut (Series, Option<egui::Id>)| {
                let palette = crate::theme::Catalog::load().get(None).clone();
                theme::apply(ui.ctx(), &palette);
                state.1 = Some(
                    ui.make_persistent_id(("timeline", "CPU timeline"))
                        .with("sample-age"),
                );
                timeline(
                    ui,
                    &palette,
                    200.0,
                    &Timeline {
                        label: "CPU timeline",
                        interval: std::time::Duration::from_secs(1),
                        composition: Composition::Independent,
                        lines: &[TimelineLine {
                            label: "Total",
                            graph: Graph {
                                series: &state.0,
                                color: palette.accent,
                                floor: 100.0,
                                unit: Unit::Percent,
                            },
                        }],
                    },
                );
            },
            (series, None),
        );
        harness.run();
        harness.get_by_label("CPU timeline").focus();
        harness.run();
        harness.key_press(egui::Key::ArrowLeft);
        harness.run();
        let key = harness
            .state()
            .1
            .ok_or_else(|| anyhow::anyhow!("no sample selection key"))?;
        assert_eq!(
            harness.ctx.data(|data| data.get_temp::<usize>(key)),
            Some(1)
        );
        let focus = harness.ctx.memory(|memory| memory.focused());
        harness.get_by_label("CPU timeline").hover();
        harness.run();
        assert_eq!(
            harness.ctx.memory(|memory| memory.focused()),
            focus,
            "hover must keep chart focus"
        );
        assert_eq!(
            harness.ctx.data(|data| data.get_temp::<usize>(key)),
            Some(2),
            "pointer must select the middle sample"
        );
        harness.key_press(egui::Key::ArrowLeft);
        harness.run();
        assert_eq!(
            harness.ctx.data(|data| data.get_temp::<usize>(key)),
            Some(3),
            "the pointer selected age 2, then Left selected age 3"
        );
        assert!(
            harness.query_by_label("~3s earlier").is_some(),
            "the tooltip must describe the selected historical time"
        );
        Ok(())
    }

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
        let rect = panel();
        let points = plot_points(&series, rect, 100.0);
        assert!(
            points
                .iter()
                .all(|point| (point.y - (rect.top() + LINE_CAP)).abs() < 0.01),
            "a saturated series should reach the top of the panel"
        );
        assert!(
            points.iter().all(|point| point.y - LINE_CAP >= rect.top()),
            "and keep its whole stroke inside it: a line centred on the top \
             edge loses the half of its width that falls above the clip, \
             which draws a saturated graph as a flat shaved line along its \
             own frame"
        );
    }

    #[test]
    fn a_zero_series_sits_on_the_baseline() {
        let series = filled(10, 10, 0.0);
        let rect = panel();
        let points = plot_points(&series, rect, 100.0);
        assert!(
            points
                .iter()
                .all(|point| (point.y - rect.bottom()).abs() < 0.01),
            "an idle series should sit exactly on the bottom of the panel — \
             the baseline is where the area fill ends, and a line lifted off \
             it by a half-stroke leaves a sliver of fill under an idle graph"
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
