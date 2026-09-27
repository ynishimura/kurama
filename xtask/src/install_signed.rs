//! `cargo xtask install-signed`: build the release binary, code sign it with a
//! stable self-signed identity and install it as `~/.cargo/bin/kurama`.
//!
//! macOS grants keychain access per code signature: the ACL of an entry stores
//! the designated requirement of the binary that created it. An ad-hoc signed
//! build is identified by its `cdhash`, which changes with every build, so the
//! MFA session and every OAuth token have to be approved again after every
//! install. Signing with one identity and one identifier makes the requirement
//! `identifier "<id>" and certificate leaf H"<cert>"`, which survives rebuilds.
//! The partition list of each entry still names builds by `cdhash`, so the
//! install ends by adding the new one there (`keychain_partition`).
//! `cargo install` would rebuild and copy an ad-hoc signed binary over the
//! signed one, so build, sign, verify and install belong in one command.
//!
//! `--as NAME` installs the same signed build under another name, so a branch
//! can be tried against the real keychain and services while the daily
//! `kurama` stays what it was: the grant belongs to the signature, not to the
//! file name.

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::root;

const USAGE: &str = "\
usage: cargo xtask install-signed [--as NAME] [--identity NAME] [--identifier ID]

  --as NAME           install as ~/.cargo/bin/NAME instead of replacing kurama;
                      the signature is the same, so the same keychain grants apply
  --identity NAME     code-signing identity in the login keychain (default: kurama-dev)
  --identifier ID     identifier stamped into the signature (default: dev.kurama.cli)

Both must stay the same across installs: the designated requirement names them,
and a changed requirement makes macOS ask for every keychain entry again.
";

const DEFAULT_IDENTITY: &str = "kurama-dev";
const DEFAULT_IDENTIFIER: &str = "dev.kurama.cli";
const DEFAULT_NAME: &str = "kurama";

#[derive(Debug, PartialEq)]
enum Request {
    Help,
    Install {
        identity: String,
        identifier: String,
        /// The file name under `~/.cargo/bin`.
        name: String,
    },
}

pub fn install_signed(args: &[String]) -> Result<(), String> {
    let (identity, identifier, name) = match parse(args)? {
        Request::Help => {
            print!("{USAGE}");
            return Ok(());
        }
        Request::Install {
            identity,
            identifier,
            name,
        } => (identity, identifier, name),
    };
    if let Some(problem) = name_problem(&name) {
        // A usage error, which exits 2 like kurama's own; every other xtask
        // failure is 1.
        eprintln!("error: `--as {name}` {problem}\n\n{USAGE}");
        std::process::exit(2);
    }

    // Before the release build, so a missing identity costs no compile time.
    require_identity(&identity)?;

    let binary = release_binary();
    run(
        "build",
        crate::cargo()
            .args(["build", "--release", "--locked"])
            .current_dir(root()),
    )?;
    run(
        "sign",
        Command::new("codesign")
            .args(["--force", "--sign", &identity, "--identifier", &identifier])
            .arg(&binary),
    )?;
    run(
        "verify",
        Command::new("codesign")
            .args(["--verify", "--strict"])
            .arg(&binary),
    )?;

    // The requirement is the point of the command: it is what the keychain
    // stores, so the user reads it here instead of a bare `cdhash`, and a
    // signature without the identifier stops the install with the working
    // binary still in place.
    let requirement = read_requirement(&binary)?;
    print!("{requirement}");
    let stamped = format!("identifier \"{identifier}\"");
    if !requirement.contains(&stamped) {
        return Err(format!(
            "the signature does not carry `{stamped}`, so keychain grants would \
             not survive the next build; nothing was installed."
        ));
    }

    let installed = installed_path(&name)?;
    install_binary(&binary, &installed)?;
    eprintln!("==> installed {}", installed.display());
    crate::keychain_partition::grant_installed_build(&installed)
}

/// Copies next to the destination and renames over it. `std::fs::copy`
/// truncates its destination, so copying straight onto the installed binary
/// would leave a truncated `kurama` first on the user's PATH when the copy
/// fails part-way; a rename within one directory replaces it in one step. The
/// copy carries the source's mode, so the installed binary stays executable.
fn install_binary(binary: &Path, installed: &Path) -> Result<(), String> {
    let staged = installed.with_extension("new");
    std::fs::copy(binary, &staged).map_err(|e| format!("stage {}: {e}", staged.display()))?;
    std::fs::rename(&staged, installed).map_err(|e| format!("install {}: {e}", installed.display()))
}

