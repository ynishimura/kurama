//! The one way kurama saves config.toml: read it, build a candidate, validate the candidate whole, and replace the file atomically unless it changed since it was read.
//!
//! Every command that writes the file goes through [`ConfigFile`]:
//!
//! 1. [`ConfigFile::open`] reads the file `KURAMA_CONFIG_PATH` or the default
//!    location names; an absent file reads as empty and is created on save.
//! 2. The caller builds the whole new text -- [`ConfigFile::append`] for new
//!    units, keeping every byte already there, or [`ConfigFile::edit`] for
//!    an [`Edit`] of the file's document that changes only what it names.
//!    What a person gave passes [`check_input`](super::input::check_input)
//!    first.
//! 3. [`ConfigFile::validate`] (an append) or [`ConfigFile::validate_edit`]
//!    (an edit) reads the candidate as `Config::parse` does, then checks it
//!    against `~/.aws/config`, which the parse cannot see. A problem in what
//!    the input adds or changes is an [`InputError`]; one in the rest of the
//!    file is a configuration error.
//! 4. [`ConfigFile::save`] takes only a [`Validated`] candidate, so nothing
//!    unchecked is written. It takes a short lock beside the file, refuses
//!    when the file is no longer what was read, and renames a temporary file
//!    from the same directory over it: the mode it had (0600 for a new file)
//!    is kept, and a symbolic link at the path keeps pointing where it did,
//!    even at a file that does not exist yet.
//!
//! Nothing here resolves a secret or reaches the network; the AWS config is
//! only read.

use std::fs::{self, File, OpenOptions, Permissions};
use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

use anyhow::Result;

use super::edit::Edit;
use super::input::{InputError, check_input};
use super::references::{aws_references, check_aws};
use super::saved::Saved;
use super::{Config, line_of, read_error};
use crate::adapters::error::CoreError;

/// A save that did not happen. The file was not changed either way.
#[derive(Debug, thiserror::Error)]
pub enum WriteError {
    #[error("cannot save {}", path.display())]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("{} changed after kurama read it; nothing was written", path.display())]
    Changed { path: PathBuf },
}

/// config.toml as it was read before an edit.
#[derive(Debug)]
pub struct ConfigFile {
    /// The path kurama was given.
    path: PathBuf,
    /// The file that is replaced: `path`, or where a symbolic link at `path`
    /// points.
    target: PathBuf,
    /// What was read; `None` when there was no file.
    original: Option<String>,
}

/// `fragment` put after the file.
#[derive(Debug)]
pub struct Appended {
    /// The file followed by `added`.
    candidate: String,
    /// Where the fragment starts in `candidate`.
    input_start: usize,
    /// What follows the original bytes: a separator, then the fragment.
    pub added: String,
    /// The units the fragment adds, in its order (`auth.github`).
    pub units: Vec<String>,
}

/// A candidate [`ConfigFile::validate`] or [`ConfigFile::validate_edit`]
/// passed; only this is saved.
#[derive(Debug)]
pub struct Validated {
    text: String,
    /// What could not be checked (`aws_profile` without an AWS config).
    pub warnings: Vec<String>,
}

impl ConfigFile {
    /// The file `KURAMA_CONFIG_PATH`, else the default location, names.
    pub fn open() -> Result<Self> {
        Self::read(Config::config_path()?)
    }

