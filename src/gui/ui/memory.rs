// ============================================================================
// Module:       gui::ui::memory
// Description:  The Memory view — a treemap of what is holding the machine's
//               memory, and a breakdown of whichever process is picked.
//
// Dependencies: egui; crate::treemap, super::{theme, widgets, chrome, graph}
// ============================================================================

//! Where the memory went.
//!
//! The Performance view's Memory panel answers "how much is in use". It
//! is a number and a line, and on a machine that is swapping it is the
//! number you already knew. This view answers the question that follows
//! it — **which processes**, and **what kind of memory** — and it is a
//! separate view rather than another panel because the answer is a
//! picture of four hundred things rather than one more graph.
//!
//! ## Why a treemap
//!
//! Area is the only encoding that compares four hundred quantities at
//! once. A sorted bar chart of four hundred bars is four hundred rows of
//! scrolling, and by the time the reader has scrolled to the small ones
//! the large ones are off screen, so nothing is being compared with
//! anything. A treemap puts every process in one rectangle, each sized
//! by what it is holding, and the answer — "Chrome, and it is not close"
//! — is legible before a single label is read.
//!
//! The layout itself is [`crate::treemap`], which is portable and
//! tested off Windows like the rest of the geometry in this crate.
//!
//! ## Private working set, not working set
//!
//! Tiles are sized by memory the process shares with *nobody*. A plain
//! working set counts every DLL and mapped file against every process
//! that has it open, so a treemap built from working sets has a total
//! several times the machine's RAM — the tiles would be proportional to
//! a quantity that does not exist. See
//! [`crate::model::ProcessRow::private_working_set`].
//!
//! ## Nothing here costs a syscall
//!
//! Every figure this view draws was already in the buffer the process
//! enumeration walks each sample and was previously discarded. Adding
//! the view added fields to a struct, not work to the sampler.

use super::theme::{self, SPACE_LG, SPACE_MD, SPACE_SM, SPACE_XS};
use super::{chrome, widgets};
use crate::gui::app::{App, MemoryMeasure};
use crate::model::{ProcessKey, ProcessRow, Snapshot};
use crate::theme::Palette;
use egui::{Rect, Sense, Ui, Vec2};

/// The least height the treemap gets, whatever else is on screen.
///
/// A treemap's whole value is that the small tiles stay big enough to
/// see. Under about this the tail of the machine — the hundreds of
/// processes holding a megabyte each — collapses into a band of slivers,
/// and the map stops answering anything the Processes view could not.
const MAP_MINIMUM: f32 = 380.0;

/// What the map leaves below itself for the machine's own summary and
/// the key, when no tile is picked.
const SUMMARY_HEIGHT: f32 = 204.0;

/// What it leaves for a picked process's breakdown, which is taller: a
/// composition bar, its key, and two rows of readouts.
const BREAKDOWN_HEIGHT: f32 = 216.0;

/// The narrowest a tile may be before its label is left off.
///
/// A label wider than its tile is drawn over the neighbours, which is
/// worse than no label: it reads as belonging to whichever tile it ends
/// up on top of.
const LABEL_MINIMUM: f32 = 54.0;

// Relations between the constants above, checked when the crate is
// compiled. A `const` block rather than a test: clippy's
// `assertions_on_constants` fires on the test form, and an assertion
// that cannot fail at runtime is not a test.
const _: () = {
    assert!(
        LABEL_MINIMUM > 0.0,
        "a tile has to be allowed to carry a label at some width"
    );
    assert!(
        MAP_MINIMUM > LABEL_MINIMUM,
        "a map shorter than one label is wide cannot show a labelled tile          at all"
    );
    assert!(
        BREAKDOWN_HEIGHT > SUMMARY_HEIGHT,
        "a picked process shows strictly more than the summary it          replaces, so it cannot need less room"
    );
};

