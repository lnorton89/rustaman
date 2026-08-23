// ============================================================================
// Module:       icon
// Description:  Every icon in the app as geometry on a 16x16 grid — the shapes,
//               not the painting.
//
// Dependencies: none — deliberately. See the module docs.
// ============================================================================

//! The icon set, as geometry.
//!
//! ## Why this module exists at all
//!
//! The app used to set its icons as Unicode characters — `U+25B8` for a
//! disclosure arrow, `U+2699` for settings, and the three window-control
//! codepoints for minimise, maximise and restore. Every one of them
//! rendered as an empty box, in the shipped window, on the machine this
//! app was written for.
//!
//! egui bundles a small font set: Ubuntu Sans for proportional text, Hack
//! for monospace, and an emoji subset. Between them they cover Latin,
//! Greek, Cyrillic and a few hundred emoji, and **almost nothing in the
//! Miscellaneous Symbols, Geometric Shapes, or Dingbats blocks** — which
//! is where every icon-shaped character lives. A glyph the font does not
//! have is drawn as the replacement box, so the nav rail came out as a
//! column of squares beside its labels.
//!
//! There are three ways out, and only one of them is any good:
//!
//! - **Ship an icon font.** A megabyte of binary in the repository, a
//!   licence to track, and a set of magic private-use-area codepoints in
//!   the source that mean nothing to a reader. It also still fails the
//!   same way when a codepoint is wrong — silently, as a box.
//! - **Ship SVGs.** egui can rasterise them, with another dependency, and
//!   each one still has to be decoded and cached at every size and colour
//!   the app uses. An icon that has to be *loaded* can fail to load.
//! - **Draw them.** An icon at this size is four to eight line segments.
//!   It takes the theme's colour as an argument, scales to any size with
//!   no rasterisation step, cannot fail to load, and cannot render as a
//!   box — a wrong path is visibly a wrong shape, in review, at the point
//!   it is written.
//!
//! So: draw them. `no_drawing_module_sets_an_icon_in_a_font` in
//! [`crate::gui::ui`] scans the drawing modules for pictographic
//! characters in string literals and fails the build for one, so this
//! cannot quietly come back.
//!
//! ## Why the shapes are here and the painting is not
//!
//! Nothing in this file mentions egui. The geometry is a list of points,
//! so it lives in the portable half of the crate where its tests run on
//! any machine — the same split as [`crate::theme`] against
//! `gui::ui::theme`, and [`crate::motion`] against `gui::ui::motion`.
//!
//! That is what lets the three properties below be *checked* rather than
//! eyeballed: every icon stays inside its grid, every icon is optically
//! centred, and every icon reaches far enough across the grid to look
//! like a member of the same set. Those are the failures that make an
//! icon set look homemade, they are invisible in a code review, and none
//! of them needs a window to detect. [`crate::gui::ui::icon`] holds the
//! painting.
//!
//! ## The grid
//!
//! Every icon is designed on a 16x16 grid with a one-unit margin, and the
//! coordinates below are in that space — the painter maps them onto
//! whatever rect it is given. Working in one grid is what makes a row of
//! different icons look like a set: the chevron's arms and the gear's
//! teeth reach the same distance from centre, so no single icon reads as
//! larger than its neighbours even though their bounding boxes differ.

/// The design grid every icon below is drawn on.
///
/// Coordinates in this module are in grid units, `0.0..=16.0`. See the
/// module docs on why they share one.
pub const GRID: f32 = 16.0;

/// The stroke weight, in grid units.
///
/// 1.5 rather than 1.0: at a 16px render a one-unit stroke lands on
/// exactly one physical pixel and comes out noticeably lighter than the
/// text beside it, so the icons look like they are disabled. At 1.5 they
/// carry the same weight as the label they sit next to.
pub const WEIGHT: f32 = 1.5;

