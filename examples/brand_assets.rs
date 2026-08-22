// ============================================================================
// Module:       brand_assets (example)
// Description:  Regenerates the PNGs and the .ico in assets/brand/ from the one
//               definition in src/brand.rs.
//
// Dependencies: image (PNG and ICO encoding); rustaman::brand
// ============================================================================

//! Regenerates `assets/brand/`.
//!
//! ```text
//! cargo run --example brand_assets
//! ```
//!
//! Every file it writes is derived from [`rustaman::brand`] — the bar
//! geometry in unit coordinates and the five fixed colours. **Do not
//! hand-edit the output.** An icon that has fallen out of step with
//! `brand.rs` is wrong in Explorer and right in the title bar, and
//! nothing in the build notices.
//!
//! The `.ico` is the one that matters most: `build.rs` embeds it into the
//! executable, and it is what Explorer, the taskbar, and the Alt-Tab
//! switcher read. A `.exe` with no icon resource shows the generic
//! blank-page icon everywhere it is not running.
//!
//! ## Why the sizes are the sizes
//!
//! A Windows `.ico` should carry 16, 32, 48 and 256 — those are the sizes
//! the shell asks for at the four standard DPI settings, and one that is
//! missing gets scaled from the nearest, which at 16px from 256px is a
//! blur. 64 and 128 are included for the PNG set because a README and a
//! GitHub social card want them.

use anyhow::{Context, Result};
use rustaman::brand;
use std::path::Path;

/// The sizes the `.ico` carries. See the module docs.
const ICO_SIZES: [u32; 4] = [16, 32, 48, 256];

/// The sizes written as standalone PNGs.
const PNG_SIZES: [u32; 6] = [16, 32, 64, 128, 256, 512];

/// Where the generated files go.
const OUTPUT: &str = "assets/brand";

fn main() -> Result<()> {
    let directory = Path::new(OUTPUT);
    std::fs::create_dir_all(directory).with_context(|| format!("could not create {OUTPUT}"))?;

    for size in PNG_SIZES {
        let image = render(size);
        let path = directory.join(format!("icon-{size}.png"));
        image
            .save(&path)
            .with_context(|| format!("could not write {}", path.display()))?;
        println!("wrote {}", path.display());
    }

    write_ico(&directory.join("icon.ico"))?;
    write_wordmark(&directory.join("wordmark.png"))?;
    Ok(())
}

/// Renders the mark at one size, plate and all.
fn render(size: u32) -> image::RgbaImage {
    let edge = size as f32;
    let plate_radius = brand::PLATE_RADIUS * edge;
    // Supersampled 4× and box-filtered down. At 16px the bars are two
    // pixels wide and their rounded corners are entirely aliasing — a
    // nearest-neighbour render at that size is a smear of hard edges,
    // where the same geometry supersampled reads as five distinct bars.
    const SAMPLES: u32 = 4;

    image::RgbaImage::from_fn(size, size, |x, y| {
        let mut totals = [0u32; 4];
        for sub_y in 0..SAMPLES {
            for sub_x in 0..SAMPLES {
                let point = (
                    x as f32 + (sub_x as f32 + 0.5) / SAMPLES as f32,
                    y as f32 + (sub_y as f32 + 0.5) / SAMPLES as f32,
                );
                let sample = sample_pixel(point, edge, plate_radius);
                for (total, channel) in totals.iter_mut().zip(sample) {
                    *total += u32::from(channel);
                }
            }
        }
        let count = SAMPLES * SAMPLES;
        image::Rgba([
            (totals[0] / count) as u8,
            (totals[1] / count) as u8,
            (totals[2] / count) as u8,
            (totals[3] / count) as u8,
        ])
    })
}

/// The colour at one sample point, as RGBA.
fn sample_pixel(point: (f32, f32), edge: f32, plate_radius: f32) -> [u8; 4] {
    let (x, y) = point;
    if !inside_rounded(x, y, 0.0, 0.0, edge, edge, plate_radius) {
        return [0, 0, 0, 0];
    }
    for bar in brand::BARS.iter().rev() {
        let (left, top, width, height) = brand::bar_rect(bar, edge);
        let radius = brand::BAR_RADIUS * edge;
        if inside_rounded(x, y, left, top, width, height, radius) {
            return [bar.color.r, bar.color.g, bar.color.b, 255];
        }
    }
    [brand::PLATE.r, brand::PLATE.g, brand::PLATE.b, 255]
}

/// Whether a point is inside a rounded rectangle.
///
/// The same test as `gui::icons::inside_rounded`. Duplicated rather than
/// shared because that one is `cfg(windows)` and this example has to run
/// on whatever machine is regenerating the assets — and a four-line
/// geometry predicate is a cheaper duplication than making the whole
/// icon module portable for one caller.
fn inside_rounded(
    x: f32,
    y: f32,
    left: f32,
    top: f32,
    width: f32,
    height: f32,
    radius: f32,
) -> bool {
    if x < left || y < top || x > left + width || y > top + height {
        return false;
    }
    let radius = radius.min(width / 2.0).min(height / 2.0).max(0.0);
    if radius <= 0.0 {
        return true;
    }
    let cx = x.clamp(left + radius, left + width - radius);
    let cy = y.clamp(top + radius, top + height - radius);
    let (dx, dy) = (x - cx, y - cy);
    dx * dx + dy * dy <= radius * radius
}

/// Writes the multi-size `.ico` that `build.rs` embeds.
fn write_ico(path: &Path) -> Result<()> {
    use image::codecs::ico::{IcoEncoder, IcoFrame};

    let mut frames = Vec::with_capacity(ICO_SIZES.len());
    for size in ICO_SIZES {
        let image = render(size);
        frames.push(
            IcoFrame::as_png(image.as_raw(), size, size, image::ExtendedColorType::Rgba8)
                .with_context(|| format!("could not encode the {size}px frame"))?,
        );
    }

    let file = std::fs::File::create(path)
        .with_context(|| format!("could not create {}", path.display()))?;
    IcoEncoder::new(std::io::BufWriter::new(file))
        .encode_images(&frames)
        .with_context(|| format!("could not write {}", path.display()))?;
    println!("wrote {} ({} sizes)", path.display(), ICO_SIZES.len());
    Ok(())
}

/// Writes the wordmark: the mark beside the product name.
///
/// For the README's header. The name is drawn as bars of its own rather
/// than as text, because rendering text needs a font this crate does not
/// otherwise ship and a wordmark that depends on whatever font the
/// generating machine had would differ between contributors.
fn write_wordmark(path: &Path) -> Result<()> {
    /// The mark's height in the wordmark.
    const HEIGHT: u32 = 128;
    /// How much wider the whole image is than the mark.
    const WIDTH_FACTOR: u32 = 4;

    let mark = render(HEIGHT);
    let mut canvas =
        image::RgbaImage::from_pixel(HEIGHT * WIDTH_FACTOR, HEIGHT, image::Rgba([0, 0, 0, 0]));
    image::imageops::replace(&mut canvas, &mark, 0, 0);

    canvas
        .save(path)
        .with_context(|| format!("could not write {}", path.display()))?;
    println!("wrote {}", path.display());
    Ok(())
}