/// Draws the Memory view.
pub fn draw(app: &mut App, ui: &mut Ui) {
    let theme = app.theme.clone();
    let Some(snapshot) = app.snapshot.clone() else {
        widgets::empty_state(ui, &theme, "Waiting for the first sample…");
        return;
    };

    toolbar(app, ui, &theme, &snapshot);
    ui.add_space(chrome::TOOLBAR_GAP);

    // Capture the viewport before entering the ScrollArea. Inside a scroll
    // area's content Ui, `available_height` describes its scrollable canvas,
    // not reliably the visible pane. Using it to size the map made the map
    // claim nearly the whole pane and then appended the summary below it,
    // manufacturing a scrollbar even on a tall window.
    let viewport_height = ui.available_height();

    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::VisibleWhenNeeded)
        .show(ui, |ui| {
            // The same trailing trim the Performance view takes, and for
            // the same reason: nothing here is a dense table that has to
            // be squeezed to the pane's own edge.
            let width = (ui.available_width() - SPACE_MD).max(0.0);
            ui.set_max_width(width);

            // The map takes whatever the breakdown below it does not.
            //
            // A fixed height left a third of a tall window empty while
            // the thing the view is *for* was cropped — and a treemap
            // given more room does not merely look better, it shows more
            // of the machine, because every extra square point is
            // another process over the fold-in threshold.
            let height = map_height(viewport_height, app.memory_view.selected.is_some());

            map(app, ui, &theme, &snapshot, height);
            ui.add_space(chrome::SECTION_GAP);
            breakdown(app, ui, &theme, &snapshot);
        });
}

/// Height left for the treemap after the appropriate lower summary.
fn map_height(viewport_height: f32, selected: bool) -> f32 {
    let reserved = if selected {
        BREAKDOWN_HEIGHT
    } else {
        SUMMARY_HEIGHT
    };
    (viewport_height - reserved).max(MAP_MINIMUM)
}

/// The row above the map: what is being measured, and the machine's own
/// totals.
fn toolbar(app: &mut App, ui: &mut Ui, theme: &Palette, snapshot: &Snapshot) {
    ui.horizontal(|ui| {
        for measure in MemoryMeasure::ALL {
            let active = app.memory_view.measure == measure;
            if widgets::chip(
                ui,
                measure.label(),
                if active {
                    theme.accent_soft
                } else {
                    theme.raised
                },
                theme.text,
            )
            .interact(Sense::click())
            .on_hover_text(match measure {
                MemoryMeasure::Resident => {
                    "Size tiles by memory each process is holding in RAM \
                     right now, shared with nobody"
                }
                MemoryMeasure::Committed => {
                    "Size tiles by everything each process has been \
                     promised, whether it is resident or paged out"
                }
            })
            .clicked()
            {
                app.memory_view.measure = measure;
            }
            ui.add_space(SPACE_XS);
        }

        chrome::toolbar_dot(ui, theme);

        // The machine's own figure beside the sum of the tiles, because
        // the two are not the same and the difference is the point: what
        // the processes hold privately, plus the cache and the kernel,
        // is what is in use.
        let held: u64 = snapshot
            .processes
            .iter()
            .map(|row| app.memory_view.measure.of(row))
            .sum();
        ui.label(
            egui::RichText::new(format!(
                "{} across {} processes · {} of {} in use on the machine",
                crate::format::bytes(held),
                crate::format::count(snapshot.processes.len() as u64),
                crate::format::bytes(snapshot.system.memory.used()),
                crate::format::bytes(snapshot.system.memory.total),
            ))
            .color(theme::rgb(theme.text_muted))
            .text_style(egui::TextStyle::Small),
        );
    });
}

/// The smallest tile worth drawing, in square points.
///
/// Below this a tile is a speck: too small to label, too small to click,
/// and too small for its area to be judged against anything. On a real
/// machine the tail is *most* of the list — four hundred processes
/// holding a megabyte each — and drawn individually they turn the corner
/// of the map into a field of noise that reads as a rendering fault.
///
/// Roughly a twenty-point square. Everything under it is folded into one
/// tile, which is both honest and more useful: "368 smaller" is a fact,
/// where three hundred specks are not.
///
/// The figure is an *area*, and a tile's area is exactly its share of
/// the map — so this is a guarantee rather than a heuristic: nothing
/// smaller than this is ever drawn. Tuned by looking at a real machine,
/// where the difference between forty-eight square points and four
/// hundred is the difference between a corner of visual noise and a map
/// whose every tile can be pointed at.
const TILE_MINIMUM: f32 = 400.0;