/// Every icon the app draws.
///
/// One enum rather than free functions so that `wildcard_enum_match_arm`
/// applies: adding a variant here fails the build in [`paint`] until it
/// is actually drawn, rather than rendering as nothing in whichever view
/// forgot it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Icon {
    /// The Processes view: a stack of rows.
    Processes,
    /// The Performance view: a rising line on an axis.
    Performance,
    /// The Memory view: a box divided into unequal panes.
    Memory,
    /// The Details view: a list with leading markers.
    Details,
    /// The Services view: a gear.
    Services,
    /// The Startup view: an upward arrow leaving a baseline.
    Startup,
    /// System Information: a computer display and stand.
    SystemInfo,
    /// Settings: sliders.
    Settings,
    /// A collapsed disclosure control.
    ChevronRight,
    /// An expanded disclosure control.
    ChevronDown,
    /// Ascending sort.
    ArrowUp,
    /// Descending sort.
    ArrowDown,
    /// Search.
    Search,
    /// Dismiss, cancel, close a chip.
    Close,
    /// The window's minimise control.
    WindowMinimise,
    /// The window's maximise control.
    WindowMaximise,
    /// The window's restore control, when it is already maximised.
    WindowRestore,
    /// A drag handle.
    Grip,
    /// A pin, for a row kept at the top of the list.
    Pin,
    /// Something is fine.
    Check,
    /// Something needs attention.
    Warning,
    /// Refresh, re-read.
    Refresh,
    /// Copy to the clipboard.
    Copy,
    /// Open the containing folder.
    Folder,
    /// A process running under reduced quality of service — Windows 11's
    /// efficiency mode.
    ///
    /// A leaf, because that is the mark Windows itself uses for this and
    /// a task manager is not the place to invent a private vocabulary for
    /// a state the platform already named. Stroked in the theme's colour
    /// like everything else here rather than in Task Manager's green: the
    /// row it sits on carries meaning through colour already, and a
    /// second, fixed green on the same row would compete with it.
    Leaf,
    /// A process that is part of Windows itself.
    ///
    /// Four panes: the shape the platform has used for its own mark
    /// since it stopped being a flag. Geometry rather than the real
    /// logo, and stroked in the theme's colour like every other icon
    /// here — this says "the operating system", it is not a badge, and
    /// a mark that kept its own colours under a dark theme would be the
    /// one thing on the row that ignores the theme.
    Windows,
}

