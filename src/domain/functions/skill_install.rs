//! What `kurama agent install` does with each Agent Skill: the directory it
//! goes in, whether the file already there says the same, and the row the
//! report gives it.

use serde::Serialize;

/// The directory of kurama's own Skill.
pub const KURAMA_SKILL: &str = "kurama";

/// The file a Skill directory holds.
pub const SKILL_FILE: &str = "SKILL.md";

/// `kurama-api-<name>`: the directory of an API's Skill, or `None` when
/// the name could not stay one directory under the destination.
pub fn api_skill_directory(api: &str) -> Option<String> {
    let one_segment = !api.is_empty()
        && api != "."
        && api != ".."
        && !api.contains(['/', '\\'])
        && !api.chars().any(char::is_control);
    one_segment.then(|| format!("kurama-api-{api}"))
}

/// What became of one Skill.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SkillStatus {
    /// Written, or to be written in a dry run: nothing was there, or
    /// something else was.
    Written,
    /// The file already holds the same bytes; it is not touched.
    Unchanged,
    /// Nothing written, for the row's `reason`.
    Skipped,
}

impl SkillStatus {
    /// Whether `content` has to be written over what the file holds now
    /// (`None`: no file, or one that cannot be read).
    pub fn of(existing: Option<&[u8]>, content: &str) -> Self {
        if existing == Some(content.as_bytes()) {
            Self::Unchanged
        } else {
            Self::Written
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Written => "written",
            Self::Unchanged => "unchanged",
            Self::Skipped => "skipped",
        }
    }
}

/// One Skill of the report.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SkillRow {
    /// The Skill's directory name (`kurama`, `kurama-api-<name>`).
    pub skill: String,
    /// The `[api.*]` it describes; `None` for kurama's own.
    pub api: Option<String>,
    /// Where the SKILL.md is, or would be.
    pub path: Option<String>,
    pub status: SkillStatus,
    /// Why a Skill was skipped.
    pub reason: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_api_skill_is_one_directory_named_after_the_api() {
        assert_eq!(
            api_skill_directory("linear").as_deref(),
            Some("kurama-api-linear")
        );
        assert_eq!(
            api_skill_directory("billing-api-dev").as_deref(),
            Some("kurama-api-billing-api-dev")
        );
        for name in ["", ".", "..", "a/b", "..\\x", "a\nb"] {
            assert_eq!(api_skill_directory(name), None, "{name:?}");
        }
    }

    #[test]
    fn only_a_file_with_other_bytes_is_written() {
        assert_eq!(SkillStatus::of(None, "x"), SkillStatus::Written);
        assert_eq!(SkillStatus::of(Some(b"old"), "x"), SkillStatus::Written);
        assert_eq!(SkillStatus::of(Some(b"x"), "x"), SkillStatus::Unchanged);
    }

    #[test]
    fn a_row_serializes_its_status_in_snake_case() {
        let row = SkillRow {
            skill: "kurama-api-plain".into(),
            api: Some("plain".into()),
            path: None,
            status: SkillStatus::Skipped,
            reason: Some("no description".into()),
        };
        assert_eq!(
            serde_json::to_value(&row).unwrap(),
            serde_json::json!({"skill": "kurama-api-plain", "api": "plain", "path": null,
                "status": "skipped", "reason": "no description"})
        );
        assert_eq!(SkillStatus::Unchanged.as_str(), "unchanged");
    }
}