/// One tile's subject: a process, or everything too small to draw.
enum Slice<'a> {
    /// One process, drawn and clickable.
    Process(&'a ProcessRow),
    /// The tail, folded together.
    Others {
        /// How many processes it stands for.
        count: usize,
    },
}

/// The treemap.
fn map(app: &mut App, ui: &mut Ui, theme: &Palette, snapshot: &Snapshot, height: f32) {
    let measure = app.memory_view.measure;

    let (rect, response) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), height), Sense::click());
    ui.painter().rect_filled(
        rect,
        egui::CornerRadius::same(theme::RADIUS),
        theme::rgb(theme.panel),
    );

    // Everything too small to draw is folded into one tile. The
    // threshold is a share of the *map*, not a fixed byte count, so it
    // follows the window: a taller pane draws more of the tail.
    let total: f64 = snapshot
        .processes
        .iter()
        .map(|row| measure.of(row) as f64)
        .sum();
    let map_area = rect.width() * rect.height();
    let floor = if map_area > 0.0 {
        total * f64::from(TILE_MINIMUM / map_area)
    } else {
        0.0
    };

    let mut slices: Vec<Slice<'_>> = Vec::new();
    let mut weights: Vec<f64> = Vec::new();
    let mut tail = 0.0f64;
    let mut tail_count = 0usize;
    for row in &snapshot.processes {
        let weight = measure.of(row) as f64;
        if weight <= 0.0 {
            continue;
        }
        if weight < floor {
            tail += weight;
            tail_count += 1;
            continue;
        }
        slices.push(Slice::Process(row));
        weights.push(weight);
    }
    if tail_count > 0 {
        slices.push(Slice::Others { count: tail_count });
        weights.push(tail);
    }

    let tiles = crate::treemap::layout(&weights, rect.width(), rect.height());
    if tiles.is_empty() {
        widgets::empty_state(ui, theme, "No process is holding any memory");
        return;
    }

    let pointer = response.hover_pos();
    let mut clicked = None;
    let mut hovered: Option<&ProcessRow> = None;

    for tile in &tiles {
        let Some(slice) = slices.get(tile.index) else {
            continue;
        };
        // Inset by a point, so neighbouring tiles are separated by a
        // seam of the panel behind rather than sharing an edge. Without
        // it a treemap reads as one mottled sheet.
        let placed = Rect::from_min_size(
            egui::pos2(rect.left() + tile.x, rect.top() + tile.y),
            Vec2::new(tile.width, tile.height),
        )
        .shrink(1.0);
        if placed.width() <= 0.0 || placed.height() <= 0.0 {
            continue;
        }

        let under = pointer.is_some_and(|at| placed.contains(at));
        let (name, amount, base, selected) = match slice {
            Slice::Process(row) => {
                if under {
                    hovered = Some(row);
                    if response.clicked() {
                        clicked = Some(row.key());
                    }
                }
                (
                    row.display_name().to_string(),
                    measure.of(row),
                    // Coloured by *kind*, the one fact that groups the
                    // tiles into something navigable: the apps in one
                    // hue, the machine's own processes in another. A hue
                    // per process would be four hundred colours meaning
                    // nothing — see the note in `super::processes` about
                    // the dot that used to be on every row.
                    kind_colour(theme, row.kind),
                    app.memory_view.selected == Some(row.key()),
                )
            }
            // Not clickable and not selectable: it is not a process, and
            // a tile that highlights under the pointer while doing
            // nothing when clicked is a worse lie than one that stays
            // still.
            Slice::Others { count } => (
                format!("{count} smaller"),
                weights.get(tile.index).copied().unwrap_or(0.0) as u64,
                theme.border_strong,
                false,
            ),
        };

        let fill = if selected {
            theme.accent
        } else if under && matches!(slice, Slice::Process(_)) {
            base.lerp(theme.text, 0.25)
        } else {
            base
        };
        ui.painter()
            .rect_filled(placed, egui::CornerRadius::same(2), theme::rgb(fill));

        if placed.width() >= LABEL_MINIMUM && placed.height() >= theme::ROW_HEIGHT {
            let label = widgets::truncated(
                ui,
                &name,
                egui::TextStyle::Small.resolve(ui.style()),
                theme.text,
                placed.width() - SPACE_SM,
            );
            ui.painter().galley(
                placed.left_top() + Vec2::new(SPACE_XS, SPACE_XS),
                label,
                theme::rgb(theme.text),
            );
            if placed.width() >= 82.0 && placed.height() >= theme::ROW_HEIGHT * 2.0 {
                let amount = widgets::truncated(
                    ui,
                    &crate::format::bytes(amount),
                    egui::TextStyle::Small.resolve(ui.style()),
                    theme.text_muted,
                    placed.width() - SPACE_SM,
                );
                ui.painter().galley(
                    placed.left_top() + Vec2::new(SPACE_XS, theme::ROW_HEIGHT),
                    amount,
                    theme::rgb(theme.text_muted),
                );
            }
        }
    }

    if let Some(row) = hovered {
        response.clone().on_hover_text(format!(
            "{}\nPID {}\nIn RAM {} · committed {}\nShared with others {}",
            row.display_name(),
            row.pid,
            crate::format::bytes(row.private_working_set),
            crate::format::bytes(row.private_bytes),
            crate::format::bytes(row.shared_working_set()),
        ));
    }
    if let Some(key) = clicked {
        // Clicking the selected tile clears it, so the way out is the
        // way in — the same rule the Network panel's adapter rows use.
        app.memory_view.selected = if app.memory_view.selected == Some(key) {
            None
        } else {
            Some(key)
        };
    }
}