    /// The file at `path`; an absent one is empty.
    pub fn read(path: PathBuf) -> Result<Self> {
        // A link to a file that does not exist yet keeps pointing there: the
        // file is created where it points.
        let target = match fs::read_link(&path) {
            Ok(link) => fs::canonicalize(&path).unwrap_or_else(|_| match path.parent() {
                Some(parent) => parent.join(link),
                None => link,
            }),
            Err(_) => path.clone(),
        };
        let original = match fs::read_to_string(&target) {
            Ok(content) => Some(content),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => {
                return Err(
                    CoreError::config(format!("cannot read {}: {error}", path.display())).into(),
                );
            }
        };
        Ok(Self {
            path,
            target,
            original,
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The text read; empty when there was no file.
    pub fn content(&self) -> &str {
        self.original.as_deref().unwrap_or_default()
    }

    /// The file with `fragment` after it: TOML that passes
    /// [`check_input`] and adds only units the file does not have. The
    /// file's bytes stay as they are; a newline and a blank line go before
    /// the fragment when the file does not end with them.
    pub fn append(&self, fragment: &str) -> Result<Appended> {
        let saved = Saved::parse(self.content()).map_err(CoreError::from)?;
        let units = check_input(fragment)?.units();
        let existing = saved.units();
        let taken: Vec<String> = units
            .iter()
            .filter(|unit| existing.contains(unit))
            .cloned()
            .collect();
        if !taken.is_empty() {
            return Err(InputError::Taken {
                path: self.path.clone(),
                units: taken,
            }
            .into());
        }
        let content = self.content();
        let mut added = String::new();
        if !content.is_empty() {
            if !content.ends_with('\n') {
                added.push('\n');
            }
            if !content.ends_with("\n\n") {
                added.push('\n');
            }
        }
        let input_start = content.len() + added.len();
        added.push_str(fragment);
        if !fragment.ends_with('\n') {
            added.push('\n');
        }
        Ok(Appended {
            candidate: format!("{content}{added}"),
            input_start,
            added,
            units,
        })
    }

    /// `appended` read as `Config::parse` reads it, then checked as
    /// [`Self::check`] says. A value `Config` refuses inside the fragment is
    /// named by its line in the fragment.
    pub async fn validate(&self, appended: &Appended) -> Result<Validated> {
        let text = &appended.candidate;
        let start = appended.input_start;
        let config: Config = toml::from_str(text).map_err(|error| match error.span() {
            Some(span) if span.start >= start => {
                let line = line_of(&text[start..], Some(span.start - start..span.end - start));
                anyhow::Error::from(InputError::Invalid(format!(
                    "the input line {}: {}",
                    line.unwrap_or(1),
                    error.message().replace(['\r', '\n'], " ")
                )))
            }
            _ => read_error(text, &error).into(),
        })?;
        self.check(text.clone(), config, &appended.units, None)
            .await
    }

    /// The file's document, to edit in place. A file that is not TOML is not
    /// edited: its syntax error is the file's own `CONFIG_INVALID`.
    pub fn edit(&self) -> Result<Edit> {
        Ok(Edit::new(&self.path, self.content()).map_err(CoreError::from)?)
    }

    /// The edited document read as `Config::parse` reads it, then checked
    /// as [`Self::check`] says. What the edit brought in is the edit's
    /// mistake: a value `Config` refuses, named by its line in the edited
    /// file, unless the file read before the edit was refused with the same
    /// message on the same line; and a problem the file did not have
    /// before, wherever it is (an `[api.*]` header the new `header` of its
    /// auth forbids).
    pub async fn validate_edit(&self, edit: &Edit) -> Result<Validated> {
        let text = edit.text();
        let before = toml::from_str::<Config>(self.content());
        let config: Config = toml::from_str(&text).map_err(|error| {
            let line = line_of(&text, error.span());
            match &before {
                Err(own)
                    if own.message() == error.message()
                        && line_of(self.content(), own.span()) == line =>
                {
                    anyhow::Error::from(read_error(self.content(), own))
                }
                _ => InputError::Invalid(format!(
                    "after the edit, config.toml line {}: {}",
                    line.unwrap_or(1),
                    error.message().replace(['\r', '\n'], " ")
                ))
                .into(),
            }
        })?;
        let had = match &before {
            Ok(before) => Some(
                problems(before)
                    .await?
                    .0
                    .into_iter()
                    .map(|(_, error)| error.to_string())
                    .collect(),
            ),
            Err(_) => None,
        };
        self.check(text, config, &edit.units(), had).await
    }

    /// `config` checked as `Config::parse` checks it, then against the AWS
    /// config (see [`problems`]). A problem inside one of `units` -- what
    /// the input adds or changes -- or, when `had` lists the problems of the
    /// file before an edit, one not among them, is an [`InputError`] and is
    /// reported first; any other is the file's own `CONFIG_INVALID`.
    async fn check(
        &self,
        text: String,
        config: Config,
        units: &[String],
        had: Option<Vec<String>>,
    ) -> Result<Validated> {
        let (found, warnings) = problems(&config).await?;
        let caused = |section: &str, error: &CoreError| {
            units
                .iter()
                .any(|unit| section == unit || section.starts_with(&format!("{unit}.")))
                || had
                    .as_ref()
                    .is_some_and(|had| !had.contains(&error.to_string()))
        };
        let first_caused = found
            .iter()
            .position(|(section, error)| caused(section, error));
        let mut found = found;
        if let Some(index) = first_caused {
            return Err(InputError::Section(found.swap_remove(index).1).into());
        }
        if let Some((_, error)) = found.into_iter().next() {
            return Err(error.into());
        }
        Ok(Validated { text, warnings })
    }

    /// Replace the file with a validated candidate, unless the file is no
    /// longer what was read. See the module documentation.
    pub fn save(&self, validated: Validated) -> Result<(), WriteError> {
        let candidate = validated.text.as_str();
        let target = &self.target;
        let io = |source| WriteError::Io {
            path: target.clone(),
            source,
        };
        let directory = match target.parent() {
            Some(parent) if !parent.as_os_str().is_empty() => parent,
            _ => Path::new("."),
        };
        fs::create_dir_all(directory).map_err(io)?;
        // Held until this function returns: two kurama processes compare and
        // replace one after the other.
        let lock = File::open(directory).map_err(io)?;
        lock.lock().map_err(io)?;
        let current = match fs::read_to_string(target) {
            Ok(content) => Some(content),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => return Err(io(error)),
        };
        if current != self.original {
            return Err(WriteError::Changed {
                path: target.clone(),
            });
        }
        let mode = match fs::metadata(target) {
            Ok(metadata) => metadata.permissions().mode() & 0o7777,
            Err(_) => 0o600,
        };
        let name = target
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        let temporary = directory.join(format!(".{name}.{}.tmp", std::process::id()));
        let written =
            write_file(&temporary, candidate, mode).and_then(|()| fs::rename(&temporary, target));
        if written.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        written.map_err(io)
    }
}

/// Every problem of `config`, in the order `kurama config check` finds
/// them: the rules `Config::parse` applies, then an `[auth.*]` named like an
/// AWS profile and an `aws_profile` naming none; and the warnings. Without an
/// AWS config the second check is a warning, as in `kurama config check`.
async fn problems(config: &Config) -> Result<(Vec<(String, CoreError)>, Vec<String>)> {
    let mut found = config.problems();
    let aws = check_aws(config).await?;
    found.extend(aws.collisions);
    let mut warnings = Vec::new();
    match aws.unknown {
        Some(unknown) => found.extend(unknown.into_iter().map(|(section, profile)| {
            let error = CoreError::config(format!(
                "[{section}] aws_profile = \"{profile}\" names no profile in {}",
                aws.aws_config.display()
            ));
            (section, error)
        })),
        None if !aws_references(config).is_empty() => warnings.push(format!(
            "{} does not exist: aws_profile references are not checked",
            aws.aws_config.display()
        )),
        None => {}
    }
    Ok((found, warnings))
}

fn write_file(path: &Path, content: &str, mode: u32) -> std::io::Result<()> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)?;
    file.write_all(content.as_bytes())?;
    file.set_permissions(Permissions::from_mode(mode))?;
    file.sync_all()
}

#[cfg(test)]
mod tests {
    use super::*;

