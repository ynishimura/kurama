//! After `install-signed`: add the new build's `cdhash` to the partition list of every keychain entry kurama reads, so the build reads them without a dialog.
//!
//! The designated requirement of a stably signed build survives rebuilds, but
//! the partition list does not: for a signature without an Apple team ID,
//! macOS records "Always Allow" as `cdhash:<hash>`, a value every build
//! changes. A run without a terminal (an agent, cron) cannot show the dialog
//! and is denied (`error -25293`). The install is the moment a person is at
//! the terminal, so it asks for the login keychain password once and extends
//! each entry's list there, keeping the hashes already on it, because a
//! `--as NAME` build installed beside the daily one still reads the entries.

use std::io::{IsTerminal, Write};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// The services kurama writes itself; the service account entry is named by
/// the configuration.
const KURAMA_SERVICES: &[&str] = &["kurama-token", "kurama-session"];

/// One generic password of a service kurama reads, with its partition list.
#[derive(Debug, PartialEq)]
struct Entry {
    service: String,
    account: String,
    partitions: Vec<String>,
}

pub fn grant_installed_build(installed: &Path) -> Result<(), String> {
    let details = Command::new("codesign")
        .args(["-dvvv"])
        .arg(installed)
        .output()
        .map_err(|e| format!("codesign -dvvv: {e}"))?;
    let cdhash = read_cdhash(&String::from_utf8_lossy(&details.stderr))
        .ok_or_else(|| format!("codesign -dvvv {} printed no CDHash", installed.display()))?;

    let mut services: Vec<String> = KURAMA_SERVICES.iter().map(|s| s.to_string()).collect();
    if let Some(service) = std::fs::read_to_string(config_path()?)
        .ok()
        .and_then(|config| service_account_keychain(&config))
    {
        services.push(service);
    }
    // `-a` adds the access control lists, where the partition list is;
    // without `-d` no secret is printed.
    let dump = Command::new("security")
        .args(["dump-keychain", "-a"])
        .output()
        .map_err(|e| format!("security dump-keychain: {e}"))?;
    let pending: Vec<(Entry, String)> =
        parse_entries(&String::from_utf8_lossy(&dump.stdout), &services)
            .into_iter()
            .filter_map(|entry| {
                let list = extended_partitions(&entry.partitions, &cdhash)?;
                Some((entry, list))
            })
            .collect();
    if pending.is_empty() {
        eprintln!("==> keychain: every entry kurama reads already allows cdhash:{cdhash}");
        return Ok(());
    }
    if !std::io::stdin().is_terminal() {
        eprintln!(
            "==> keychain: {} entries do not allow cdhash:{cdhash} yet; without a terminal \
             nobody can type the keychain password, so the first read of each will ask instead",
            pending.len()
        );
        return Ok(());
    }

    let password = read_keychain_password()?;
    for (entry, list) in &pending {
        eprintln!(
            "==> keychain: allow cdhash:{cdhash} on {} / {}",
            entry.service, entry.account
        );
        set_partitions(entry, list, &password)?;
    }
    Ok(())
}

/// The file kurama itself reads: `KURAMA_CONFIG_PATH`, else the default.
fn config_path() -> Result<PathBuf, String> {
    if let Some(path) = std::env::var_os("KURAMA_CONFIG_PATH") {
        return Ok(PathBuf::from(path));
    }
    let home = std::env::var_os("HOME").ok_or("HOME is not set")?;
    Ok(PathBuf::from(home).join(".config/kurama/config.toml"))
}

/// Echo off while the password is typed; `stty` works on the terminal it
/// inherits as stdin.
fn read_keychain_password() -> Result<String, String> {
    eprint!("login keychain password (to let this build read kurama's entries): ");
    std::io::stderr().flush().map_err(|e| e.to_string())?;
    let stty = |mode: &str| {
        Command::new("stty")
            .arg(mode)
            .stdin(Stdio::inherit())
            .status()
    };
    stty("-echo").map_err(|e| format!("stty -echo: {e}"))?;
    let mut line = String::new();
    let read = std::io::stdin().read_line(&mut line);
    let _ = stty("echo");
    eprintln!();
    read.map_err(|e| format!("read the password: {e}"))?;
    Ok(line.trim_end_matches(['\r', '\n']).to_owned())
}

