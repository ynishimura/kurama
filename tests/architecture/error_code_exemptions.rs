//! What is wrong with the list of error codes no scenario pins (ARCH-040),
//! given the list, `error_code.rs` and the scenario sources as text, and
//! the inputs that pin the answer. The architecture rule reads it, and so
//! does the harness's own scenario (`tests/scenarios/verification_harness.rs`),
//! which is why it names nothing else of either test binary.

use std::collections::BTreeSet;

/// One exemption: the code, what it is, and why no scenario can pin it.
pub(crate) type Exemption = (&'static str, &'static str, &'static str);

/// Every problem of `exemptions`, in list order.
pub(crate) fn error_code_exemption_problems(
    exemptions: &[Exemption],
    error_codes: &str,
    scenarios: &str,
) -> Vec<String> {
    let mut problems = Vec::new();
    let mut seen = BTreeSet::new();
    for (code, what, why_no_scenario) in exemptions {
        if !seen.insert(code) {
            problems.push(format!("{code} is listed twice"));
        }
        if what.trim().is_empty() || why_no_scenario.trim().is_empty() {
            problems.push(format!(
                "{code}: say what it is and why no scenario can pin it"
            ));
        }
        if !error_codes.contains(&format!("=> \"{code}\"")) {
            problems.push(format!("{code} is not an error code of error_code.rs"));
        }
        if scenarios.contains(&format!("\"{code}\"")) {
            problems.push(format!(
                "{code} is pinned by a scenario now; remove the exemption"
            ));
        }
    }
    problems
}

/// An `error_code.rs` with two codes, one of which a scenario expects.
pub(crate) const FIXTURE_ERROR_CODES: &str =
    "Self::Internal => \"INTERNAL\",\nSelf::Pinned => \"PINNED\",\n";
pub(crate) const FIXTURE_SCENARIOS: &str = "v.expect_error(\"PINNED\", 2);\n";

/// A sound exemption: a real code no scenario expects, with its reasons.
pub(crate) const SOUND: [Exemption; 1] = [("INTERNAL", "the fallback", "nothing reaches it")];

/// Repeated, without a reason, unknown to `error_code.rs`, and pinned by a
/// scenario already.
pub(crate) const UNSOUND: [Exemption; 5] = [
    ("INTERNAL", "the fallback", "nothing reaches it"),
    ("INTERNAL", "the fallback", "nothing reaches it"),
    ("INTERNAL", " ", "nothing reaches it"),
    ("GONE", "renamed away", "no scenario"),
    ("PINNED", "pinned now", "was not once"),
];

/// What [`error_code_exemption_problems`] says of [`UNSOUND`].
pub(crate) const UNSOUND_PROBLEMS: [&str; 5] = [
    "INTERNAL is listed twice",
    "INTERNAL is listed twice",
    "INTERNAL: say what it is and why no scenario can pin it",
    "GONE is not an error code of error_code.rs",
    "PINNED is pinned by a scenario now; remove the exemption",
];
