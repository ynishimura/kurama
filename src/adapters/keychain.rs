//! One macOS keychain entry per `(service, key)` with a JSON value in the
//! password field. The session cache (service `kurama-session`) and the
//! token store (`kurama-token`) share it. Headless callers get an error
//! instead of waiting for a Keychain dialog. Runtime scenarios read raw
//! secrets from a directory instead (`--features test-fakes` plus
//! `KURAMA_TEST_KEYCHAIN_SECRET_DIR`, one file per `<service>/<key>`, byte
//! for byte as the keychain would return it).
//! Other platforms report that native keychain reads are unavailable; the
//! file-backed secret fake remains available for their runtime scenarios.

use crate::ports::KeychainDenied;
#[cfg(target_os = "macos")]
use serde::Serialize;
#[cfg(target_os = "macos")]
use serde::de::DeserializeOwned;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeychainError {
    Backend(String),
    #[cfg(target_os = "macos")]
    Denied(KeychainDenied),
    #[cfg(target_os = "macos")]
    InvalidData,
}

#[cfg(target_os = "macos")]
pub fn load_json<T: DeserializeOwned>(
    service: &str,
    key: &str,
) -> Result<Option<T>, KeychainError> {
    with_entry(service, key, |entry| decode(entry.get_password()))
}

/// The password field as it was stored, for an entry whose value is not JSON
/// (a 1Password service account token added with `security add-generic-password`).
pub fn load_secret(
    service: &str,
    key: &str,
) -> Result<Option<zeroize::Zeroizing<String>>, KeychainError> {
    #[cfg(feature = "test-fakes")]
    if let Some(dir) = std::env::var_os("KURAMA_TEST_KEYCHAIN_SECRET_DIR") {
        // Both dimensions, and no trimming: the keychain returns what was
        // stored, so a scenario must be able to see a stray newline too.
        let entry = std::path::Path::new(&dir).join(service).join(key);
        return match std::fs::read_to_string(entry) {
            Ok(value) => Ok(Some(zeroize::Zeroizing::new(value))),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(KeychainError::Backend(error.to_string())),
        };
    }
    #[cfg(target_os = "macos")]
    {
        with_entry(service, key, |entry| match entry.get_password() {
            Ok(value) => Ok(Some(zeroize::Zeroizing::new(value))),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(error) => Err(backend_error(error)),
        })
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (service, key);
        Err(KeychainError::Backend(
            "the keychain is only available on macOS; export OP_SERVICE_ACCOUNT_TOKEN instead"
                .into(),
        ))
    }
}

/// An entry this build may not write is replaced rather than kept: both
/// stores here are caches, and one that can be neither read nor overwritten
/// would stay poisoned for every later run. macOS answers such a write with
/// `errSecDuplicateItem` (the entry exists and this signature is not on its
/// access list) or with the access-denied codes, and deleting it needs no
/// grant, so a second attempt writes an entry this build owns.
#[cfg(target_os = "macos")]
const DUPLICATE_ITEM: i32 = -25299;

#[cfg(target_os = "macos")]
pub fn store_json<T: Serialize>(service: &str, key: &str, value: &T) -> Result<(), KeychainError> {
    let json = zeroize::Zeroizing::new(
        serde_json::to_string(value).map_err(|_| KeychainError::InvalidData)?,
    );
    with_entry(service, key, |entry| {
        write_replacing(|| entry.set_password(&json), || entry.delete_credential())
    })
}

/// Write an entry through `set`, and when the write says the entry is out of
/// reach, `delete` it and `set` once more. The retry's own failure is the one
/// worth reporting, since the first said only "it exists".
#[cfg(target_os = "macos")]
fn write_replacing(
    set: impl Fn() -> keyring::Result<()>,
    delete: impl FnOnce() -> keyring::Result<()>,
) -> Result<(), KeychainError> {
    match set() {
        Ok(()) => Ok(()),
        Err(error) if replaces_the_entry(&error) => {
            delete().map_err(backend_error)?;
            set().map_err(backend_error)
        }
        Err(error) => Err(backend_error(error)),
    }
}