/// `security` reads the password from its controlling terminal when it has
/// one, whatever its stdin holds, so it is started in a session of its own:
/// with no terminal it reads the password from stdin, and the password never
/// appears in an argument list.
fn set_partitions(entry: &Entry, list: &str, password: &str) -> Result<(), String> {
    let mut child = partition_command(entry, list)
        .spawn()
        .map_err(|e| format!("security set-generic-password-partition-list: {e}"))?;
    if let Some(mut stdin) = child.stdin.take() {
        writeln!(stdin, "{password}").map_err(|e| format!("send the password: {e}"))?;
    }
    let output = child
        .wait_with_output()
        .map_err(|e| format!("security set-generic-password-partition-list: {e}"))?;
    if output.status.success() {
        return Ok(());
    }
    Err(format!(
        "the binary is installed, but {} / {} still does not allow it: {}",
        entry.service,
        entry.account,
        String::from_utf8_lossy(&output.stderr).trim()
    ))
}

fn partition_command(entry: &Entry, list: &str) -> Command {
    let mut command = Command::new("security");
    command
        .args(["set-generic-password-partition-list", "-s", &entry.service])
        .args(["-a", &entry.account, "-S", list])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    // SAFETY: `setsid` is async-signal-safe and touches no memory of the
    // parent; the child is a fresh process that only calls exec after it.
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() == -1 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    command
}

fn read_cdhash(codesign_details: &str) -> Option<String> {
    codesign_details
        .lines()
        .find_map(|line| line.strip_prefix("CDHash="))
        .map(str::to_owned)
}

fn service_account_keychain(config: &str) -> Option<String> {
    let config: toml::Table = config.parse().ok()?;
    config
        .get("onepassword")?
        .get("service_account_keychain")?
        .as_str()
        .map(str::to_owned)
}

/// Each item of `security dump-keychain` starts with a `keychain:` line; its
/// partition list is the `description:` after `authorizations (1): partition_id`.
fn parse_entries(dump: &str, services: &[String]) -> Vec<Entry> {
    dump.split("\nkeychain: ")
        .filter_map(|item| {
            let service = quoted_attribute(item, "\"svce\"<blob>=")?;
            if !services.contains(&service) {
                return None;
            }
            let account = quoted_attribute(item, "\"acct\"<blob>=")?;
            let mut lines = item.lines();
            lines.find(|line| line.trim_end().ends_with("partition_id"))?;
            let partitions = lines
                .find_map(|line| line.trim().strip_prefix("description: "))?
                .split(", ")
                .map(str::to_owned)
                .collect();
            Some(Entry {
                service,
                account,
                partitions,
            })
        })
        .collect()
}

fn quoted_attribute(item: &str, key: &str) -> Option<String> {
    item.lines().find_map(|line| {
        let value = line.trim().strip_prefix(key)?;
        Some(value.strip_prefix('"')?.strip_suffix('"')?.to_owned())
    })
}

