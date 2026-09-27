//! The two rules xtask keeps: every file says what it is for, GitHub is
//! reached from one file, and its tests start the binary through the sandbox
//! helper.

use crate::support::*;

/// The xtask files that may start `gh`. Reading and writing GitHub is one
/// file's job, so `ready`, `conflicts` and `worktree` judge what `board`
/// read, and a fake `gh` (`xtask/tests/github.rs`) has one caller to answer.
pub(crate) const XTASK_GH_FILES: [Allowed; 1] = [(
    "xtask/src/board.rs",
    "the one file that reads and writes the board through gh",
    "github_worktree_add_claims_a_free_issue_and_writes_in_progress",
)];

/// Whether an xtask source starts `gh`.
pub(crate) fn starts_gh(source: &str) -> bool {
    source.contains("Command::new(\"gh\")")
}

/// xtask is a tool and not production code, so most of `src/`'s rules would
/// only be noise there: its errors are strings by design (every one ends in
/// a printed line), spawning cargo and git is its whole job, and it has no
/// layers. Two rules are worth having. Each file says what it is for, which
/// `cargo xtask map` prints for xtask too; and GitHub is reached from one
/// file, which keeps the decisions agents act on apart from the requests
/// that feed them.
#[test]
fn xtask_files_say_what_they_are_for_and_only_the_board_reaches_github() {
    let mut problems = Vec::new();
    for dir in ["xtask/src", "xtask/tests"] {
        for path in rust_files(&root().join(dir)) {
            let relative = path.strip_prefix(root()).unwrap().display().to_string();
            let content = std::fs::read_to_string(&path).unwrap();
            if !content.starts_with("//!") {
                problems.push(format!("{relative}: start the file with a `//!` line"));
            }
            if dir == "xtask/src" && starts_gh(&content) && !allows(&XTASK_GH_FILES, &relative) {
                problems.push(format!(
                    "{relative}: starts `gh`; read or write GitHub through xtask/src/board.rs"
                ));
            }
        }
    }
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}

/// A test that starts xtask with the person's HOME copies their kurama
/// configuration into a scratch worktree, and one with their CARGO_HOME can
/// install into their `~/.cargo/bin`. Both almost happened; the helper sets
/// them all, so it is the only place the binary may be started.
#[test]
fn xtask_tests_start_the_binary_only_through_the_sandbox_helper() {
    let helper = root().join("xtask/tests/support/mod.rs");
    let stray: Vec<String> = rust_files(&root().join("xtask/tests"))
        .into_iter()
        .filter(|path| *path != helper)
        .filter(|path| {
            std::fs::read_to_string(path)
                .unwrap()
                .contains("CARGO_BIN_EXE_xtask")
        })
        .map(|path| path.strip_prefix(root()).unwrap().display().to_string())
        .collect();
    assert!(
        stray.is_empty(),
        "start xtask through support::xtask, which keeps HOME and CARGO_HOME in the scratch directory:\n{}",
        stray.join("\n")
    );
}

/// A cargo xtask starts has to drop what `cargo run` set for xtask, or its
/// builds and the shell's rebuild each other's DuckDB; `xtask::cargo()` is the
/// one place that does.
#[test]
fn xtask_starts_cargo_only_through_its_cargo_helper() {
    let stray: Vec<String> = rust_files(&root().join("xtask/src"))
        .into_iter()
        .filter(|path| {
            let source = std::fs::read_to_string(path).unwrap();
            let starts = source.matches("Command::new(\"cargo\")").count();
            let allowed = usize::from(path.ends_with("xtask/src/main.rs"));
            starts > allowed
        })
        .map(|path| path.strip_prefix(root()).unwrap().display().to_string())
        .collect();
    assert!(
        stray.is_empty(),
        "start cargo through crate::cargo(), which drops the variables cargo run set for xtask:\n{}",
        stray.join("\n")
    );
}
