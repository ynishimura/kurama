//! `kurama obsidian`, pure: a vault path normalized and held to `[obsidian] allow_paths`, the Obsidian CLI argv of each subcommand, and the CLI's answer filtered and shaped.
//!
//! The CLI can write to the vault and run JavaScript. What keeps kurama from
//! ever asking it to is that every argv is built here: the subcommand after
//! `vault=` is one of [`Subcommand`]'s constants, and a caller's value is only
//! ever the value of one `key=value` element, so a query such as
//! `x eval code=...` stays the value of `query=`.

use serde::Serialize;
use serde_json::Value;

/// The read-only subcommands kurama runs, and the only ones.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Subcommand {
    Search,
    Read,
    Files,
}

impl Subcommand {
    /// The CLI's own name of the subcommand.
    pub fn cli_name(self) -> &'static str {
        match self {
            Self::Search => "search:context",
            Self::Read => "read",
            Self::Files => "files",
        }
    }
}

/// A path or folder kurama does not hand to the CLI.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PathRefused {
    #[error("{path:?} is not a vault path: it is absolute, holds `..` or a backslash")]
    NotRelative { path: String, allowed: Vec<String> },
    #[error("{path:?} is outside [obsidian] allow_paths")]
    Outside { path: String, allowed: Vec<String> },
}

impl PathRefused {
    /// What may be read instead.
    pub fn hint(&self) -> String {
        let (Self::NotRelative { allowed, .. } | Self::Outside { allowed, .. }) = self;
        format!(
            "name a path relative to the vault root inside one of: {}",
            allowed.join(", ")
        )
    }
}

/// One `allow_paths` entry as kurama checks against it: relative, no `..`,
/// ending in `/`; or why it cannot be one.
pub fn allowed_folder(entry: &str) -> Result<String, String> {
    match normalize(entry) {
        Some(folder) if !folder.is_empty() => Ok(format!("{folder}/")),
        _ => Err(format!(
            "allow_paths entry {entry:?} is not a folder relative to the vault root without `..`"
        )),
    }
}

/// `path` relative to the vault root with `.` and empty segments dropped, or
/// `None` when it is absolute, climbs with `..` or holds a backslash.
fn normalize(path: &str) -> Option<String> {
    if path.starts_with('/') || path.contains('\\') {
        return None;
    }
    let mut segments = Vec::new();
    for segment in path.split('/') {
        match segment {
            "" | "." => {}
            ".." => return None,
            segment => segments.push(segment),
        }
    }
    Some(segments.join("/"))
}

fn inside(path: &str, allowed: &[String]) -> bool {
    allowed
        .iter()
        .any(|folder| path.starts_with(folder.as_str()))
}

/// The note `path` names, when it lies inside an allowed folder.
pub fn allowed_note(path: &str, allowed: &[String]) -> Result<String, PathRefused> {
    let normalized = normalize(path).ok_or_else(|| PathRefused::NotRelative {
        path: path.to_owned(),
        allowed: allowed.to_vec(),
    })?;
    if !inside(&normalized, allowed) {
        return Err(PathRefused::Outside {
            path: path.to_owned(),
            allowed: allowed.to_vec(),
        });
    }
    Ok(normalized)
}

/// The folder `folder` names, without its trailing `/`, when it is an
/// allowed folder or lies inside one.
pub fn allowed_subfolder(folder: &str, allowed: &[String]) -> Result<String, PathRefused> {
    let normalized = normalize(folder).ok_or_else(|| PathRefused::NotRelative {
        path: folder.to_owned(),
        allowed: allowed.to_vec(),
    })?;
    if normalized.is_empty() || !inside(&format!("{normalized}/"), allowed) {
        return Err(PathRefused::Outside {
            path: folder.to_owned(),
            allowed: allowed.to_vec(),
        });
    }
    Ok(normalized)
}

/// The folders a run reads: the one asked for, checked, or every allowed
/// folder.
pub fn folders_to_read(
    folder: Option<&str>,
    allowed: &[String],
) -> Result<Vec<String>, PathRefused> {
    match folder {
        Some(folder) => Ok(vec![allowed_subfolder(folder, allowed)?]),
        None => Ok(allowed
            .iter()
            .map(|folder| folder.trim_end_matches('/').to_owned())
            .collect()),
    }
}