fn extended_partitions(partitions: &[String], cdhash: &str) -> Option<String> {
    let id = format!("cdhash:{cdhash}");
    if partitions.contains(&id) {
        return None;
    }
    Some(
        partitions
            .iter()
            .cloned()
            .chain(std::iter::once(id))
            .collect::<Vec<_>>()
            .join(","),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `codesign -dvvv` prints `CandidateCDHash` lines before `CDHash=`; only
    /// the latter is the 40-digit value a partition list holds.
    #[test]
    fn the_cdhash_is_read_from_the_codesign_details() {
        let details = "Executable=/x/kurama\n\
                       CandidateCDHash sha256=9593e253b46f8dfcf4c9114a14862ef97aff9716\n\
                       CDHash=9593e253b46f8dfcf4c9114a14862ef97aff9716\n\
                       Authority=kurama-dev\n";

        assert_eq!(
            read_cdhash(details).as_deref(),
            Some("9593e253b46f8dfcf4c9114a14862ef97aff9716")
        );
        assert_eq!(read_cdhash("Authority=kurama-dev\n"), None);
    }

    #[test]
    fn the_service_account_entry_comes_from_the_onepassword_section() {
        let config = "[onepassword]\nvault = \"Agent\"\nservice_account_keychain = \"OP_TOKEN\"\n";

        assert_eq!(
            service_account_keychain(config).as_deref(),
            Some("OP_TOKEN")
        );
        assert_eq!(
            service_account_keychain("[onepassword]\nvault = \"A\"\n"),
            None
        );
        assert_eq!(service_account_keychain("not toml ["), None);
    }

    const DUMP: &str = r#"keychain: "/Users/u/Library/Keychains/login.keychain-db"
version: 512
class: "genp"
attributes:
    "acct"<blob>="github"
    "svce"<blob>="kurama-token"
access: 3 entries
    entry 0:
        authorizations (1): partition_id
        don't-require-password
        description: cdhash:b4a96b123ec04b484a2648f6c6f69b6ba67c281f
        applications: <null>
keychain: "/Users/u/Library/Keychains/login.keychain-db"
version: 512
class: "genp"
attributes:
    "acct"<blob>="u"
    "svce"<blob>="OP_TOKEN"
access: 1 entries
    entry 0:
        authorizations (1): partition_id
        don't-require-password
        description: apple-tool:, cdhash:ff4a76fb89315de6f99c565139a2617093457b76
        applications: <null>
keychain: "/Users/u/Library/Keychains/login.keychain-db"
version: 512
class: "genp"
attributes:
    "acct"<blob>="AiApiKeys"
    "svce"<blob>="dev.warp.Warp-Stable"
access: 1 entries
    entry 0:
        authorizations (1): partition_id
        don't-require-password
        description: teamid:2BBY89MBSN
        applications: <null>
keychain: "/Users/u/Library/Keychains/login.keychain-db"
version: 512
class: "genp"
attributes:
    "acct"<blob>="old"
    "svce"<blob>="kurama-session"
access: 1 entries
    entry 0:
        authorizations (6): decrypt derive export_clear export_wrapped mac sign
        description: kurama-session
        applications: <null>
"#;

    /// Only the named services, and only entries a partition list restricts:
    /// an entry without one is readable by any build already.
    #[test]
    fn entries_are_the_named_services_with_a_partition_list() {
        let services = vec![
            "kurama-token".to_owned(),
            "kurama-session".to_owned(),
            "OP_TOKEN".to_owned(),
        ];

        assert_eq!(
            parse_entries(DUMP, &services),
            vec![
                Entry {
                    service: "kurama-token".into(),
                    account: "github".into(),
                    partitions: vec!["cdhash:b4a96b123ec04b484a2648f6c6f69b6ba67c281f".into()],
                },
                Entry {
                    service: "OP_TOKEN".into(),
                    account: "u".into(),
                    partitions: vec![
                        "apple-tool:".into(),
                        "cdhash:ff4a76fb89315de6f99c565139a2617093457b76".into()
                    ],
                },
            ]
        );
    }

    /// The hashes already there stay, for a `--as` build beside the daily one;
    /// a list that holds the hash needs no password at all.
    #[test]
    fn the_new_hash_is_appended_to_what_the_list_holds() {
        let partitions = vec!["apple-tool:".to_owned(), "cdhash:aaa".to_owned()];

        assert_eq!(
            extended_partitions(&partitions, "bbb").as_deref(),
            Some("apple-tool:,cdhash:aaa,cdhash:bbb")
        );
        assert_eq!(extended_partitions(&partitions, "aaa"), None);
    }

    /// A fake `security` records whether it could open a controlling terminal
    /// and what it read on stdin. Run the test from a terminal (or under
    /// `script -q /dev/null`) to see it fail when the session is not new.
    #[test]
    fn security_gets_no_terminal_and_reads_the_password_from_stdin() {
        let dir = std::env::temp_dir().join(format!("kurama-partition-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let fake = dir.join("security");
        std::fs::write(
            &fake,
            "#!/bin/sh\n\
             if (: < /dev/tty) 2>/dev/null; then echo tty; else echo none; fi > \"$0.tty\"\n\
             /bin/cat > \"$0.stdin\"\n\
             echo \"$@\" > \"$0.args\"\n",
        )
        .unwrap();
        std::os::unix::fs::PermissionsExt::set_mode(
            &mut std::fs::metadata(&fake).unwrap().permissions(),
            0o755,
        );
        std::fs::set_permissions(&fake, std::os::unix::fs::PermissionsExt::from_mode(0o755))
            .unwrap();
        let entry = Entry {
            service: "kurama-token".into(),
            account: "github".into(),
            partitions: vec![],
        };

        let mut command = partition_command(&entry, "cdhash:aaa");
        command.env("PATH", &dir);
        let mut child = command.spawn().unwrap();
        writeln!(child.stdin.take().unwrap(), "hunter2").unwrap();
        assert!(child.wait().unwrap().success());

        let read = |suffix: &str| std::fs::read_to_string(dir.join(format!("security.{suffix}")));
        let observed = (
            read("tty").unwrap(),
            read("stdin").unwrap(),
            read("args").unwrap(),
        );
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(observed.0, "none\n");
        assert_eq!(observed.1, "hunter2\n");
        assert!(!observed.2.contains("hunter2"), "{}", observed.2);
    }
}