/// Whether a failed write means the entry is there but out of reach, which
/// deleting it fixes.
#[cfg(target_os = "macos")]
fn replaces_the_entry(error: &keyring::Error) -> bool {
    platform_code(error)
        .is_some_and(|code| code == DUPLICATE_ITEM || ACCESS_DENIED_CODES.contains(&code))
}

/// The macOS status code behind a keyring error, when there is one.
#[cfg(target_os = "macos")]
fn platform_code(error: &keyring::Error) -> Option<i32> {
    let keyring::Error::PlatformFailure(platform) = error else {
        return None;
    };
    platform
        .downcast_ref::<security_framework::base::Error>()
        .map(|error| error.code())
}

/// Removing an absent entry is not an error.
#[cfg(target_os = "macos")]
pub fn remove(service: &str, key: &str) -> Result<(), KeychainError> {
    with_entry(service, key, |entry| match entry.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(error) => Err(backend_error(error)),
    })
}

/// macOS grants keychain access per binary signature, and every build is
/// ad-hoc signed with a new hash, so a rebuilt kurama is not on an existing
/// entry's access list. Reading it then fails with `errSecAuthFailed` (the
/// grant is missing or someone denied the dialog) or
/// `errSecInteractionNotAllowed` (`with_entry` disabled the dialog that
/// would grant it, because nobody is at the terminal). macOS words the first
/// one as a wrong password, which it is not: both mean the same thing to a
/// caller, and both are fixed the same way.
#[cfg(target_os = "macos")]
const ACCESS_DENIED_CODES: [i32; 2] = [-25293, -25308];

#[cfg(target_os = "macos")]
fn backend_error(error: keyring::Error) -> KeychainError {
    if let Some(code) = platform_code(&error).filter(|code| ACCESS_DENIED_CODES.contains(code)) {
        return KeychainError::Denied(KeychainDenied { code });
    }
    KeychainError::Backend(error.to_string())
}

/// The denial a runtime scenario asks the file-backed stores to answer with
/// (`KURAMA_TEST_KEYCHAIN_DENIED=<status code>`), as a rebuilt kurama is
/// answered by the real keychain.
#[cfg(feature = "test-fakes")]
pub fn fake_denial() -> Option<KeychainDenied> {
    let code = std::env::var("KURAMA_TEST_KEYCHAIN_DENIED").ok()?;
    Some(KeychainDenied {
        code: code
            .parse()
            .expect("KURAMA_TEST_KEYCHAIN_DENIED is a status code"),
    })
}

/// Whether the process has already warned about a denial.
struct DenialLog {
    warned: std::sync::atomic::AtomicBool,
}

impl DenialLog {
    const fn new() -> Self {
        Self {
            warned: std::sync::atomic::AtomicBool::new(false),
        }
    }

    /// True for the first denial of the process only.
    fn first(&self) -> bool {
        !self.warned.swap(true, std::sync::atomic::Ordering::Relaxed)
    }
}

static DENIALS: DenialLog = DenialLog::new();

/// A keychain entry this build was refused, which the caller treats as
/// absent. A refused build is refused every entry, so one warning per process
/// says it, with what to do; the entries themselves are named at debug.
pub fn log_denied(entry: &str, denied: KeychainDenied) {
    if DENIALS.first() {
        tracing::warn!(
            "{denied}; until then kurama goes on without the entries it cannot \
             read, and names each one at debug"
        );
    }
    tracing::debug!(
        entry,
        code = denied.code,
        "keychain entry skipped: this build may not read it"
    );
}

#[cfg(target_os = "macos")]
fn with_entry<T>(
    service: &str,
    key: &str,
    action: impl FnOnce(keyring::Entry) -> Result<T, KeychainError>,
) -> Result<T, KeychainError> {
    use std::io::IsTerminal;
    let _interaction_lock = if std::io::stdin().is_terminal() {
        None
    } else {
        Some(
            security_framework::os::macos::keychain::SecKeychain::disable_user_interaction()
                .map_err(|error| KeychainError::Backend(error.to_string()))?,
        )
    };
    let entry = keyring::Entry::new(service, key).map_err(backend_error)?;
    action(entry)
}