fn argv(vault: &str, subcommand: Subcommand, values: &[(&str, &str)]) -> Vec<String> {
    let mut argv = vec![format!("vault={vault}"), subcommand.cli_name().to_owned()];
    argv.extend(values.iter().map(|(key, value)| format!("{key}={value}")));
    argv
}

/// `search:context` in one folder, as JSON.
pub fn search_argv(vault: &str, query: &str, folder: &str, limit: usize) -> Vec<String> {
    let limit = limit.to_string();
    argv(
        vault,
        Subcommand::Search,
        &[
            ("query", query),
            ("path", folder),
            ("limit", &limit),
            ("format", "json"),
        ],
    )
}

/// `read` of one note.
pub fn read_argv(vault: &str, path: &str) -> Vec<String> {
    argv(vault, Subcommand::Read, &[("path", path)])
}

/// `files` of one folder.
pub fn files_argv(vault: &str, folder: &str) -> Vec<String> {
    argv(vault, Subcommand::Files, &[("folder", folder)])
}

/// The CLI's own failure, when its output is one: it exits 0 and prints
/// `Error: ...` or `Vault not found.` on stdout instead of the answer.
pub fn cli_failure(stdout: &str) -> Option<&str> {
    let text = stdout.trim_end();
    (!text.contains('\n') && (text.starts_with("Error: ") || text == "Vault not found."))
        .then_some(text)
}

/// One line of a note that matched.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Match {
    pub path: String,
    pub line: u64,
    pub text: String,
}

/// The matches of one `search:context ... format=json` answer that lie
/// inside an allowed folder; `No matches found.` is none.
pub fn search_matches(stdout: &str, allowed: &[String]) -> Result<Vec<Match>, String> {
    if stdout.trim() == "No matches found." {
        return Ok(Vec::new());
    }
    let files: Vec<Value> = serde_json::from_str(stdout).map_err(|error| {
        format!("the CLI's search answer is not the JSON it documents: {error}")
    })?;
    let mut matches = Vec::new();
    for file in &files {
        let Some(path) = file["file"].as_str().filter(|path| inside(path, allowed)) else {
            continue;
        };
        for found in file["matches"].as_array().into_iter().flatten() {
            matches.push(Match {
                path: path.to_owned(),
                line: found["line"].as_u64().unwrap_or(0),
                text: found["text"].as_str().unwrap_or("").to_owned(),
            });
        }
    }
    Ok(matches)
}

/// The paths of one `files` answer that lie inside an allowed folder.
pub fn listed_files(stdout: &str, allowed: &[String]) -> Vec<String> {
    stdout
        .lines()
        .filter(|path| inside(path, allowed))
        .map(str::to_owned)
        .collect()
}