    const EXISTING: &str = "# mine\n[core]\nlog_level = \"debug\"   # keep this";
    const FRAGMENT: &str = "[auth.svc]\nkind = \"token\"\ntoken = \"op://Agent/svc/credential\"\n\n\
         [api.svc]\nbase_url = \"https://svc.example.com\"\nauth = \"svc\"\n";

    fn file_with(dir: &Path, content: Option<&str>) -> PathBuf {
        let path = dir.join("config.toml");
        if let Some(content) = content {
            fs::write(&path, content).unwrap();
        }
        path
    }

    /// What `validate` would hand `save`, for a test of `save` alone.
    fn validated(text: &str) -> Validated {
        Validated {
            text: text.to_owned(),
            warnings: Vec::new(),
        }
    }

    #[test]
    fn an_append_keeps_the_original_bytes_and_separates_the_fragment() {
        let dir = tempfile::tempdir().unwrap();
        let file = ConfigFile::read(file_with(dir.path(), Some(EXISTING))).unwrap();
        let appended = file.append(FRAGMENT).unwrap();
        assert!(appended.candidate.starts_with(EXISTING));
        assert_eq!(appended.added, format!("\n\n{FRAGMENT}"));
        assert_eq!(appended.units, ["auth.svc", "api.svc"]);
        assert!(Config::parse(&appended.candidate).is_ok());

        let ended = ConfigFile::read(file_with(dir.path(), Some("[core]\n\n"))).unwrap();
        assert_eq!(ended.append("[aws]").unwrap().added, "[aws]\n");
        let absent = ConfigFile::read(dir.path().join("none.toml")).unwrap();
        assert_eq!(absent.append(FRAGMENT).unwrap().candidate, FRAGMENT);
    }

