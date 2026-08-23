// ============================================================================
// Module:       gui::font
// Description:  Installs a UI typeface by family name from the machine's own
//               font directories, falling back to the bundled faces.
//
// Dependencies: egui (FontDefinitions); std::fs for the font directories
// ============================================================================

//! The UI typeface.
//!
//! egui ships Ubuntu Sans and Hack compiled into the binary, and until
//! this module those were the only faces the app had. This lets the
//! window take a face that is *installed on the machine* instead, named
//! in the config.
//!
//! ## Why by name from the system, rather than a file in `assets/`
//!
//! Because of what the face in question is. The app is asked to render
//! in Product Sans, which is Google's corporate typeface: it has never
//! been released for third-party use and cannot be licensed, so a copy
//! of it committed to `assets/` would be redistributing it, in the
//! repository and in every binary built from it.
//!
//! Reading one that is already installed is a different act entirely.
//! The font is on the machine because whoever owns the machine put it
//! there, this reads it the way any other application on that machine
//! reads it, and nothing about it enters the repository — the config
//! holds a *name*, which is not the font.
//!
//! It is also the more useful design regardless of the licence. A
//! bundled face is one face; this is whichever one the person running
//! the app prefers, changed without a rebuild.
//!
//! ## The bundled faces stay underneath
//!
//! The named face is *prepended* to the proportional family rather than
//! replacing it, and this is load-bearing rather than tidy. Product Sans
//! Regular is 41 KB — it covers Latin and very little else. Replacing
//! the family with it would silently drop Greek, Cyrillic and every
//! glyph in a process name that came from outside Latin-1, which on a
//! process list is not hypothetical: a path or a window title is
//! whatever the program that owns it decided to call itself.
//!
//! Prepending means the named face is asked first and the bundled one
//! answers for everything it does not have, which is exactly the
//! behaviour a fallback chain is for.
//!
//! ## Failure is silence, on purpose
//!
//! A missing font is not an error worth showing anybody. The name in
//! the config may be for a face on another machine, or spelled the way
//! a font menu spells it rather than the way the file is named. In
//! every one of those cases the right outcome is the app opening in the
//! bundled face, which is what it did before this module existed.

use std::path::{Path, PathBuf};

/// The name the loaded face is registered under.
const KEY: &str = "configured-ui-face";

/// Installs `family` as the app's proportional face, if it can be found.
///
/// Returns the path it loaded, so a caller can say which face it got.
/// `None` means the family was not found, or its file could not be
/// read — in both cases the context is left alone and keeps the
/// bundled faces.
pub fn install(ctx: &egui::Context, family: &str) -> Option<PathBuf> {
    let path = locate(family)?;
    let bytes = std::fs::read(&path).ok()?;

    let mut fonts = egui::FontDefinitions::default();
    fonts.font_data.insert(
        KEY.to_owned(),
        std::sync::Arc::new(egui::FontData::from_owned(bytes)),
    );
    // Ahead of the bundled faces rather than instead of them; see the
    // module docs on why that is not a detail.
    fonts
        .families
        .entry(egui::FontFamily::Proportional)
        .or_default()
        .insert(0, KEY.to_owned());
    ctx.set_fonts(fonts);
    Some(path)
}

/// The directories a font can be installed into on Windows.
///
/// The per-user one first. Installing a font without administrator
/// rights puts it there, which is how most fonts on a work machine get
/// installed, and a per-user copy is the one the person chose most
/// recently.
fn directories() -> Vec<PathBuf> {
    let mut directories = Vec::new();
    if let Some(local) = std::env::var_os("LOCALAPPDATA") {
        directories.push(
            Path::new(&local)
                .join("Microsoft")
                .join("Windows")
                .join("Fonts"),
        );
    }
    if let Some(root) = std::env::var_os("SystemRoot") {
        directories.push(Path::new(&root).join("Fonts"));
    }
    directories
}

/// Finds the file holding `family`'s regular weight.
fn locate(family: &str) -> Option<PathBuf> {
    for directory in directories() {
        let Ok(entries) = std::fs::read_dir(&directory) else {
            continue;
        };
        let files: Vec<PathBuf> = entries
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| is_a_font_file(path))
            .collect();
        if let Some(found) = pick(&files, family) {
            return Some(found.clone());
        }
    }
    None
}

/// Whether a path names a font this can actually load.
///
/// `.fon` and `.pfb` are also in the font directory and are neither
/// TrueType nor OpenType; handing one to the font stack is a load
/// failure rather than a fallback, so they are filtered here.
fn is_a_font_file(path: &Path) -> bool {
    let Some(extension) = path.extension().and_then(|extension| extension.to_str()) else {
        return false;
    };
    let extension = extension.to_ascii_lowercase();
    extension == "ttf" || extension == "otf" || extension == "ttc"
}

