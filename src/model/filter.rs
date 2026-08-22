// ============================================================================
// Module:       model::filter
// Description:  The search box's query language and the matching it performs
//               over a snapshot's rows.
//
// Dependencies: std collections; super::ProcessRow
// ============================================================================

//! What the search box does.
//!
//! A bare word is a case-insensitive substring, matched across everything
//! that identifies a process — its image name, its description, its PID,
//! its owner, and its path. That covers the overwhelming majority of
//! searches, where the user knows roughly what the thing is called and
//! wants it out of four hundred rows.
//!
//! On top of that, a `field:value` term restricts the match to one field.
//! The reason is not power-user completeness, it is a real ambiguity: a
//! machine running a program called `system-monitor.exe` cannot be
//! searched for SYSTEM-owned processes at all, because every bare query
//! that finds one finds the other. `user:system` is unambiguous.
//!
//! Terms are separated by whitespace and combined with AND — `chrome
//! user:alice` means both. AND rather than OR because a filter exists to
//! narrow, and every added word making the result *larger* is the
//! opposite of what typing more of what you are looking for should do.
//!
//! A quoted term (`"visual studio"`) keeps its spaces, which is the only
//! way to search for a description that has one.
//!
//! A term containing nothing alphanumeric is dropped rather than searched
//! for. See [`Query::parse`] — it is what stops the list going blank
//! while a query is still being typed.

use super::ProcessRow;
use std::collections::HashSet;

/// A parsed query.
///
/// Parsing is separated from matching because a query is typed once per
/// keystroke and matched against every row: parsing four hundred times
/// per frame what could be parsed once is exactly the kind of per-frame
/// work `docs/PERFORMANCE.md` is about.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Query {
    /// The terms, all of which must match.
    terms: Vec<Term>,
}

/// One term of a query.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Term {
    /// Which field to look in, or `None` for all of them.
    field: Option<Field>,
    /// The needle, already lowercased so matching does not have to be.
    needle: String,
    /// Whether the term was prefixed with `-`, inverting it.
    negated: bool,
}

/// A field a term can be restricted to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Field {
    /// The image file name.
    Name,
    /// The version resource's description.
    Description,
    /// The process id, matched as text so `pid:44` finds 44 and 4400.
    Pid,
    /// The owning account.
    User,
    /// The full path to the executable.
    Path,
    /// The window title.
    Title,
}

impl Field {
    /// Maps a `field:` prefix to a field, case-insensitively.
    ///
    /// Returns `None` for an unrecognised prefix, and the caller then
    /// treats the whole `word:something` as a plain substring. That is
    /// deliberate: a path like `C:\Windows` contains a colon, and a
    /// search box that silently dropped everything before it — or worse,
    /// errored — would be baffling.
    fn parse(text: &str) -> Option<Self> {
        match text.to_ascii_lowercase().as_str() {
            "name" | "exe" => Some(Self::Name),
            "desc" | "description" => Some(Self::Description),
            "pid" => Some(Self::Pid),
            "user" | "owner" => Some(Self::User),
            "path" => Some(Self::Path),
            "title" | "window" => Some(Self::Title),
            _ => None,
        }
    }
}