/// A process kind's colour on the map.
///
/// Three hues from the theme's own ramp rather than three chosen
/// colours, so the map belongs to whatever theme is loaded — the same
/// rule the graphs follow.
fn kind_colour(theme: &Palette, kind: crate::model::ProcessKind) -> crate::color::Rgb {
    /// How far each ramp hue is pulled back towards the surface behind
    /// it.
    ///
    /// The ramp is built for *lines* — a stroke a point or two wide,
    /// where full chroma is what makes it visible at all. The same
    /// colour across a tile two hundred points on a side is a slab of
    /// neon, and a map made of them is unreadable in the way a highlighter
    /// is: nothing recedes, so nothing stands out, and the selected tile
    /// has nothing to be brighter *than*.
    ///
    /// Pulled back this far the hues still separate the three kinds at a
    /// glance, and the accent on a selected tile is unmistakable.
    const MUTED: f32 = 0.62;

    let count = crate::model::ProcessKind::ALL.len();
    let index = crate::model::ProcessKind::ALL
        .iter()
        .position(|candidate| *candidate == kind)
        .unwrap_or(0);
    theme.series(index, count).lerp(theme.panel, MUTED)
}

/// The selected process's own breakdown, or the machine's if none is
/// picked.
fn breakdown(app: &mut App, ui: &mut Ui, theme: &Palette, snapshot: &Snapshot) {
    let selected = app
        .memory_view
        .selected
        .and_then(|key| find(snapshot, key).cloned());

    let Some(row) = selected else {
        machine(ui, theme, snapshot);
        return;
    };

    widgets::section(
        ui,
        theme,
        &format!("{} · PID {}", row.display_name(), row.pid),
    );

    // The composition bar: the three states a process's memory can be
    // in, drawn end to end at their real proportions. This is the
    // "how", where the treemap is the "who".
    let shared = row.shared_working_set();
    let paged_out = row.paged_out();
    composition(
        ui,
        theme,
        &[
            (
                "Private, in RAM",
                row.private_working_set,
                theme.series(0, 3),
            ),
            ("Shared with others", shared, theme.series(1, 3)),
            ("Committed, paged out", paged_out, theme.series(2, 3)),
        ],
    );
    ui.add_space(chrome::SECTION_GAP);

    ui.horizontal_top(|ui| {
        let width = stat_width(ui.available_width(), 4);
        stat(
            ui,
            theme,
            width,
            "Working set",
            &crate::format::bytes(row.working_set),
        );
        stat(
            ui,
            theme,
            width,
            "Private commit",
            &crate::format::bytes(row.private_bytes),
        );
        stat(
            ui,
            theme,
            width,
            "Peak working set",
            &crate::format::bytes(row.peak_working_set),
        );
        stat(
            ui,
            theme,
            width,
            "Peak commit",
            &crate::format::bytes(row.peak_private_bytes),
        );
    });
    ui.add_space(SPACE_MD);
    ui.horizontal_top(|ui| {
        let width = stat_width(ui.available_width(), 4);
        stat(
            ui,
            theme,
            width,
            "Paged pool",
            &crate::format::bytes(row.paged_pool),
        );
        stat(
            ui,
            theme,
            width,
            "Non-paged pool",
            &crate::format::bytes(row.nonpaged_pool),
        );
        stat(
            ui,
            theme,
            width,
            "Page faults",
            &crate::format::count(row.page_faults),
        );
        // The number that separates a process which is merely large from
        // one that is thrashing: these are the faults that had to reach
        // the disk.
        stat(
            ui,
            theme,
            width,
            "Hard faults",
            &format!("{:.1}/s", row.hard_fault_rate),
        );
    });
}

