//! The static checks every xtask command that checks the tree runs first:
//! the generated case tests, formatting, clippy, dead code and unused dependencies. They answer in a minute or two
//! against a warm `target/`, where the tests, scenarios and mutants after them
//! take many more, so a lint is never what the last step of a long run finds.
//!
//! Which commands run them is `main.rs`'s `COMMANDS` table, not each command:
//! a command added without a row there does not dispatch, so none can forget.

use std::sync::OnceLock;

use crate::{Gate, cargo, cargo_plugin, plugin_installed, root};

/// `cargo fmt`, reading only.
pub const FORMAT: [&str; 4] = ["fmt", "--all", "--", "--check"];

/// `cargo clippy` over every target with the test fakes, warnings as errors:
/// the build the tests after it compile.
pub const CLIPPY: [&str; 9] = [
    "clippy",
    "--locked",
    "--workspace",
    "--all-targets",
    "--features",
    "test-fakes",
    "--",
    "-D",
    "warnings",
];

/// The library alone with `--cfg dead_code_audit`, which `src/lib.rs` reads to
/// compile the modules it publishes for the tests as crate-private: public,
/// every `pub` item in them counts as used and rustc names none. Only the
/// kurama crate is rebuilt, since the flags after `--` reach no dependency.
pub const DEAD_CODE: [&str; 12] = [
    "rustc",
    "--locked",
    "--lib",
    "--profile",
    "check",
    "--features",
    "test-fakes",
    "--",
    "--cfg",
    "dead_code_audit",
    "-D",
    "warnings",
];

/// What the prelude found, for the commands that write a gate summary.
static RESULTS: OnceLock<Vec<Gate>> = OnceLock::new();

/// The generated cases, format, clippy, dead code, then unused dependencies, stopping at the first
/// failure: nothing after a static failure is worth its minutes.
pub fn run() -> Result<(), String> {
    eprintln!("==> generated cases: {}", crate::cases::GENERATED);
    crate::cases::check_generated()?;
    let mut gates = vec![passed("generated-cases", "matches tests/cases/")];
    for (name, args) in [
        ("format", &FORMAT[..]),
        ("clippy", &CLIPPY[..]),
        ("dead-code", &DEAD_CODE[..]),
    ] {
        eprintln!("==> {name}: cargo {}", args.join(" "));
        let ok = cargo()
            .args(args)
            .current_dir(root())
            .status()
            .map_err(|error| format!("cargo: {error}"))?
            .success();
        if !ok {
            return Err(failure(name));
        }
        gates.push(passed(name, "passed"));
    }
    gates.push(unused_dependencies()?);
    let _ = RESULTS.set(gates);
    Ok(())
}

/// The gates `run` passed, empty when it did not run in this process.
pub fn results() -> Vec<Gate> {
    RESULTS.get().cloned().unwrap_or_default()
}

fn unused_dependencies() -> Result<Gate, String> {
    if !plugin_installed("cargo-machete") {
        eprintln!("==> unused-deps: skipped, cargo-machete is not installed");
        return Ok(passed(
            "unused-deps",
            "skipped: cargo-machete not installed (cargo install --locked cargo-machete)",
        ));
    }
    eprintln!("==> unused-deps: cargo machete");
    let ok = cargo_plugin("cargo-machete")
        .current_dir(root())
        .status()
        .map_err(|error| format!("cargo machete: {error}"))?
        .success();
    if !ok {
        return Err(failure("unused-deps"));
    }
    Ok(passed("unused-deps", "no unused dependencies"))
}

fn passed(name: &str, detail: &str) -> Gate {
    Gate {
        name: name.into(),
        ok: true,
        detail: detail.into(),
    }
}

fn failure(name: &str) -> String {
    let fix = match name {
        "format" => "cargo fmt --all fixes it",
        "clippy" => "see the warnings above",
        "dead-code" => "remove what rustc names above, or mark an item only tests use #[cfg(test)]",
        _ => "remove what cargo machete names above",
    };
    format!("static check `{name}` failed ({fix}); nothing after the static checks ran")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The same clippy `check` has always run: every target, the fakes on,
    /// warnings as errors. A narrower one would pass what `check` refuses.
    #[test]
    fn clippy_covers_every_target_with_the_fakes_and_denies_warnings() {
        assert_eq!(
            CLIPPY.join(" "),
            "clippy --locked --workspace --all-targets --features test-fakes -- -D warnings"
        );
        assert_eq!(FORMAT.join(" "), "fmt --all -- --check");
    }

    /// The library alone, with the modules `lib.rs` publishes for the tests
    /// compiled crate-private, so rustc's dead-code lint reaches their `pub`
    /// items. Without the cfg every one of them counts as used.
    #[test]
    fn dead_code_compiles_the_library_with_its_modules_private() {
        assert_eq!(
            DEAD_CODE.join(" "),
            "rustc --locked --lib --profile check --features test-fakes -- --cfg dead_code_audit -D warnings"
        );
    }

    #[test]
    fn a_failure_says_how_to_fix_it_and_that_nothing_else_ran() {
        assert!(failure("format").contains("cargo fmt --all fixes it"));
        assert!(
            failure("clippy").contains("static check `clippy` failed (see the warnings above)")
        );
        assert!(failure("unused-deps").contains("cargo machete"));
        assert!(failure("dead-code").contains("#[cfg(test)]"));
        assert!(failure("clippy").ends_with("nothing after the static checks ran"));
    }
}
