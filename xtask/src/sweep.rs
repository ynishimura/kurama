//! Cap `target/` after `cargo xtask check`, taking the artifacts that come
//! back cheaply before the ones that do not. `cargo xtask sweep` is the same
//! thing on its own, and `mise run sweep` runs that.

use std::path::{Path, PathBuf};

use crate::worktree::{directory_size, human_size};
use crate::{cargo_plugin, plugin_installed, root};

/// One debug generation is 7GB of it (5.5GB of dependencies and a 1.3GB DuckDB
/// build), and `install-signed` leaves a release one beside it, so a cap of 8GB
/// was under what a single gate needs and evicted part of it every run.
pub const TARGET_MAX_SIZE: &str = "12GB";

/// The same cap in the kilobytes `du -sk` reports.
const TARGET_MAX_KILOBYTES: u64 = 12 * 1024 * 1024;

/// `cargo xtask sweep`: the cap on its own, for a clone that grew between
/// gates.
pub fn sweep(args: &[String]) -> Result<(), String> {
    if let Some(argument) = args.first() {
        return Err(format!("sweep takes no arguments, got `{argument}`"));
    }
    println!("{}", after_check());
    Ok(())
}

/// Best-effort cap after a gate that compiled. A failed step is a warning and
/// never a failed check.
pub fn after_check() -> String {
    let target = root().join("target");
    let before = directory_size(&target);
    if !over_cap(before) {
        return format!("{} of {TARGET_MAX_SIZE}", human_size(before));
    }
    // The incremental caches are the cheapest gigabytes in the folder: they
    // belong to the workspace crates alone and the next compile writes them
    // again. `cargo sweep --maxsize` takes files in modification order
    // instead, and the oldest are the C++ crates nothing ever changes --
    // DuckDB, aws-lc-sys -- so the cap used to evict the five minutes of
    // build the next gate needs most.
    eprintln!("==> sweep: dropping the incremental caches");
    let dropped = drop_incremental(&target);
    let left = before.saturating_sub(dropped);
    if !over_cap(left) {
        return format!(
            "{} of incremental caches dropped; {} of {TARGET_MAX_SIZE}",
            human_size(dropped),
            human_size(left)
        );
    }
    let swept = sweep_to_cap();
    format!(
        "{} of incremental caches dropped, then {} left over the cap: {swept}",
        human_size(dropped),
        human_size(left)
    )
}

fn over_cap(kilobytes: u64) -> bool {
    kilobytes > TARGET_MAX_KILOBYTES
}

/// The incremental caches of both profiles, whether or not they exist.
fn incremental_dirs(target: &Path) -> [PathBuf; 2] {
    [
        target.join("debug").join("incremental"),
        target.join("release").join("incremental"),
    ]
}

/// Removes them, returning the kilobytes that freed.
fn drop_incremental(target: &Path) -> u64 {
    let mut freed = 0;
    for dir in incremental_dirs(target) {
        if !dir.exists() {
            continue;
        }
        let size = directory_size(&dir);
        match std::fs::remove_dir_all(&dir) {
            Ok(()) => freed += size,
            Err(error) => eprintln!("==> sweep: {} left as-is: {error}", dir.display()),
        }
    }
    freed
}

/// Modification-order eviction, which is the last resort: it can take the
/// DuckDB build with it. Missing cargo-sweep is not a failed check.
fn sweep_to_cap() -> String {
    if !plugin_installed("cargo-sweep") {
        return "cargo-sweep not installed (mise run setup)".into();
    }
    eprintln!("==> sweep: cargo sweep --maxsize {TARGET_MAX_SIZE}");
    match cargo_plugin("cargo-sweep")
        .args(sweep_args())
        .current_dir(root())
        .status()
    {
        Ok(status) if status.success() => "oldest artifacts dropped too".into(),
        Ok(_) => "cargo sweep failed; the rest was left as-is".into(),
        Err(error) => format!("cargo sweep: {error}"),
    }
}

fn sweep_args() -> [&'static str; 3] {
    ["sweep", "--maxsize", TARGET_MAX_SIZE]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_fallback_eviction_is_given_the_same_cap() {
        assert_eq!(sweep_args(), ["sweep", "--maxsize", TARGET_MAX_SIZE]);
    }

    /// The printed cap and the cap the folder is measured against are the same
    /// number: they were two constants and could disagree.
    #[test]
    fn the_printed_cap_is_the_cap_that_is_measured() {
        let gigabytes: u64 = TARGET_MAX_SIZE
            .trim_end_matches("GB")
            .parse()
            .expect("the cap is stated in whole gigabytes");
        assert_eq!(TARGET_MAX_KILOBYTES, gigabytes * 1024 * 1024);
    }

    #[test]
    fn a_folder_at_the_cap_is_not_over_it() {
        assert!(!over_cap(TARGET_MAX_KILOBYTES));
        assert!(over_cap(TARGET_MAX_KILOBYTES + 1));
        assert!(!over_cap(0));
    }

    /// Both profiles, because `install-signed` leaves a release generation
    /// behind and its incremental cache is as disposable as the debug one.
    #[test]
    fn the_incremental_caches_of_both_profiles_are_what_is_dropped_first() {
        let dirs = incremental_dirs(Path::new("/clone/target"));
        assert_eq!(
            dirs,
            [
                PathBuf::from("/clone/target/debug/incremental"),
                PathBuf::from("/clone/target/release/incremental"),
            ]
        );
    }

    #[test]
    fn sweep_takes_no_arguments() {
        let error = sweep(&["--all".to_string()]).expect_err("an argument is a usage error");
        assert!(error.contains("--all"), "{error}");
    }

    #[test]
    fn leftover_runs_mise_sweep_after_check() {
        let leftover = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/../lefthook.yml"));
        assert!(
            leftover.contains("mise run sweep"),
            "pre-push must run the same sweep as mise.toml"
        );
        let check = leftover.find("cargo xtask check").expect("check");
        let sweep = leftover.find("mise run sweep").expect("sweep");
        assert!(check < sweep, "sweep must run after check, not in parallel");
    }

    /// One implementation of the cap: `mise run sweep` runs the xtask command
    /// rather than `cargo sweep` directly, which would evict in modification
    /// order and take the DuckDB build.
    #[test]
    fn mise_sweep_runs_the_xtask_command() {
        let mise = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/../mise.toml"));
        assert!(
            mise.contains("cargo xtask sweep"),
            "mise.toml sweep must run the one implementation of the cap"
        );
        assert!(
            !mise.contains("cargo sweep --maxsize"),
            "mise.toml must not evict in modification order on its own"
        );
    }
}