/// What is shown until a tile is picked: the machine's own composition,
/// and the map's key.
///
/// Rather than an empty half-view saying "pick something". The same
/// stacked bar the per-process breakdown uses, so the two read as the
/// same measurement at two scales — and this one answers the question
/// the treemap raises: the tiles add up to less than the memory in use,
/// and the difference is cache and kernel rather than a rounding error.
fn machine(ui: &mut Ui, theme: &Palette, snapshot: &Snapshot) {
    let memory = &snapshot.system.memory;
    let held: u64 = snapshot
        .processes
        .iter()
        .map(|row| row.private_working_set)
        .sum();
    // What the processes hold privately cannot exceed what is in use;
    // the two come from different counters sampled a moment apart, so
    // the subtraction saturates rather than wrapping.
    let elsewhere = memory.used().saturating_sub(held);

    widgets::section(ui, theme, "This machine");
    composition(
        ui,
        theme,
        &[
            ("Held by processes", held, theme.series(0, 3)),
            ("Kernel and drivers", elsewhere, theme.series(1, 3)),
            ("Cached", memory.cached, theme.series(2, 3)),
            (
                "Free",
                memory.available.saturating_sub(memory.cached),
                theme.border_strong,
            ),
        ],
    );
    ui.add_space(chrome::SECTION_GAP);
    legend(ui, theme);
}

/// The map's key.
fn legend(ui: &mut Ui, theme: &Palette) {
    widgets::section(ui, theme, "Pick a tile to break a process down");
    ui.horizontal(|ui| {
        for kind in crate::model::ProcessKind::ALL {
            let (swatch, _) = ui.allocate_exact_size(Vec2::splat(SPACE_MD), Sense::hover());
            ui.painter().rect_filled(
                swatch,
                egui::CornerRadius::same(2),
                theme::rgb(kind_colour(theme, kind)),
            );
            ui.add_space(SPACE_XS);
            ui.label(
                egui::RichText::new(kind.label())
                    .color(theme::rgb(theme.text_muted))
                    .text_style(egui::TextStyle::Small),
            );
            ui.add_space(SPACE_LG);
        }
    });
}

/// A stacked bar showing how one total divides.
///
/// Proportional, labelled underneath, and it does not draw a segment it
/// cannot label — a two-point sliver with a caption beside it is a
/// caption pointing at nothing.
fn composition(ui: &mut Ui, theme: &Palette, parts: &[(&str, u64, crate::color::Rgb)]) {
    /// The bar's height. Taller than `widgets::meter`: this one is the
    /// subject rather than an indicator beside a number.
    const HEIGHT: f32 = 22.0;

    let total: u64 = parts.iter().map(|(_, value, _)| value).sum();
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), HEIGHT), Sense::hover());
    ui.painter().rect_filled(
        rect,
        egui::CornerRadius::same(theme::RADIUS),
        theme::rgb(theme.raised),
    );
    if total == 0 {
        return;
    }

    let mut x = rect.left();
    for (_, value, colour) in parts {
        let span = (*value as f64 / total as f64) as f32 * rect.width();
        if span <= 0.0 {
            continue;
        }
        let segment =
            Rect::from_min_size(egui::pos2(x, rect.top()), Vec2::new(span, rect.height()));
        ui.painter()
            .rect_filled(segment, egui::CornerRadius::same(2), theme::rgb(*colour));
        x += span;
    }

    ui.add_space(SPACE_SM);
    ui.horizontal_wrapped(|ui| {
        for (label, value, colour) in parts {
            let (swatch, _) = ui.allocate_exact_size(Vec2::splat(SPACE_MD), Sense::hover());
            ui.painter()
                .rect_filled(swatch, egui::CornerRadius::same(2), theme::rgb(*colour));
            ui.add_space(SPACE_XS);
            ui.label(
                egui::RichText::new(*label)
                    .color(theme::rgb(theme.text_muted))
                    .text_style(egui::TextStyle::Small),
            );
            ui.add_space(SPACE_XS);
            ui.label(
                egui::RichText::new(crate::format::bytes(*value))
                    .color(theme::rgb(theme.text))
                    .text_style(egui::TextStyle::Monospace),
            );
            ui.add_space(SPACE_LG);
        }
    });
}