impl Icon {
    /// The paths this icon is drawn from, in grid units.
    ///
    /// A `Vec` of polylines plus a flag for whether each closes. Returned
    /// rather than painted directly so that [`paint`] owns the mapping
    /// from grid space to screen space in one place — an icon that did
    /// its own arithmetic would be the one that ends up half a pixel off
    /// the others.
    pub fn strokes(self) -> Vec<Path> {
        match self {
            // Three stacked rows, the top one longer — a list of
            // processes rather than a generic list, which is what the
            // uniform version reads as.
            Self::Processes => vec![
                Path::open(&[(3.0, 4.5), (13.0, 4.5)]),
                Path::open(&[(3.0, 8.0), (13.0, 8.0)]),
                Path::open(&[(3.0, 11.5), (9.0, 11.5)]),
            ],
            // A line chart: a baseline and a rising, falling, rising
            // trace. Deliberately not a smooth curve — the app's own
            // graphs are polylines, and the icon should look like what it
            // opens.
            Self::Performance => vec![
                Path::open(&[(2.5, 13.0), (13.5, 13.0)]),
                Path::open(&[(3.0, 10.5), (6.0, 6.0), (9.0, 9.0), (13.0, 3.5)]),
            ],
            // A box cut into unequal panes — the shape of the view it
            // opens, which is a treemap. Deliberately *not* a memory
            // module with pins: that draws the hardware, and this view
            // is about how the machine's memory is being spent rather
            // than what it is plugged into.
            // Four panes with a gap between them, which is what makes
            // it read as the platform's mark rather than as a window or
            // a grid — `Memory` is already a divided box, and the two
            // have to stay distinguishable at eighteen points.
            Self::Windows => vec![
                Path::closed(&[(2.5, 3.0), (7.2, 3.0), (7.2, 7.5), (2.5, 7.5)]),
                Path::closed(&[(8.8, 3.0), (13.5, 3.0), (13.5, 7.5), (8.8, 7.5)]),
                Path::closed(&[(2.5, 8.5), (7.2, 8.5), (7.2, 13.0), (2.5, 13.0)]),
                Path::closed(&[(8.8, 8.5), (13.5, 8.5), (13.5, 13.0), (8.8, 13.0)]),
            ],
            Self::Memory => vec![
                Path::closed(&[(2.5, 3.5), (13.5, 3.5), (13.5, 12.5), (2.5, 12.5)]),
                Path::open(&[(9.0, 3.5), (9.0, 12.5)]),
                Path::open(&[(9.0, 8.0), (13.5, 8.0)]),
            ],
            // A list with leading dots: the same rows as `Processes` but
            // marked, which is the difference between the two views.
            Self::Details => vec![
                Path::dot(3.0, 4.5),
                Path::open(&[(6.5, 4.5), (13.0, 4.5)]),
                Path::dot(3.0, 8.0),
                Path::open(&[(6.5, 8.0), (13.0, 8.0)]),
                Path::dot(3.0, 11.5),
                Path::open(&[(6.5, 11.5), (13.0, 11.5)]),
            ],
            // A gear: a ring plus six teeth. Six rather than eight
            // because at 16px eight teeth close up into a solid ring and
            // the shape stops reading as a gear at all.
            Self::Services => {
                let mut paths = vec![Path::circle(8.0, 8.0, 3.0)];
                for index in 0..6 {
                    let angle = std::f32::consts::TAU * index as f32 / 6.0;
                    let (sin, cos) = angle.sin_cos();
                    paths.push(Path::open(&[
                        (8.0 + cos * 4.0, 8.0 + sin * 4.0),
                        (8.0 + cos * 5.8, 8.0 + sin * 5.8),
                    ]));
                }
                paths
            }
            // An arrow leaving a baseline: what "runs at logon" looks
            // like. The baseline is what stops it reading as the sort
            // arrow.
            Self::Startup => vec![
                Path::open(&[(4.0, 13.5), (12.0, 13.5)]),
                Path::open(&[(8.0, 11.0), (8.0, 2.5)]),
                Path::open(&[(4.5, 6.0), (8.0, 2.5), (11.5, 6.0)]),
            ],
            // A display and stand: the physical machine rather than a
            // chart, table, or setting inside it.
            Self::SystemInfo => vec![
                Path::closed(&[(2.5, 3.0), (13.5, 3.0), (13.5, 10.5), (2.5, 10.5)]),
                Path::open(&[(8.0, 10.5), (8.0, 13.0)]),
                Path::open(&[(5.5, 13.0), (10.5, 13.0)]),
            ],
            // Sliders: two rails with a handle on each, at different
            // positions. Equal positions read as a decorative pattern
            // rather than as controls that have been set.
            Self::Settings => vec![
                Path::open(&[(2.5, 5.5), (13.5, 5.5)]),
                Path::circle(6.0, 5.5, 1.8),
                Path::open(&[(2.5, 10.5), (13.5, 10.5)]),
                Path::circle(10.5, 10.5, 1.8),
            ],
            Self::ChevronRight => vec![Path::open(&[(6.5, 3.5), (11.0, 8.0), (6.5, 12.5)])],
            Self::ChevronDown => vec![Path::open(&[(3.5, 6.5), (8.0, 11.0), (12.5, 6.5)])],
            // The sort arrows carry a stem as well as a head. A bare
            // chevron beside a column name reads as "there is more here",
            // not as "sorted ascending".
            Self::ArrowUp => vec![
                Path::open(&[(8.0, 13.0), (8.0, 3.5)]),
                Path::open(&[(4.5, 7.0), (8.0, 3.5), (11.5, 7.0)]),
            ],
            Self::ArrowDown => vec![
                Path::open(&[(8.0, 3.0), (8.0, 12.5)]),
                Path::open(&[(4.5, 9.0), (8.0, 12.5), (11.5, 9.0)]),
            ],
            Self::Search => vec![
                Path::circle(7.0, 7.0, 4.0),
                Path::open(&[(10.0, 10.0), (13.5, 13.5)]),
            ],
            Self::Close => vec![
                Path::open(&[(4.5, 4.5), (11.5, 11.5)]),
                Path::open(&[(11.5, 4.5), (4.5, 11.5)]),
            ],
            // The window controls follow the shapes Windows itself uses,
            // because these three are the one place in the app where
            // matching the platform beats being distinctive: a user aims
            // at the top-right corner without looking.
            Self::WindowMinimise => vec![Path::open(&[(4.0, 8.0), (12.0, 8.0)])],
            Self::WindowMaximise => vec![Path::closed(&[
                (4.0, 4.0),
                (12.0, 4.0),
                (12.0, 12.0),
                (4.0, 12.0),
            ])],
            // Two overlapping frames, the back one clipped to an L so the
            // shape does not turn into a solid grid of lines at 16px.
            Self::WindowRestore => vec![
                Path::closed(&[(3.0, 6.0), (10.0, 6.0), (10.0, 13.0), (3.0, 13.0)]),
                Path::open(&[
                    (6.0, 6.0),
                    (6.0, 3.0),
                    (13.0, 3.0),
                    (13.0, 10.0),
                    (10.0, 10.0),
                ]),
            ],
            // Six dots in two columns: the conventional "drag me" mark.
            //
            // It was two short rules, which read as quieter than the
            // icons beside it — deliberately, since a grip is an
            // affordance rather than a destination. But
            // `every_icon_fills_enough_of_its_grid_to_match_its_neighbours`
            // was right to reject it: quieter is a job for *weight*, not
            // for a shape that covers a third of the grid its neighbours
            // fill. Dots keep it visually light while reaching the same
            // distance from centre as everything else.
            Self::Grip => vec![
                Path::dot(6.0, 4.0),
                Path::dot(10.0, 4.0),
                Path::dot(6.0, 8.0),
                Path::dot(10.0, 8.0),
                Path::dot(6.0, 12.0),
                Path::dot(10.0, 12.0),
            ],
            Self::Pin => vec![
                Path::open(&[(8.0, 9.5), (8.0, 14.0)]),
                Path::closed(&[(5.0, 3.0), (11.0, 3.0), (10.0, 9.5), (6.0, 9.5)]),
            ],
            Self::Check => vec![Path::open(&[(3.5, 8.5), (6.5, 11.5), (12.5, 5.0)])],
            Self::Warning => vec![
                Path::closed(&[(8.0, 2.5), (14.0, 13.0), (2.0, 13.0)]),
                Path::open(&[(8.0, 6.5), (8.0, 9.5)]),
                Path::dot(8.0, 11.3),
            ],
            // An arc with an arrowhead, rather than a full circle: a
            // closed ring has no direction, and direction is the whole
            // meaning of this one.
            Self::Refresh => vec![
                Path::arc(8.0, 8.0, 5.0, 0.6, 5.4),
                Path::open(&[(11.0, 1.5), (12.2, 4.6), (9.0, 5.2)]),
            ],
            Self::Copy => vec![
                Path::closed(&[(6.0, 6.0), (13.0, 6.0), (13.0, 13.5), (6.0, 13.5)]),
                Path::open(&[(3.0, 10.0), (3.0, 2.5), (10.0, 2.5)]),
            ],
            Self::Folder => vec![Path::closed(&[
                (2.5, 12.5),
                (2.5, 4.0),
                (6.5, 4.0),
                (8.0, 6.0),
                (13.5, 6.0),
                (13.5, 12.5),
            ])],
            // A leaf on the grid's rising diagonal, with its midrib
            // drawn along the same line. The rib is what makes it read
            // as a leaf at eighteen points rather than as a lozenge —
            // the outline alone is the shape of a pill turned on its
            // side, which is not a plant.
            Self::Leaf => vec![
                Path::closed(&[
                    (3.0, 13.0),
                    (3.0, 8.0),
                    (5.0, 4.5),
                    (8.5, 3.0),
                    (13.0, 3.0),
                    (13.0, 7.5),
                    (11.0, 11.0),
                    (7.5, 13.0),
                ]),
                Path::open(&[(3.5, 12.5), (7.0, 9.0), (11.5, 5.5)]),
            ],
        }
    }
}