impl Query {
    /// Parses the text of the search box.
    ///
    /// Never fails. A search box that rejects input while it is being
    /// typed is unusable — every query passes through a prefix that is
    /// not yet what the user means — so anything unparseable is treated
    /// as literal text to look for.
    #[must_use]
    pub fn parse(text: &str) -> Self {
        let mut terms = Vec::new();
        for word in split_terms(text) {
            let (negated, word) = match word.strip_prefix('-') {
                // A lone `-` is a literal, not an empty negation.
                Some(rest) if !rest.is_empty() => (true, rest.to_string()),
                Some(_) | None => (false, word),
            };
            let (field, needle) = match word.split_once(':') {
                Some((prefix, rest)) if !rest.is_empty() => match Field::parse(prefix) {
                    Some(field) => (Some(field), rest.to_string()),
                    // An unrecognised prefix — `C:\Windows` — is literal.
                    None => (None, word.clone()),
                },
                // `pid:` with nothing after it: a field term the user is
                // still typing. Searching for the literal text "pid:"
                // would blank the list between the colon and the first
                // character of the value.
                Some((prefix, _)) if Field::parse(prefix).is_some() => continue,
                Some(_) | None => (None, word.clone()),
            };
            // A term with nothing alphanumeric in it is not a search.
            // Every query passes through states like `-`, `:` and `"` on
            // its way to being typed, and treating those as a literal
            // substring blanks the whole list mid-keystroke — the list
            // vanishes and comes back as the next character lands, which
            // reads as the app losing its data. A path or a name with
            // punctuation in it still has letters, so `C:\Windows` and
            // `some-tool.exe` are unaffected.
            if needle.is_empty() || !needle.chars().any(char::is_alphanumeric) {
                continue;
            }
            terms.push(Term {
                field,
                needle: needle.to_lowercase(),
                negated,
            });
        }
        Self { terms }
    }

    /// Whether this query narrows anything.
    ///
    /// An empty query means no filter at all, which is a different state
    /// from "a filter that matches everything": the tree is drawn with
    /// the user's own expansion state rather than fully expanded, and no
    /// result count is shown.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.terms.is_empty()
    }

    /// Whether `row` satisfies every term.
    #[must_use]
    pub fn matches(&self, row: &ProcessRow) -> bool {
        self.terms.iter().all(|term| {
            let found = match term.field {
                Some(Field::Name) => contains(&row.name, &term.needle),
                Some(Field::Description) => contains(&row.description, &term.needle),
                Some(Field::Pid) => contains(&row.pid.to_string(), &term.needle),
                Some(Field::User) => contains(&row.user, &term.needle),
                Some(Field::Path) => contains(&path_text(row), &term.needle),
                Some(Field::Title) => row
                    .window_title
                    .as_deref()
                    .is_some_and(|title| contains(title, &term.needle)),
                None => {
                    contains(&row.name, &term.needle)
                        || contains(&row.description, &term.needle)
                        || contains(&row.pid.to_string(), &term.needle)
                        || contains(&row.user, &term.needle)
                        || contains(&path_text(row), &term.needle)
                        || row
                            .window_title
                            .as_deref()
                            .is_some_and(|title| contains(title, &term.needle))
                }
            };
            found != term.negated
        })
    }

    /// The indices of the rows this query matches.
    ///
    /// Returns a set rather than a `Vec` because [`super::tree::Layout`]
    /// tests membership once per row while flattening, and walks up the
    /// ancestor chain of each match — both are lookups, not iteration.
    #[must_use]
    pub fn select(&self, rows: &[ProcessRow]) -> HashSet<usize> {
        rows.iter()
            .enumerate()
            .filter(|(_, row)| self.matches(row))
            .map(|(index, _)| index)
            .collect()
    }
}

/// Case-insensitive substring test.
///
/// ASCII-lowercases the haystack per call rather than pre-folding it.
/// That is a per-row allocation, and it is the right trade here: the
/// alternative is a second lowercased copy of five string fields on every
/// row of every snapshot, which costs memory on all four hundred rows to
/// save work on the handful being filtered. Filtering only runs when the
/// user is typing, and only over rows that are already in cache.
fn contains(haystack: &str, needle_lowercase: &str) -> bool {
    if haystack.is_empty() {
        return false;
    }
    // The fast path: no uppercase in the haystack means no folding is
    // needed, which covers most paths and every PID.
    if !haystack.chars().any(char::is_uppercase) {
        return haystack.contains(needle_lowercase);
    }
    haystack.to_lowercase().contains(needle_lowercase)
}