    #[test]
    fn a_unit_the_file_has_is_refused_whole() {
        let dir = tempfile::tempdir().unwrap();
        let file = ConfigFile::read(file_with(
            dir.path(),
            Some("[api.svc]\nbase_url = \"https://old\"\n[aws.session_name]\nprefix = \"x\"\n"),
        ))
        .unwrap();
        let taken = |fragment| match file.append(fragment).unwrap_err().downcast::<InputError>() {
            Ok(InputError::Taken { units, .. }) => units,
            other => panic!("{other:?}"),
        };
        assert_eq!(taken(FRAGMENT), ["api.svc"]);
        assert_eq!(taken("[aws.session_cache]\nduration = 3600\n"), ["aws"]);
    }

    #[test]
    fn an_input_the_policy_refuses_is_not_appended() {
        let dir = tempfile::tempdir().unwrap();
        let file = ConfigFile::read(file_with(dir.path(), Some(EXISTING))).unwrap();
        let error = file
            .append("[auth.n]\nkind = \"token\"\ntoken = 42\n")
            .unwrap_err();
        assert!(matches!(
            error.downcast_ref::<InputError>(),
            Some(InputError::LiteralSecret { .. })
        ));
    }

    /// A value the edit wrote wrong is the edit's mistake, named by its line
    /// in the edited file; a file that was refused before the edit keeps its
    /// own error, with its own line.
    #[tokio::test]
    async fn a_type_error_is_the_edits_or_the_files_own() {
        let dir = tempfile::tempdir().unwrap();
        let file = ConfigFile::read(file_with(dir.path(), Some(EXISTING))).unwrap();
        let mut edit = file.edit().unwrap();
        edit.set("core.log_level", "42").unwrap();
        let error = file.validate_edit(&edit).await.unwrap_err();
        match error.downcast_ref::<InputError>() {
            Some(InputError::Invalid(message)) => {
                assert!(
                    message.starts_with("after the edit, config.toml line 3:"),
                    "{message}"
                )
            }
            other => panic!("{other:?}"),
        }

        let broken = "[core]\nlog_level = 1\n\n[api.x]\nbase_url = \"https://x\"\n";
        let file = ConfigFile::read(file_with(dir.path(), Some(broken))).unwrap();
        let mut edit = file.edit().unwrap();
        edit.set("api.x.base_url", "\"https://y\"").unwrap();
        let error = file.validate_edit(&edit).await.unwrap_err();
        assert!(error.downcast_ref::<InputError>().is_none(), "{error:#}");
        assert!(
            error.to_string().contains("config.toml line 2:"),
            "{error:#}"
        );
        // An edit that fixes the file's error and makes another is named
        // by the new error, not the old one.
        let file =
            ConfigFile::read(file_with(dir.path(), Some("[core]\nlog_level = 1\n"))).unwrap();
        let mut edit = file.edit().unwrap();
        edit.replace("[core]\nlog_level = \"debug\"\nbogus = 1\n")
            .unwrap();
        let error = file.validate_edit(&edit).await.unwrap_err();
        match error.downcast_ref::<InputError>() {
            Some(InputError::Invalid(message)) => assert!(
                message.starts_with("after the edit, config.toml line 3:")
                    && message.contains("bogus"),
                "{message}"
            ),
            other => panic!("{other:?}: {error:#}"),
        }
        let syntax = ConfigFile::read(file_with(dir.path(), Some("[core\n"))).unwrap();
        assert!(syntax.edit().is_err());
    }