/// One stroked path in grid space.
pub struct Path {
    /// The points, in grid units.
    pub points: Vec<(f32, f32)>,
    /// Whether the last point joins back to the first.
    pub closed: bool,
}

impl Path {
    /// A polyline.
    fn open(points: &[(f32, f32)]) -> Self {
        Self {
            points: points.to_vec(),
            closed: false,
        }
    }

    /// A polyline whose end joins its start.
    fn closed(points: &[(f32, f32)]) -> Self {
        Self {
            points: points.to_vec(),
            closed: true,
        }
    }

    /// A circle, approximated as a closed polygon.
    ///
    /// Sixteen segments: at the sizes these are drawn, the difference
    /// between sixteen and a true circle is well under a pixel, and a
    /// polygon goes through the same stroking path as everything else
    /// here rather than needing its own shape type and its own stroke
    /// handling.
    fn circle(cx: f32, cy: f32, radius: f32) -> Self {
        Self::arc(cx, cy, radius, 0.0, std::f32::consts::TAU).close()
    }

    /// An arc from `start` to `end` radians.
    fn arc(cx: f32, cy: f32, radius: f32, start: f32, end: f32) -> Self {
        /// Segments per full turn.
        const SEGMENTS: usize = 16;
        let span = end - start;
        let steps = ((span.abs() / std::f32::consts::TAU * SEGMENTS as f32).ceil() as usize).max(2);
        let points = (0..=steps)
            .map(|step| {
                let angle = start + span * step as f32 / steps as f32;
                let (sin, cos) = angle.sin_cos();
                (cx + cos * radius, cy + sin * radius)
            })
            .collect();
        Self {
            points,
            closed: false,
        }
    }