/// A row's path as searchable text.
fn path_text(row: &ProcessRow) -> String {
    row.path
        .as_ref()
        .map(|path| path.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// Splits query text into terms, honouring double quotes.
///
/// An unterminated quote — which every quoted query passes through while
/// being typed — closes at the end of the input rather than discarding
/// the term.
fn split_terms(text: &str) -> Vec<String> {
    let mut terms = Vec::new();
    let mut current = String::new();
    let mut quoted = false;
    for ch in text.chars() {
        match ch {
            '"' => quoted = !quoted,
            c if c.is_whitespace() && !quoted => {
                if !current.is_empty() {
                    terms.push(std::mem::take(&mut current));
                }
            }
            c => current.push(c),
        }
    }
    if !current.is_empty() {
        terms.push(current);
    }
    terms
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn row(name: &str, pid: u32, user: &str) -> ProcessRow {
        ProcessRow {
            pid,
            name: name.to_string(),
            user: user.to_string(),
            ..ProcessRow::default()
        }
    }

    #[test]
    fn an_empty_query_is_not_a_filter() {
        assert!(Query::parse("").is_empty());
        assert!(Query::parse("   ").is_empty());
        assert!(
            !Query::parse("chrome").is_empty(),
            "a real query must report itself as one, or the tree is drawn \
             fully expanded for no reason"
        );
    }

    #[test]
    fn a_bare_word_searches_every_identifying_field() {
        let mut process = row("chrome.exe", 4242, "DESKTOP\\alice");
        process.description = "Google Chrome".to_string();
        process.path = Some(PathBuf::from("C:\\Program Files\\Google\\chrome.exe"));

        for query in ["chrome", "google", "4242", "alice", "program files"] {
            assert!(
                Query::parse(query).matches(&process),
                "{query:?} should have matched"
            );
        }
        assert!(!Query::parse("firefox").matches(&process));
    }

    #[test]
    fn matching_is_case_insensitive_in_both_directions() {
        let process = row("Chrome.exe", 1, "DESKTOP\\Alice");
        assert!(Query::parse("chrome").matches(&process), "query lowercase");
        assert!(Query::parse("CHROME").matches(&process), "query uppercase");
        assert!(Query::parse("ALICE").matches(&process));
    }

    #[test]
    fn a_field_term_resolves_the_ambiguity_it_exists_for() {
        // The case from the module docs: a program whose *name* contains
        // "system" cannot otherwise be told from processes *owned* by
        // SYSTEM.
        let program = row("system-monitor.exe", 1, "DESKTOP\\alice");
        let service = row("svchost.exe", 2, "NT AUTHORITY\\SYSTEM");

        let bare = Query::parse("system");
        assert!(
            bare.matches(&program) && bare.matches(&service),
            "ambiguous"
        );

        let by_user = Query::parse("user:system");
        assert!(
            !by_user.matches(&program),
            "the program is not owned by SYSTEM"
        );
        assert!(by_user.matches(&service));
    }

    #[test]
    fn terms_combine_with_and() {
        let alice = row("chrome.exe", 1, "DESKTOP\\alice");
        let bob = row("chrome.exe", 2, "DESKTOP\\bob");
        let query = Query::parse("chrome user:alice");
        assert!(query.matches(&alice));
        assert!(
            !query.matches(&bob),
            "adding a word must narrow the result, never widen it"
        );
    }

    #[test]
    fn a_negated_term_excludes() {
        let chrome = row("chrome.exe", 1, "DESKTOP\\alice");
        let firefox = row("firefox.exe", 2, "DESKTOP\\alice");
        let query = Query::parse("-chrome");
        assert!(!query.matches(&chrome));
        assert!(query.matches(&firefox));
    }

    #[test]
    fn a_path_containing_a_colon_is_not_mistaken_for_a_field_term() {
        // `C:\Windows` is the single most likely thing to be pasted into
        // this box, and a parser that dropped everything before the colon
        // would be baffling.
        let mut process = row("notepad.exe", 1, "DESKTOP\\alice");
        process.path = Some(PathBuf::from("C:\\Windows\\System32\\notepad.exe"));
        assert!(
            Query::parse("c:\\windows").matches(&process),
            "an unrecognised prefix must be treated as literal text"
        );
    }

    #[test]
    fn a_quoted_term_keeps_its_spaces() {
        let mut process = row("devenv.exe", 1, "DESKTOP\\alice");
        process.description = "Microsoft Visual Studio 2022".to_string();
        assert!(
            Query::parse("\"visual studio\"").matches(&process),
            "a quoted phrase is one term"
        );
        // Unquoted, the same text is two AND-ed terms — which also
        // matches here, but for a different reason. Check the phrase is
        // really being kept whole by using a phrase whose words appear
        // separately.
        let mut other = row("a.exe", 2, "DESKTOP\\alice");
        other.description = "Studio Visual".to_string();
        assert!(
            !Query::parse("\"visual studio\"").matches(&other),
            "a quoted phrase must not match its own words out of order"
        );
        assert!(
            Query::parse("visual studio").matches(&other),
            "unquoted, the same words are independent terms"
        );
    }

    #[test]
    fn an_unterminated_quote_still_searches() {
        // Every quoted query passes through this state while being typed.
        let mut process = row("devenv.exe", 1, "DESKTOP\\alice");
        process.description = "Visual Studio".to_string();
        assert!(
            Query::parse("\"visual stu").matches(&process),
            "a half-typed quoted term must still filter, not match nothing"
        );
    }

    #[test]
    fn a_query_that_is_only_punctuation_matches_everything() {
        // Another state every query passes through while being typed.
        let process = row("chrome.exe", 1, "DESKTOP\\alice");
        for text in ["-", ":", "\"", "pid:", "-\""] {
            let query = Query::parse(text);
            assert!(
                query.is_empty() || query.matches(&process),
                "{text:?} should not filter the whole list away mid-keystroke"
            );
        }
    }

    #[test]
    fn a_pid_term_matches_as_text_so_a_prefix_finds_a_process() {
        let process = row("a.exe", 4400, "DESKTOP\\alice");
        assert!(
            Query::parse("pid:44").matches(&process),
            "typing a PID a digit at a time must find it before the last one"
        );
        assert!(!Query::parse("pid:99").matches(&process));
    }

    #[test]
    fn a_window_title_is_searchable() {
        let mut process = row("explorer.exe", 1, "DESKTOP\\alice");
        process.window_title = Some("Downloads".to_string());
        assert!(Query::parse("downloads").matches(&process));
        assert!(Query::parse("title:down").matches(&process));

        let untitled = row("explorer.exe", 2, "DESKTOP\\alice");
        assert!(
            !Query::parse("title:down").matches(&untitled),
            "a row with no window must not match a title term"
        );
    }

    #[test]
    fn select_returns_the_matching_indices() {
        let rows = vec![
            row("chrome.exe", 1, "DESKTOP\\alice"),
            row("firefox.exe", 2, "DESKTOP\\alice"),
            row("chrome.exe", 3, "DESKTOP\\bob"),
        ];
        let selected = Query::parse("chrome").select(&rows);
        assert_eq!(selected, [0usize, 2].into_iter().collect::<HashSet<_>>());
    }

    #[test]
    fn parsing_is_done_once_rather_than_per_row() {
        // Not a behavioural test so much as a guard on the shape of the
        // API: `Query` has to be constructible independently of the rows
        // it will be matched against, or the per-keystroke cost becomes
        // per-keystroke-per-row.
        let query = Query::parse("chrome user:alice");
        let rows: Vec<ProcessRow> = (0..1_000)
            .map(|pid| row("chrome.exe", pid, "DESKTOP\\alice"))
            .collect();
        assert_eq!(query.select(&rows).len(), 1_000);
    }
}