/// The width one readout of a row gets. See
/// `super::performance::stat_column_width`, which this mirrors — the two
/// views' rows have to line up when someone switches between them.
fn stat_width(available: f32, columns: usize) -> f32 {
    let columns = columns.max(1);
    let gaps = SPACE_SM * (columns - 1) as f32;
    ((available - gaps) / columns as f32).max(0.0)
}

/// One readout: a caption over a value.
fn stat(ui: &mut Ui, theme: &Palette, width: f32, caption: &str, value: &str) {
    ui.allocate_ui_with_layout(
        Vec2::new(width, 0.0),
        egui::Layout::top_down(egui::Align::Min),
        |ui| {
            // Pinned, for the reason `performance::stat_column` explains
            // at length: without it each column is as wide as its own
            // caption and the row comes out ransom-note width.
            ui.set_min_width(width);
            ui.label(
                egui::RichText::new(caption)
                    .color(theme::rgb(theme.text_muted))
                    .text_style(egui::TextStyle::Small),
            );
            ui.label(
                egui::RichText::new(value)
                    .color(theme::rgb(theme.text))
                    .text_style(egui::TextStyle::Monospace),
            );
        },
    );
}

/// The row a key names in this snapshot, if it is still running.
fn find(snapshot: &Snapshot, key: ProcessKey) -> Option<&ProcessRow> {
    snapshot.processes.iter().find(|row| row.key() == key)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_roomy_memory_view_does_not_manufacture_scroll_overflow() {
        let viewport = 700.0;
        assert_eq!(map_height(viewport, false), viewport - SUMMARY_HEIGHT);
        assert_eq!(map_height(viewport, true), viewport - BREAKDOWN_HEIGHT);
    }

    #[test]
    fn a_short_memory_view_preserves_the_readable_map_floor() {
        assert_eq!(map_height(320.0, false), MAP_MINIMUM);
        assert_eq!(map_height(320.0, true), MAP_MINIMUM);
    }

    #[test]
    fn the_readout_columns_share_the_row_without_overflowing_it() {
        // The same arithmetic as the Performance view's stat rows, and
        // the same regression: `set_min_width` makes every column claim
        // its share exactly, so a share that ignored the gaps between
        // them would overflow the row.
        let available = 800.0;
        for columns in 1..=4usize {
            let width = stat_width(available, columns);
            let claimed = columns as f32 * width + (columns - 1) as f32 * SPACE_SM;
            assert!(
                claimed <= available,
                "{columns} columns of {width} plus their gaps is {claimed}, \
                 which overflows {available}"
            );
        }
        assert!((stat_width(0.0, 4) - 0.0).abs() < f32::EPSILON);
    }

    #[test]
    fn every_process_kind_gets_its_own_colour_on_the_map() {
        // The map is navigable because the tiles group by kind. Two
        // kinds sharing a hue would make that grouping a lie.
        let theme = crate::theme::Catalog::load().get(None).clone();
        let colours: Vec<crate::color::Rgb> = crate::model::ProcessKind::ALL
            .iter()
            .map(|kind| kind_colour(&theme, *kind))
            .collect();
        for (index, colour) in colours.iter().enumerate() {
            for other in colours.iter().skip(index + 1) {
                assert!(
                    colour != other,
                    "two process kinds are drawn in the same colour, so the \
                     map cannot be read by kind"
                );
            }
        }
    }
}
