// ============================================================================
// Module:       treemap
// Description:  Squarified treemap layout — turns a list of weights into a
//               set of rectangles filling a box, as squarely as it can.
//
// Dependencies: std only. Deliberately portable: this is geometry, and it is
//               tested on the non-Windows job like the rest of the model.
// ============================================================================

//! Laying a set of weights out as rectangles that fill a box.
//!
//! The Memory view draws every process as a tile whose **area** is the
//! memory it is holding. Area is the only encoding that lets a reader
//! compare four hundred things at once and see the answer without
//! reading a single number — a bar chart of four hundred bars is four
//! hundred rows of scrolling, and a pie chart of four hundred slices is
//! a circle with a fringe.
//!
//! ## Why squarified rather than the obvious slice-and-dice
//!
//! The naive layout — take the box, cut a strip off it per item, repeat
//! — is four lines long and produces slivers. A process holding a
//! thousandth of the total gets a rectangle a thousandth as wide and
//! full height: a hairline nobody can point at, click, or read a label
//! in, and whose area the eye cannot judge at all because judging area
//! means comparing two dimensions and one of them has collapsed.
//!
//! The squarified algorithm (Bruls, Huizing and van Wijk, 2000) instead
//! fills a row at a time, adding items to the current row while doing so
//! *improves* the worst aspect ratio in it and starting a new row when
//! it would not. The result is tiles close to square, which are the ones
//! whose areas can actually be compared, and which are big enough to
//! hold text and take a click.
//!
//! ## The weights are sorted, and equal weights keep their order
//!
//! Descending, because the algorithm only produces good aspect ratios if
//! the large items are placed first. [`layout`] sorts internally rather
//! than trusting the caller, and the sort is *stable*, so a caller whose
//! weights are equal gets its own order preserved rather than an
//! arbitrary one that changes between samples.

/// One laid-out tile: where it is, and which input it came from.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Tile {
    /// The index of this tile's weight in the slice passed to [`layout`].
    ///
    /// Carried because [`layout`] sorts, so the output order is not the
    /// input order and a caller needs to get back to its own item.
    pub index: usize,
    /// Distance from the box's left edge.
    pub x: f32,
    /// Distance from the box's top edge.
    pub y: f32,
    /// Width, never negative.
    pub width: f32,
    /// Height, never negative.
    pub height: f32,
}

impl Tile {
    /// The tile's area.
    #[must_use]
    pub fn area(&self) -> f32 {
        self.width * self.height
    }

    /// How far from square this tile is: 1.0 is square, larger is worse.
    ///
    /// The quantity the algorithm minimises. Infinite for a tile with no
    /// extent, which is the degenerate case the caller should not draw.
    #[must_use]
    pub fn aspect(&self) -> f32 {
        let long = self.width.max(self.height);
        let short = self.width.min(self.height);
        if short <= 0.0 {
            f32::INFINITY
        } else {
            long / short
        }
    }
}

/// Lays `weights` out as tiles filling a `width` by `height` box.
///
/// Weights that are zero, negative or not finite are dropped: they have
/// no area to draw and including them would put zero-width tiles in the
/// output for callers to filter out again.
///
/// Returns an empty list for an empty box or no usable weights.
#[must_use]
pub fn layout(weights: &[f64], width: f32, height: f32) -> Vec<Tile> {
    if width <= 0.0 || height <= 0.0 {
        return Vec::new();
    }

    let mut items: Vec<(usize, f64)> = weights
        .iter()
        .copied()
        .enumerate()
        .filter(|(_, weight)| weight.is_finite() && *weight > 0.0)
        .collect();
    if items.is_empty() {
        return Vec::new();
    }
    // Descending, stably — see the module docs on both halves of that.
    items.sort_by(|a, b| b.1.total_cmp(&a.1));

    let total: f64 = items.iter().map(|(_, weight)| weight).sum();
    if total <= 0.0 {
        return Vec::new();
    }

    // Weights are rescaled into area up front, so the row arithmetic
    // below is in the same units as the box and never has to carry the
    // ratio around.
    let scale = f64::from(width) * f64::from(height) / total;
    let areas: Vec<(usize, f64)> = items
        .into_iter()
        .map(|(index, weight)| (index, weight * scale))
        .collect();

    let mut tiles = Vec::with_capacity(areas.len());
    let mut free = Free {
        x: 0.0,
        y: 0.0,
        width,
        height,
    };
    let mut row: Vec<(usize, f64)> = Vec::new();

    for entry in areas {
        // The row is only extended while doing so makes its worst tile
        // *more* square. The moment it would make it worse, the row is
        // committed and this item starts the next one.
        if row.is_empty() || improves(&row, entry.1, free.short_side()) {
            row.push(entry);
            continue;
        }
        free = commit(&mut tiles, &row, free);
        row.clear();
        row.push(entry);
    }
    if !row.is_empty() {
        commit(&mut tiles, &row, free);
    }
    tiles
}