/// The designated requirement stored in the signature. `codesign -d -r-`
/// splits its output: `designated => ...` on stdout, `Executable=...` on
/// stderr.
fn read_requirement(binary: &Path) -> Result<String, String> {
    let shown = Command::new("codesign")
        .args(["-d", "-r-"])
        .arg(binary)
        .output()
        .map_err(|e| format!("codesign -d -r-: {e}"))?;
    if !shown.status.success() {
        return Err(format!(
            "codesign -d -r- {}: {}",
            binary.display(),
            String::from_utf8_lossy(&shown.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&shown.stdout).into_owned())
}

/// The binary `cargo build --release` just wrote, which `CARGO_TARGET_DIR`
/// moves: without it this command would sign and install a stale build.
fn release_binary() -> PathBuf {
    crate::target_dir().join("release/kurama")
}

fn parse(args: &[String]) -> Result<Request, String> {
    let mut identity = DEFAULT_IDENTITY.to_string();
    let mut identifier = DEFAULT_IDENTIFIER.to_string();
    let mut name = DEFAULT_NAME.to_string();
    let mut rest = args.iter().map(String::as_str);
    while let Some(flag) = rest.next() {
        match flag {
            "--as" => name = crate::value_of(flag, &mut rest, USAGE)?,
            "--identity" => identity = crate::value_of(flag, &mut rest, USAGE)?,
            "--identifier" => identifier = crate::value_of(flag, &mut rest, USAGE)?,
            "--help" | "-h" => return Ok(Request::Help),
            other => return Err(format!("unknown argument `{other}`\n\n{USAGE}")),
        }
    }
    Ok(Request::Install {
        identity,
        identifier,
        name,
    })
}

/// Why `name` cannot be a command in `~/.cargo/bin`, if it cannot: a path
/// would install somewhere else, and a leading `-` or `.` makes a file no
/// shell runs by name.
fn name_problem(name: &str) -> Option<&'static str> {
    if name.is_empty() {
        return Some("names nothing");
    }
    if name.starts_with('-') || name.starts_with('.') {
        return Some("cannot start with `-` or `.`");
    }
    if !name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
    {
        return Some("is not a command name: use letters, digits, `-`, `_` and `.`");
    }
    None
}

/// `codesign` answers a missing identity with a one-line "no identity found";
/// this turns it into the steps that create the right kind of certificate.
fn require_identity(identity: &str) -> Result<(), String> {
    // No `-v`: that also asks for a trust root, which `codesign` does not need
    // and which cannot be added without an authorization dialog.
    let listing = Command::new("security")
        .args(["find-identity", "-p", "codesigning"])
        .output()
        .map_err(|e| format!("security find-identity: {e}"))?;
    if identity_is_listed(&String::from_utf8_lossy(&listing.stdout), identity) {
        eprintln!("==> identity: {identity}");
        return Ok(());
    }
    Err(missing_identity_hint(identity))
}

/// `security find-identity` prints each common name in double quotes, among
/// lines that name the policy, the SHA-1 hashes and the count; matching the
/// quoted name keeps `identities`, `Policy` or a hash prefix from passing as
/// an identity.
fn identity_is_listed(listing: &str, identity: &str) -> bool {
    listing.contains(&format!("\"{identity}\""))
}

fn missing_identity_hint(identity: &str) -> String {
    format!(
        "no code-signing identity named `{identity}` in the login keychain.\n\
         Create one without leaving the terminal (20 years: the signature names\n\
         the certificate, so replacing an expired one makes macOS ask again):\n\
         \n  \
         openssl req -x509 -newkey rsa:2048 -nodes -days 7300 \\\n    \
           -keyout /tmp/{identity}.key -out /tmp/{identity}.crt \\\n    \
           -subj \"/CN={identity}\" \\\n    \
           -addext \"basicConstraints=critical,CA:false\" \\\n    \
           -addext \"keyUsage=critical,digitalSignature\" \\\n    \
           -addext \"extendedKeyUsage=critical,codeSigning\"\n  \
         P=$(openssl rand -hex 16)\n  \
         openssl pkcs12 -export -inkey /tmp/{identity}.key -in /tmp/{identity}.crt \\\n    \
           -out /tmp/{identity}.p12 -name {identity} -passout \"pass:$P\" \\\n    \
           -macalg sha1 -certpbe PBE-SHA1-3DES -keypbe PBE-SHA1-3DES -legacy\n  \
         security import /tmp/{identity}.p12 -P \"$P\" -A -T /usr/bin/codesign\n  \
         rm -f /tmp/{identity}.key /tmp/{identity}.p12; unset P\n\
         \n\
         Two details are not optional: `-legacy`, because macOS cannot read the\n\
         PKCS#12 that OpenSSL 3 writes by default, and a non-empty password,\n\
         because an empty one fails the same import with `MAC verification\n\
         failed`. The first signing then asks once for permission to use the\n\
         key: answer Always Allow. Keychain Access > Certificate Assistant does\n\
         the same job; either way, see docs/development/setup.md."
    )
}

/// Where `cargo install` writes: `$CARGO_HOME/bin`, which cargo itself sets
/// for the commands it spawns, and `$HOME/.cargo/bin` for a bare run.
pub(crate) fn installed_path(name: &str) -> Result<PathBuf, String> {
    let cargo_home = match std::env::var_os("CARGO_HOME") {
        Some(home) => PathBuf::from(home),
        None => PathBuf::from(std::env::var_os("HOME").ok_or("HOME is not set")?).join(".cargo"),
    };
    Ok(cargo_home.join("bin").join(name))
}

/// Runs one step with its output on the terminal; a non-zero exit is the error.
/// The line it prints is read back from the command, so it cannot drift from
/// the arguments that run.
fn run(step: &str, command: &mut Command) -> Result<(), String> {
    let line = command_line(command);
    eprintln!("==> {step}: {line}");
    let status = command.status().map_err(|e| format!("{step}: {e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("{step} failed: {line}"))
    }
}

fn command_line(command: &Command) -> String {
    std::iter::once(command.get_program())
        .chain(command.get_args())
        .map(|part| part.to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::args;

    #[test]
    fn signing_defaults_to_the_stable_identity_and_identifier() {
        assert_eq!(
            parse(&[]).unwrap(),
            Request::Install {
                identity: DEFAULT_IDENTITY.into(),
                identifier: DEFAULT_IDENTIFIER.into(),
                name: "kurama".into(),
            }
        );
    }

    #[test]
    fn both_the_identity_and_the_identifier_can_be_overridden() {
        assert_eq!(
            parse(&args(&[
                "--identity",
                "other-dev",
                "--identifier",
                "com.example.kurama"
            ]))
            .unwrap(),
            Request::Install {
                identity: "other-dev".into(),
                identifier: "com.example.kurama".into(),
                name: "kurama".into(),
            }
        );
    }

    #[test]
    fn a_flag_without_a_value_is_a_usage_error() {
        assert!(parse(&args(&["--identity"])).is_err());
        assert!(parse(&args(&["--wat"])).is_err());
    }

    #[test]
    fn help_is_a_request_of_its_own() {
        assert_eq!(parse(&args(&["--help"])).unwrap(), Request::Help);
    }

    /// The missing-identity message must carry the steps that create a
    /// code-signing certificate, not a bare "no identity found".
    #[test]
    fn a_missing_identity_names_the_certificate_to_create() {
        let hint = missing_identity_hint("kurama-dev");

        assert!(hint.contains("codeSigning"), "{hint}");
    }

    /// `-p codesigning` still prints the policy, the count and the hashes;
    /// only the quoted common name is an identity.
    #[test]
    fn only_the_quoted_common_name_counts_as_an_identity() {
        let listing = "  Policy: Code Signing\n  \
                       Matching identities\n  \
                       1) A1B2C3D4E5 \"kurama-dev\"\n     \
                       1 identities found\n";

        assert!(identity_is_listed(listing, "kurama-dev"));
        assert!(!identity_is_listed(listing, "identities"));
        assert!(!identity_is_listed(listing, "Policy"));
        assert!(!identity_is_listed(listing, "A1B2C3"));
        assert!(!identity_is_listed(listing, "kurama"));
    }

    /// A failing copy must not be able to truncate the installed binary, so
    /// the new one is staged beside it and renamed over it, executable bit
    /// and all.
    #[test]
    fn installing_replaces_the_binary_in_one_step() {
        use std::os::unix::fs::PermissionsExt;

        let dir = std::env::temp_dir().join(format!("kurama-install-{}", std::process::id()));
        let bin = dir.join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        let built = dir.join("built");
        std::fs::write(&built, b"new binary").unwrap();
        std::fs::set_permissions(&built, PermissionsExt::from_mode(0o755)).unwrap();
        let installed = bin.join("kurama");
        std::fs::write(&installed, b"old binary").unwrap();

        install_binary(&built, &installed).unwrap();

        assert_eq!(std::fs::read(&installed).unwrap(), b"new binary");
        assert_eq!(
            std::fs::metadata(&installed).unwrap().permissions().mode() & 0o777,
            0o755
        );
        assert!(!installed.with_extension("new").exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// The alias keeps the signature and changes only where the file goes.
    #[test]
    fn as_names_the_file_and_leaves_the_signature_alone() {
        assert_eq!(
            parse(&args(&["--as", "kurama-64"])).unwrap(),
            Request::Install {
                identity: DEFAULT_IDENTITY.into(),
                identifier: DEFAULT_IDENTIFIER.into(),
                name: "kurama-64".into(),
            }
        );
        assert!(parse(&args(&["--as"])).is_err());
        assert!(
            installed_path("kurama-64")
                .unwrap()
                .ends_with(".cargo/bin/kurama-64")
                || std::env::var_os("CARGO_HOME").is_some()
        );
        assert_eq!(
            installed_path("kurama-64").unwrap().file_name().unwrap(),
            "kurama-64"
        );
    }

    #[test]
    fn a_name_that_is_not_a_command_name_is_refused() {
        for name in [
            "",
            "../kurama",
            "bin/kurama",
            "-kurama",
            ".kurama",
            "kurama dev",
        ] {
            assert!(name_problem(name).is_some(), "{name:?} was accepted");
        }
        for name in ["kurama", "kurama-dev", "kurama-64", "kurama_2.1"] {
            assert_eq!(name_problem(name), None, "{name:?} was refused");
        }
    }
}