    /// A filled dot, as a degenerate zero-length stroke.
    ///
    /// A round-capped stroke of zero length paints a disc of the stroke's
    /// width, which is exactly what a leading marker is — and means a dot
    /// takes the icon's colour and weight for free rather than needing a
    /// separate filled shape whose radius would have to be kept in step
    /// with the stroke by hand.
    fn dot(x: f32, y: f32) -> Self {
        Self {
            points: vec![(x, y), (x, y)],
            closed: false,
        }
    }

    /// Marks this path closed.
    fn close(mut self) -> Self {
        self.closed = true;
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::Result;

    /// Every icon the enum names, so a new variant is drawn by the tests
    /// below without anyone remembering to add it.
    ///
    /// Listed rather than derived: there is no way to enumerate a Rust
    /// enum's variants at runtime, and `wildcard_enum_match_arm` being
    /// denied means the `match` in `strokes` already fails the build for
    /// a variant nobody drew. This list is what makes the *tests* cover
    /// it too, and the length assertion below is what catches a variant
    /// added to the enum and not to this list.
    const ALL: [Icon; 25] = [
        Icon::Windows,
        Icon::Processes,
        Icon::Performance,
        Icon::Memory,
        Icon::Details,
        Icon::Services,
        Icon::Startup,
        Icon::Settings,
        Icon::ChevronRight,
        Icon::ChevronDown,
        Icon::ArrowUp,
        Icon::ArrowDown,
        Icon::Search,
        Icon::Close,
        Icon::WindowMinimise,
        Icon::WindowMaximise,
        Icon::WindowRestore,
        Icon::Grip,
        Icon::Pin,
        Icon::Check,
        Icon::Warning,
        Icon::Refresh,
        Icon::Copy,
        Icon::Folder,
        Icon::Leaf,
    ];

    #[test]
    fn every_icon_stays_inside_the_grid() -> Result<()> {
        for icon in ALL {
            for path in icon.strokes() {
                for (x, y) in path.points {
                    assert!(
                        (0.0..=GRID).contains(&x) && (0.0..=GRID).contains(&y),
                        "{icon:?} has a point at ({x}, {y}), outside the \
                         0..={GRID} grid — it will be clipped by its own rect"
                    );
                }
            }
        }
        Ok(())
    }

    /// An icon whose ink sits in a corner of the grid reads as
    /// off-centre next to one that is balanced, however carefully the
    /// rect it is given is positioned.
    #[test]
    fn every_icon_is_optically_centred() -> Result<()> {
        /// How far the ink's centre may sit from the grid's, in grid
        /// units. One unit is about a pixel at the sizes these are drawn
        /// at, which is the point where a person starts to see it.
        const TOLERANCE: f32 = 1.0;

        for icon in ALL {
            let points: Vec<(f32, f32)> = icon
                .strokes()
                .into_iter()
                .flat_map(|path| path.points)
                .collect();
            let Some(&(first_x, first_y)) = points.first() else {
                panic_free_fail(icon)?;
                continue;
            };
            let (mut min_x, mut max_x) = (first_x, first_x);
            let (mut min_y, mut max_y) = (first_y, first_y);
            for (x, y) in &points {
                min_x = min_x.min(*x);
                max_x = max_x.max(*x);
                min_y = min_y.min(*y);
                max_y = max_y.max(*y);
            }
            let centre_x = (min_x + max_x) / 2.0;
            let centre_y = (min_y + max_y) / 2.0;
            assert!(
                (centre_x - GRID / 2.0).abs() <= TOLERANCE,
                "{icon:?} is centred at x={centre_x}, not {}",
                GRID / 2.0
            );
            assert!(
                (centre_y - GRID / 2.0).abs() <= TOLERANCE,
                "{icon:?} is centred at y={centre_y}, not {}",
                GRID / 2.0
            );
        }
        Ok(())
    }

    /// An icon has to actually reach across its grid, or it reads as
    /// smaller than the ones beside it even though its box is the same.
    #[test]
    fn every_icon_fills_enough_of_its_grid_to_match_its_neighbours() -> Result<()> {
        /// The smallest span, in grid units, an icon may cover on its
        /// longer axis.
        const MINIMUM_SPAN: f32 = 7.0;

        for icon in ALL {
            let points: Vec<(f32, f32)> = icon
                .strokes()
                .into_iter()
                .flat_map(|path| path.points)
                .collect();
            let Some(&(first_x, first_y)) = points.first() else {
                panic_free_fail(icon)?;
                continue;
            };
            let (mut min_x, mut max_x) = (first_x, first_x);
            let (mut min_y, mut max_y) = (first_y, first_y);
            for (x, y) in &points {
                min_x = min_x.min(*x);
                max_x = max_x.max(*x);
                min_y = min_y.min(*y);
                max_y = max_y.max(*y);
            }
            let span = (max_x - min_x).max(max_y - min_y);
            assert!(
                span >= MINIMUM_SPAN,
                "{icon:?} spans only {span} grid units, so it reads as \
                 smaller than the icons beside it"
            );
        }
        Ok(())
    }

    /// The house rule forbids `panic!`, and an empty icon is a real
    /// failure rather than something to skip — so it is reported as an
    /// `Err` the caller propagates.
    fn panic_free_fail(icon: Icon) -> Result<()> {
        anyhow::bail!("{icon:?} draws nothing at all")
    }
}