/// The rectangle still to be filled.
#[derive(Clone, Copy, Debug)]
struct Free {
    x: f32,
    y: f32,
    width: f32,
    height: f32,
}

impl Free {
    /// The shorter side, which is the one a row is laid along.
    ///
    /// Laying along the short side is what keeps tiles square: a row
    /// spanning the long side of a wide box is already a wide strip
    /// before anything is put in it.
    fn short_side(&self) -> f32 {
        self.width.min(self.height)
    }
}

/// Whether adding `area` to `row` improves its worst aspect ratio.
fn improves(row: &[(usize, f64)], area: f64, side: f32) -> bool {
    worst(row, side) >= worst_with(row, area, side)
}

/// The worst aspect ratio in `row`, laid along a side of length `side`.
fn worst(row: &[(usize, f64)], side: f32) -> f64 {
    ratios(row.iter().map(|(_, area)| *area), side)
}

/// The same, with one more area folded in.
fn worst_with(row: &[(usize, f64)], area: f64, side: f32) -> f64 {
    ratios(
        row.iter()
            .map(|(_, item)| *item)
            .chain(std::iter::once(area)),
        side,
    )
}

/// The worst aspect ratio a row of these areas would have.
///
/// The published form: with `s` the row's total area, `w` the side it is
/// laid along, and `min`/`max` its extremes, the worst ratio is
/// `max(w²·max/s², s²/(w²·min))`.
fn ratios(areas: impl Iterator<Item = f64>, side: f32) -> f64 {
    let side = f64::from(side);
    let mut sum = 0.0;
    let mut low = f64::INFINITY;
    let mut high: f64 = 0.0;
    for area in areas {
        sum += area;
        low = low.min(area);
        high = high.max(area);
    }
    if sum <= 0.0 || side <= 0.0 || low <= 0.0 {
        return f64::INFINITY;
    }
    let squared = side * side;
    ((squared * high) / (sum * sum)).max((sum * sum) / (squared * low))
}