/// The JSON in an entry's password field; a missing entry is `None`, and
/// unreadable JSON is reported without echoing the value.
#[cfg(target_os = "macos")]
pub fn decode<T: DeserializeOwned>(
    value: keyring::Result<String>,
) -> Result<Option<T>, KeychainError> {
    match value {
        Ok(value) => serde_json::from_str(&zeroize::Zeroizing::new(value))
            .map(Some)
            .map_err(|_| KeychainError::InvalidData),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(error) => Err(backend_error(error)),
    }
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;

    #[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
    struct Entry {
        secret: String,
    }

    #[test]
    fn keyring_password_decodes_the_json_value() {
        let expected = Entry {
            secret: "test-secret".into(),
        };
        let actual: Entry = decode(Ok(serde_json::to_string(&expected).unwrap()))
            .unwrap()
            .unwrap();
        assert_eq!(actual, expected);
    }

    #[test]
    fn keyring_missing_entry_is_a_miss() {
        assert!(
            decode::<Entry>(Err(keyring::Error::NoEntry))
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn keyring_corrupt_json_reports_error_without_secret_value() {
        let error = decode::<Entry>(Ok("corrupt-sensitive-value".into())).unwrap_err();
        assert_eq!(error, KeychainError::InvalidData);
        assert!(!format!("{error:?}").contains("corrupt-sensitive-value"));
    }

    #[test]
    fn keyring_backend_errors_remain_errors() {
        assert!(matches!(
            decode::<Entry>(Err(keyring::Error::NoDefaultStore)),
            Err(KeychainError::Backend(_))
        ));
    }

    fn platform_failure(code: i32) -> keyring::Error {
        keyring::Error::PlatformFailure(Box::new(security_framework::base::Error::from_code(code)))
    }

    /// macOS reports a missing access grant as a wrong password. Say what it
    /// is instead, since the message reaches a `status` warning line where
    /// there is no hint to carry it.
    #[test]
    fn keyring_access_denied_says_a_rebuild_needs_one_approval() {
        for code in ACCESS_DENIED_CODES {
            let error = backend_error(platform_failure(code));
            assert_eq!(error, KeychainError::Denied(KeychainDenied { code }));
            let KeychainError::Denied(denied) = error else {
                unreachable!()
            };
            let message = denied.to_string();
            assert!(
                message.contains("macOS denied this build access"),
                "{message}"
            );
            assert!(message.contains("install-signed"), "{message}");
            assert!(message.contains(&code.to_string()), "{message}");
        }
    }

    /// A cache that can be neither read nor overwritten would stay poisoned
    /// for every later run, which is what happened to `kurama-session` after a
    /// rebuild: the read was denied and the write answered "already exists".
    #[test]
    fn keyring_an_entry_out_of_reach_is_replaced_instead_of_kept() {
        assert!(replaces_the_entry(&platform_failure(DUPLICATE_ITEM)));
        for code in ACCESS_DENIED_CODES {
            assert!(replaces_the_entry(&platform_failure(code)), "{code}");
        }
    }

    /// Only those: a full disk or a locked keychain is not fixed by deleting
    /// the entry, and deleting one on that guess loses a cached session for
    /// nothing.
    #[test]
    fn keyring_other_write_failures_are_not_answered_by_deleting() {
        // errSecNoSuchAttr, errSecInteractionRequired, and an error that
        // carries no macOS status code at all.
        for code in [-25303, -25315] {
            assert!(!replaces_the_entry(&platform_failure(code)), "{code}");
        }
        assert!(!replaces_the_entry(&keyring::Error::NoEntry));
        assert!(!replaces_the_entry(&keyring::Error::NoDefaultStore));
        assert_eq!(platform_code(&keyring::Error::NoEntry), None);
        assert_eq!(
            platform_code(&platform_failure(DUPLICATE_ITEM)),
            Some(DUPLICATE_ITEM)
        );
    }

    /// What a scripted entry was asked to do, in order.
    #[derive(Debug, PartialEq)]
    enum Call {
        Set,
        Delete,
    }

    /// `write_replacing` against an entry whose writes answer `writes` in
    /// turn and whose delete answers `delete`; the calls it made.
    fn write_scripted(
        writes: Vec<keyring::Result<()>>,
        delete: keyring::Result<()>,
    ) -> (Result<(), KeychainError>, Vec<Call>) {
        let calls = std::cell::RefCell::new(Vec::new());
        let writes = std::cell::RefCell::new(writes.into_iter());
        let result = write_replacing(
            || {
                calls.borrow_mut().push(Call::Set);
                writes.borrow_mut().next().expect("no more writes scripted")
            },
            || {
                calls.borrow_mut().push(Call::Delete);
                delete
            },
        );
        (result, calls.into_inner())
    }

    /// An entry out of reach is deleted and written once more, and the
    /// retry's answer is the result: its failure, not the first one.
    #[test]
    fn keychain_store_replaces_an_inaccessible_entry_then_retries() {
        for code in [
            DUPLICATE_ITEM,
            ACCESS_DENIED_CODES[0],
            ACCESS_DENIED_CODES[1],
        ] {
            let (result, calls) = write_scripted(vec![Err(platform_failure(code)), Ok(())], Ok(()));
            assert_eq!(result, Ok(()), "{code}");
            assert_eq!(calls, [Call::Set, Call::Delete, Call::Set], "{code}");

            // errSecNoSuchAttr from the retry.
            let (result, calls) = write_scripted(
                vec![Err(platform_failure(code)), Err(platform_failure(-25303))],
                Ok(()),
            );
            assert_eq!(
                result,
                Err(backend_error(platform_failure(-25303))),
                "{code}"
            );
            assert_eq!(calls, [Call::Set, Call::Delete, Call::Set], "{code}");
        }
    }

    /// A delete that fails ends the write with that failure: there is no
    /// point writing again over an entry that is still there.
    #[test]
    fn keychain_store_reports_a_failed_delete_without_retrying() {
        let (result, calls) = write_scripted(
            vec![Err(platform_failure(DUPLICATE_ITEM))],
            Err(platform_failure(ACCESS_DENIED_CODES[0])),
        );
        assert_eq!(
            result,
            Err(KeychainError::Denied(KeychainDenied {
                code: ACCESS_DENIED_CODES[0]
            }))
        );
        assert_eq!(calls, [Call::Set, Call::Delete]);
    }

    /// Any other write failure is reported as it is, and the entry is kept.
    #[test]
    fn keychain_store_keeps_the_entry_on_other_write_failures() {
        for error in [platform_failure(-25303), keyring::Error::NoDefaultStore] {
            let (result, calls) = write_scripted(vec![Err(error)], Ok(()));
            assert!(
                matches!(result, Err(KeychainError::Backend(_))),
                "{result:?}"
            );
            assert_eq!(calls, [Call::Set]);
        }
        let (result, calls) = write_scripted(vec![Ok(())], Ok(()));
        assert_eq!(result, Ok(()));
        assert_eq!(calls, [Call::Set]);
    }

    #[test]
    fn keyring_other_platform_failures_keep_their_own_message() {
        // errSecNoSuchAttr: nothing to do with an access grant.
        let KeychainError::Backend(message) = backend_error(platform_failure(-25303)) else {
            panic!("expected a backend error");
        };
        assert!(!message.contains("install-signed"), "{message}");
    }
}

#[cfg(test)]
mod denial_tests {
    use super::*;

    /// The one warning of a refused build is the first denial's; every later
    /// one is debug.
    #[test]
    fn keychain_only_the_first_denial_of_a_process_warns() {
        let log = DenialLog::new();
        assert!(log.first());
        assert!(!log.first());
        assert!(!log.first());
    }
}

#[cfg(all(test, not(target_os = "macos")))]
mod tests {
    use super::*;
    use crate::adapters::utils::test_env;

    #[test]
    #[serial_test::serial]
    fn native_keychain_reads_report_the_platform_limit() {
        let fake_dir = std::env::var_os("KURAMA_TEST_KEYCHAIN_SECRET_DIR");
        test_env::remove("KURAMA_TEST_KEYCHAIN_SECRET_DIR");
        let result = load_secret("kurama-unused", "unused");
        test_env::set_or_remove("KURAMA_TEST_KEYCHAIN_SECRET_DIR", fake_dir);
        let Err(KeychainError::Backend(message)) = result else {
            panic!("native keychain reads must report the unsupported platform");
        };
        assert!(message.contains("only available on macOS"));
        assert!(message.contains("OP_SERVICE_ACCOUNT_TOKEN"));
    }
}
