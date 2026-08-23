// ============================================================================
// Module:       unsafe_check
// Description:  Enforces the repository's safe-leaf boundary for unsafe Rust.
//
// Dependencies: anyhow, std::fs (walks Rust sources during tests)
// ============================================================================

//! Structural checks for the unsafe boundary documented in `AGENTS.md`.
//!
//! Compiler lints verify local mechanics such as safety comments. This
//! test enforces repository-level rules the compiler cannot express:
//! unsafe stays under `src/win`, tests never need it, public unsafe APIs
//! are forbidden, and one function cannot accumulate multiple unsafe
//! operations.

#[cfg(test)]
mod tests {
    use anyhow::Result;
    use std::collections::HashMap;
    use std::fs;
    use std::path::{Path, PathBuf};

    /// Finds every Rust source below `root`.
    fn rust_sources(root: &Path) -> Result<Vec<PathBuf>> {
        let mut pending = vec![root.to_path_buf()];
        let mut found = Vec::new();
        while let Some(directory) = pending.pop() {
            for entry in fs::read_dir(directory)? {
                let path = entry?.path();
                if path.is_dir() {
                    pending.push(path);
                } else if path.extension().is_some_and(|extension| extension == "rs") {
                    found.push(path);
                }
            }
        }
        found.sort();
        Ok(found)
    }

    /// Whether a non-comment source line opens an unsafe boundary.
    fn is_boundary(line: &str) -> bool {
        let keyword = ["un", "safe"].concat();
        let code = line.split("//").next().unwrap_or_default();
        code.contains(&format!("{keyword} {{"))
            || code.contains(&format!("{keyword} fn "))
            || code.contains(&format!("{keyword} impl "))
            || code.contains(&format!("{keyword} extern "))
    }

    /// The nearest named function above a line, used as its leaf owner.
    fn enclosing_function(lines: &[&str], line: usize) -> Option<usize> {
        (0..=line).rev().find(|index| {
            let trimmed = lines[*index].trim_start();
            trimmed.starts_with("fn ")
                || trimmed.starts_with("pub fn ")
                || trimmed.starts_with("pub(crate) fn ")
                || trimmed.starts_with("unsafe fn ")
                || trimmed.starts_with("unsafe extern ")
        })
    }

    #[test]
    fn unsafe_code_stays_in_one_operation_leaf_wrappers() -> Result<()> {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        let mut failures = Vec::new();
        let mut per_function: HashMap<(String, usize), usize> = HashMap::new();
        let keyword = ["un", "safe"].concat();

        for path in rust_sources(&root.join("src"))? {
            let relative = path
                .strip_prefix(root)?
                .to_string_lossy()
                .replace('\\', "/");
            if relative == "src/unsafe_check.rs" {
                continue;
            }
            let text = fs::read_to_string(path)?;
            let lines: Vec<&str> = text.lines().collect();
            let test_start = lines.iter().position(|line| line.trim() == "mod tests {");

            for (index, line) in lines.iter().enumerate() {
                if !is_boundary(line) {
                    continue;
                }
                let location = format!("{relative}:{}", index + 1);
                if !relative.starts_with("src/win/") {
                    failures.push(format!("{location}: unsafe code must live under src/win"));
                }
                if test_start.is_some_and(|start| index > start) {
                    failures.push(format!("{location}: test code must use safe wrappers"));
                }
                if line.contains(&format!("pub {keyword} fn")) {
                    failures.push(format!("{location}: public unsafe APIs are forbidden"));
                }
                if let Some(function) = enclosing_function(&lines, index) {
                    *per_function
                        .entry((relative.clone(), function))
                        .or_default() += 1;
                }
            }
        }

        for ((file, line), count) in per_function {
            if count > 1 {
                failures.push(format!(
                    "{file}:{}: leaf wrapper contains {count} unsafe operations",
                    line + 1
                ));
            }
        }

        assert!(
            failures.is_empty(),
            "unsafe boundary violation(s):\n{}",
            failures.join("\n")
        );
        Ok(())
    }
}