/// `content` cut to at most `max_bytes` on a character boundary, and whether
/// it was cut.
pub fn cut_note(content: &str, max_bytes: usize) -> (&str, bool) {
    if content.len() <= max_bytes {
        return (content, false);
    }
    let end = (0..=max_bytes)
        .rev()
        .find(|end| content.is_char_boundary(*end))
        .unwrap_or(0);
    (&content[..end], true)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn allowed() -> Vec<String> {
        vec!["Wiki/".to_owned(), "Daily/2026/".to_owned()]
    }

    #[test]
    fn an_allow_paths_entry_is_a_relative_folder_ending_in_a_slash() {
        assert_eq!(allowed_folder("Wiki").unwrap(), "Wiki/");
        assert_eq!(allowed_folder("./Daily/2026/").unwrap(), "Daily/2026/");
        for entry in ["/Wiki/", "Wiki/../Private/", "..", "", "/", "a\\b"] {
            assert!(allowed_folder(entry).is_err(), "{entry:?}");
        }
    }

    #[test]
    fn a_note_inside_an_allowed_folder_is_read_by_its_normalized_path() {
        assert_eq!(
            allowed_note("Wiki/./kurama.md", &allowed()).unwrap(),
            "Wiki/kurama.md"
        );
        assert_eq!(
            allowed_note("Daily/2026/10-05.md", &allowed()).unwrap(),
            "Daily/2026/10-05.md"
        );
    }

    #[rstest::rstest]
    #[case("Wiki/../Private/secret.md", true)]
    #[case("/Users/me/vault/Wiki/a.md", true)]
    #[case("Wiki\\a.md", true)]
    #[case("Private/secret.md", false)]
    #[case("Wikipedia/a.md", false)]
    #[case("Daily/2025/a.md", false)]
    #[case("Wiki", false)]
    fn a_path_outside_allow_paths_is_refused(#[case] path: &str, #[case] not_relative: bool) {
        let refused = allowed_note(path, &allowed()).unwrap_err();
        assert_eq!(
            matches!(refused, PathRefused::NotRelative { .. }),
            not_relative,
            "{refused:?}"
        );
        assert_eq!(
            refused.hint(),
            "name a path relative to the vault root inside one of: Wiki/, Daily/2026/"
        );
    }

    #[test]
    fn a_folder_is_an_allowed_one_or_inside_one() {
        assert_eq!(allowed_subfolder("Wiki", &allowed()).unwrap(), "Wiki");
        assert_eq!(
            allowed_subfolder("Wiki/sub/", &allowed()).unwrap(),
            "Wiki/sub"
        );
        for folder in ["Daily", "", ".", "Wiki/..", "Private"] {
            assert!(allowed_subfolder(folder, &allowed()).is_err(), "{folder:?}");
        }
        assert_eq!(
            folders_to_read(None, &allowed()).unwrap(),
            ["Wiki", "Daily/2026"]
        );
        assert_eq!(
            folders_to_read(Some("Wiki/sub"), &allowed()).unwrap(),
            ["Wiki/sub"]
        );
    }

    #[test]
    fn a_value_is_only_ever_the_value_of_one_element() {
        assert_eq!(
            search_argv("brain", "x eval code=app.vault", "Wiki", 5),
            [
                "vault=brain",
                "search:context",
                "query=x eval code=app.vault",
                "path=Wiki",
                "limit=5",
                "format=json"
            ]
        );
        assert_eq!(
            read_argv("brain", "Wiki/a.md"),
            ["vault=brain", "read", "path=Wiki/a.md"]
        );
        assert_eq!(
            files_argv("brain", "Wiki"),
            ["vault=brain", "files", "folder=Wiki"]
        );
    }

    #[test]
    fn the_cli_reports_its_failures_on_stdout() {
        assert_eq!(
            cli_failure("Error: File \"Wiki/x.md\" not found.\n"),
            Some("Error: File \"Wiki/x.md\" not found.")
        );
        assert_eq!(cli_failure("Vault not found.\n"), Some("Vault not found."));
        assert_eq!(cli_failure("Error: in a note\nsecond line\n"), None);
        assert_eq!(cli_failure("# a note\n"), None);
    }

    #[test]
    fn only_matches_inside_allowed_folders_are_kept() {
        let answer = r#"[
            {"file":"Wiki/kurama.md","matches":[{"line":3,"text":"kurama reads"},{"line":9,"text":"kurama again"}]},
            {"file":"Private/secret.md","matches":[{"line":1,"text":"kurama secret"}]}
        ]"#;
        assert_eq!(
            search_matches(answer, &allowed()).unwrap(),
            [
                Match {
                    path: "Wiki/kurama.md".into(),
                    line: 3,
                    text: "kurama reads".into()
                },
                Match {
                    path: "Wiki/kurama.md".into(),
                    line: 9,
                    text: "kurama again".into()
                },
            ]
        );
        assert_eq!(
            search_matches("No matches found.\n", &allowed()).unwrap(),
            []
        );
        assert!(search_matches("not json", &allowed()).is_err());
        assert_eq!(
            listed_files("Wiki/a.md\nPrivate/b.md\nDaily/2026/c.md\n", &allowed()),
            ["Wiki/a.md", "Daily/2026/c.md"]
        );
    }

    #[test]
    fn a_long_note_is_cut_on_a_character_boundary() {
        assert_eq!(cut_note("short", 10), ("short", false));
        assert_eq!(cut_note("exactly", 7), ("exactly", false));
        // "あ" is three bytes: four bytes hold one of them, not one and a third.
        assert_eq!(cut_note("ああ", 4), ("あ", true));
        assert_eq!(cut_note("abcdef", 3), ("abc", true));
    }
}