    #[test]
    fn a_save_replaces_the_file_and_keeps_its_mode() {
        let dir = tempfile::tempdir().unwrap();
        let path = file_with(dir.path(), Some(EXISTING));
        fs::set_permissions(&path, Permissions::from_mode(0o640)).unwrap();
        let file = ConfigFile::read(path.clone()).unwrap();
        let candidate = file.append(FRAGMENT).unwrap().candidate;
        file.save(validated(&candidate)).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), candidate);
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o640
        );
        let left: Vec<_> = fs::read_dir(dir.path()).unwrap().collect();
        assert_eq!(left.len(), 1, "no temporary file is left");
    }

    #[test]
    fn a_new_file_and_its_directory_are_created_for_the_owner_only() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("kurama").join("config.toml");
        let file = ConfigFile::read(path.clone()).unwrap();
        file.save(validated(FRAGMENT)).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), FRAGMENT);
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }

    #[test]
    fn a_symbolic_link_keeps_pointing_at_the_file_it_named() {
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("dotfiles.toml");
        fs::write(&real, EXISTING).unwrap();
        let link = dir.path().join("config.toml");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let file = ConfigFile::read(link.clone()).unwrap();
        let candidate = file.append(FRAGMENT).unwrap().candidate;
        file.save(validated(&candidate)).unwrap();
        assert!(
            fs::symlink_metadata(&link)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(fs::read_to_string(&real).unwrap(), candidate);
    }

    #[test]
    fn a_link_to_a_file_that_does_not_exist_yet_creates_it_where_it_points() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir(dir.path().join("dotfiles")).unwrap();
        let link = dir.path().join("config.toml");
        std::os::unix::fs::symlink("dotfiles/kurama.toml", &link).unwrap();
        let file = ConfigFile::read(link.clone()).unwrap();
        file.save(validated(FRAGMENT)).unwrap();
        assert!(
            fs::symlink_metadata(&link)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(
            fs::read_to_string(dir.path().join("dotfiles/kurama.toml")).unwrap(),
            FRAGMENT
        );
    }

    #[test]
    fn a_file_changed_after_it_was_read_is_not_overwritten() {
        let dir = tempfile::tempdir().unwrap();
        let path = file_with(dir.path(), Some(EXISTING));
        let file = ConfigFile::read(path.clone()).unwrap();
        fs::write(&path, "[core]\n# edited elsewhere\n").unwrap();
        let error = file.save(validated(FRAGMENT)).unwrap_err();
        assert!(
            matches!(&error, WriteError::Changed { path: named } if *named == path),
            "{error}"
        );
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            "[core]\n# edited elsewhere\n"
        );
    }

    #[test]
    fn a_directory_that_cannot_be_written_fails_and_leaves_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = file_with(dir.path(), Some(EXISTING));
        let file = ConfigFile::read(path.clone()).unwrap();
        fs::set_permissions(dir.path(), Permissions::from_mode(0o500)).unwrap();
        let error = file.save(validated(FRAGMENT)).unwrap_err();
        fs::set_permissions(dir.path(), Permissions::from_mode(0o700)).unwrap();
        assert!(matches!(error, WriteError::Io { .. }), "{error}");
        assert_eq!(fs::read_to_string(&path).unwrap(), EXISTING);
    }
}