/// Places a finished row and returns what is left of the box.
fn commit(tiles: &mut Vec<Tile>, row: &[(usize, f64)], free: Free) -> Free {
    let total: f64 = row.iter().map(|(_, area)| area).sum();
    if total <= 0.0 {
        return free;
    }

    // Along the short side, so the strip's own thickness is the one that
    // adapts — see `Free::short_side`.
    let vertical = free.width <= free.height;
    let thickness = if vertical {
        (total / f64::from(free.width)) as f32
    } else {
        (total / f64::from(free.height)) as f32
    };

    let mut offset = 0.0f32;
    for (index, area) in row {
        let extent = if vertical {
            (area / total) as f32 * free.width
        } else {
            (area / total) as f32 * free.height
        };
        let tile = if vertical {
            Tile {
                index: *index,
                x: free.x + offset,
                y: free.y,
                width: extent,
                height: thickness,
            }
        } else {
            Tile {
                index: *index,
                x: free.x,
                y: free.y + offset,
                width: thickness,
                height: extent,
            }
        };
        tiles.push(tile);
        offset += extent;
    }

    if vertical {
        Free {
            x: free.x,
            y: free.y + thickness,
            width: free.width,
            height: (free.height - thickness).max(0.0),
        }
    } else {
        Free {
            x: free.x + thickness,
            y: free.y,
            width: (free.width - thickness).max(0.0),
            height: free.height,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_box_or_no_weights_lays_nothing_out() {
        assert!(layout(&[1.0, 2.0], 0.0, 100.0).is_empty());
        assert!(layout(&[1.0, 2.0], 100.0, 0.0).is_empty());
        assert!(layout(&[], 100.0, 100.0).is_empty());
    }

    #[test]
    fn weights_that_cannot_be_drawn_are_dropped_rather_than_laid_out_flat() {
        // A zero or negative weight has no area. Left in, it would come
        // back as a zero-width tile for the caller to filter out again —
        // and a caller that forgot would draw an invisible click target.
        let tiles = layout(&[4.0, 0.0, -1.0, f64::NAN, 4.0], 100.0, 100.0);
        assert_eq!(tiles.len(), 2, "only the two usable weights are tiles");
        assert!(tiles
            .iter()
            .all(|tile| tile.width > 0.0 && tile.height > 0.0));
    }

    #[test]
    fn every_tile_carries_the_index_of_the_weight_it_came_from() {
        // `layout` sorts, so the output order is not the input order.
        // Without the index a caller cannot tell which of its own items
        // a tile is — and the tile would be unlabellable and unclickable.
        let tiles = layout(&[1.0, 9.0, 3.0], 100.0, 100.0);
        let mut indices: Vec<usize> = tiles.iter().map(|tile| tile.index).collect();
        indices.sort_unstable();
        assert_eq!(indices, vec![0, 1, 2]);

        let biggest = tiles
            .iter()
            .max_by(|a, b| a.area().total_cmp(&b.area()))
            .map(|tile| tile.index);
        assert_eq!(biggest, Some(1), "the largest weight gets the largest tile");
    }

    #[test]
    fn the_tiles_areas_are_proportional_to_their_weights() {
        // The whole point of the encoding: twice the memory is twice the
        // area, or the picture is decoration rather than a measurement.
        let weights = [50.0, 25.0, 15.0, 10.0];
        let tiles = layout(&weights, 400.0, 300.0);
        let box_area = 400.0 * 300.0;
        let total: f64 = weights.iter().sum();

        for tile in &tiles {
            let expected = (weights[tile.index] / total) as f32 * box_area;
            let error = (tile.area() - expected).abs() / expected;
            assert!(
                error < 0.02,
                "tile {} has area {} but its weight is {} of {}, which is \
                 {expected}",
                tile.index,
                tile.area(),
                weights[tile.index],
                total
            );
        }
    }

    #[test]
    fn the_tiles_fill_the_box_without_overlapping() {
        // Two properties at once, because they fail together: a layout
        // that leaves a gap is one that has put the missing area
        // somewhere it should not be.
        let weights: Vec<f64> = (1..=40).map(|n| f64::from(n) * 3.0).collect();
        let (width, height) = (640.0f32, 400.0f32);
        let tiles = layout(&weights, width, height);

        let covered: f32 = tiles.iter().map(Tile::area).sum();
        let error = (covered - width * height).abs() / (width * height);
        assert!(
            error < 0.01,
            "the tiles cover {covered} of {}",
            width * height
        );

        for (i, a) in tiles.iter().enumerate() {
            assert!(
                a.x >= -0.01
                    && a.y >= -0.01
                    && a.x + a.width <= width + 0.01
                    && a.y + a.height <= height + 0.01,
                "tile {i} at ({}, {}) {}x{} leaves the box",
                a.x,
                a.y,
                a.width,
                a.height
            );
            for b in tiles.iter().skip(i + 1) {
                let overlap = (a.x + a.width).min(b.x + b.width) - a.x.max(b.x) > 0.01
                    && (a.y + a.height).min(b.y + b.height) - a.y.max(b.y) > 0.01;
                assert!(!overlap, "tiles {i} and another overlap");
            }
        }
    }

    #[test]
    fn the_tiles_are_squarer_than_slicing_the_box_into_strips_would_be() {
        // The reason this is the squarified algorithm and not the four
        // lines that also "work". Slice-and-dice gives every item the
        // full height of the box, so a small item is a hairline: an
        // aspect ratio in the hundreds, an area the eye cannot judge and
        // a target the pointer cannot hit.
        let weights: Vec<f64> = (1..=60).map(f64::from).collect();
        let (width, height) = (800.0f32, 500.0f32);
        let tiles = layout(&weights, width, height);

        let worst = tiles.iter().map(Tile::aspect).fold(0.0f32, f32::max);

        // What slicing would have produced for the smallest item.
        let total: f64 = weights.iter().sum();
        let sliver = (weights[0] / total) as f32 * width;
        let sliced = height / sliver;

        assert!(
            worst < 12.0,
            "the worst tile is {worst}:1, which is not a shape anyone can \
             compare the area of"
        );
        assert!(
            worst < sliced / 4.0,
            "squarifying gave {worst}:1 where slicing gives {sliced}:1 — \
             not enough better to be worth the algorithm"
        );
    }

    #[test]
    fn one_weight_fills_the_whole_box() {
        let tiles = layout(&[7.0], 200.0, 100.0);
        assert_eq!(tiles.len(), 1);
        let tile = tiles[0];
        assert!((tile.width - 200.0).abs() < 0.01);
        assert!((tile.height - 100.0).abs() < 0.01);
    }

    #[test]
    fn equal_weights_keep_the_order_they_were_given_in() {
        // A process list re-sorted every second must not make the
        // treemap reshuffle: two processes holding the same memory would
        // otherwise swap tiles between samples, and a tile that moves is
        // a tile that cannot be clicked.
        let tiles = layout(&[5.0, 5.0, 5.0, 5.0], 300.0, 200.0);
        let order: Vec<usize> = tiles.iter().map(|tile| tile.index).collect();
        assert_eq!(order, vec![0, 1, 2, 3]);
    }
}