/// Picks the file for `family`'s regular weight out of `files`.
///
/// Split from the directory walk so the choosing can be tested without
/// a font directory to walk — the part worth testing is which of six
/// candidates wins, and that has nothing to do with the filesystem.
///
/// Windows names a font file after its full face name, so a family
/// installs as several files that all begin with the family name:
/// `Product Sans Regular.ttf`, `Product Sans Bold.ttf`, `Product Sans
/// Italic.ttf`, `Product Sans Bold Italic.ttf`. Matching the family
/// alone therefore matches all four, and the order they come back from
/// the directory is not defined — so the app's weight would be whatever
/// the filesystem felt like that morning.
///
/// Hence the preference order: the exact name first, for a family whose
/// single file is named after it; then `<family> Regular`; then any
/// remaining match that is neither bold nor italic. A face with no
/// upright regular at all is not worth guessing at.
fn pick<'a>(files: &'a [PathBuf], family: &str) -> Option<&'a PathBuf> {
    let wanted = family.trim().to_ascii_lowercase();
    if wanted.is_empty() {
        return None;
    }

    let stem = |path: &PathBuf| -> Option<String> {
        Some(path.file_stem()?.to_str()?.to_ascii_lowercase())
    };

    let exact = files
        .iter()
        .find(|path| stem(path).is_some_and(|stem| stem == wanted));
    if exact.is_some() {
        return exact;
    }

    let regular = format!("{wanted} regular");
    let named = files
        .iter()
        .find(|path| stem(path).is_some_and(|stem| stem == regular));
    if named.is_some() {
        return named;
    }

    files.iter().find(|path| {
        stem(path).is_some_and(|stem| {
            stem.starts_with(&wanted) && !stem.contains("bold") && !stem.contains("italic")
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn files(names: &[&str]) -> Vec<PathBuf> {
        names.iter().map(PathBuf::from).collect()
    }

    #[test]
    fn the_regular_weight_wins_over_the_bold_and_the_italic() -> anyhow::Result<()> {
        // The failure this exists for is not a crash. All four files
        // match the family, `read_dir` has no defined order, and the
        // app would have opened in bold on some machines and upright on
        // others with nothing to explain the difference.
        let candidates = files(&[
            "Product Sans Bold Italic.ttf",
            "Product Sans Bold.ttf",
            "Product Sans Italic.ttf",
            "Product Sans Regular.ttf",
        ]);
        let chosen = pick(&candidates, "Product Sans")
            .ok_or_else(|| anyhow::anyhow!("no face chosen from four candidates"))?;
        assert_eq!(
            chosen,
            &PathBuf::from("Product Sans Regular.ttf"),
            "the regular weight is the one a UI is set in"
        );
        Ok(())
    }

    #[test]
    fn a_family_whose_file_is_named_after_it_is_matched_exactly() -> anyhow::Result<()> {
        // Not every family spells the weight out. A variable font
        // usually ships as one file named for the family alone.
        let candidates = files(&["Outfit.ttf", "Outfit Bold.ttf"]);
        let chosen = pick(&candidates, "Outfit")
            .ok_or_else(|| anyhow::anyhow!("the exact name did not match"))?;
        assert_eq!(chosen, &PathBuf::from("Outfit.ttf"));
        Ok(())
    }

    #[test]
    fn the_match_is_not_case_sensitive() -> anyhow::Result<()> {
        // The name comes from a config file a person typed.
        let candidates = files(&["Product Sans Regular.ttf"]);
        assert!(
            pick(&candidates, "product sans").is_some(),
            "a name typed in lower case has to find the same file"
        );
        Ok(())
    }

    #[test]
    fn a_family_that_is_not_installed_finds_nothing() {
        // The path that has to stay quiet: the app opens in the bundled
        // face rather than reporting anything.
        let candidates = files(&["Product Sans Regular.ttf", "Arial.ttf"]);
        assert!(pick(&candidates, "Helvetica Neue").is_none());
    }

    #[test]
    fn a_family_with_only_a_bold_face_is_not_set_in_it() {
        // Better the bundled regular than this family's bold: a whole
        // UI in bold reads as a rendering fault, and the fallback is a
        // face that was designed to be read at 13.5 points.
        let candidates = files(&["Product Sans Bold.ttf"]);
        assert!(pick(&candidates, "Product Sans").is_none());
    }

    #[test]
    fn an_empty_name_matches_nothing_rather_than_everything() {
        // `starts_with("")` is true of every string, so an unset or
        // blank config value would otherwise install the first font in
        // the directory — alphabetically, on Windows, Arial.
        let candidates = files(&["Arial.ttf", "Product Sans Regular.ttf"]);
        assert!(pick(&candidates, "").is_none());
        assert!(pick(&candidates, "   ").is_none());
    }

    #[test]
    fn a_bitmap_font_is_not_offered_to_the_font_stack() {
        // `.fon` sits in the same directory and is neither TrueType nor
        // OpenType; loading one fails rather than falling back.
        assert!(is_a_font_file(Path::new("Product Sans Regular.ttf")));
        assert!(is_a_font_file(Path::new("Outfit.OTF")));
        assert!(!is_a_font_file(Path::new("vgasys.fon")));
        assert!(!is_a_font_file(Path::new("marlett")));
    }
}
