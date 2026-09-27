//! The Japanese README follows the English one: the same headings, code
//! blocks and images per section, and each section names the English text it
//! translates by a digest, so an English edit fails until it is translated.

use sha2::{Digest, Sha256};

use crate::support::*;

/// What a section of `README.ja.md` records about the English it translates.
const MARKER_PREFIX: &str = "<!-- en: ";

/// One heading and what follows it up to the next heading; the text before
/// the first heading is a section with an empty heading.
struct Section {
    heading: String,
    text: String,
    code: Vec<String>,
    images: Vec<String>,
    marker: Option<String>,
}

fn sections(markdown: &str) -> Vec<Section> {
    let mut sections = vec![Section {
        heading: String::new(),
        text: String::new(),
        code: Vec::new(),
        images: Vec::new(),
        marker: None,
    }];
    let mut fence: Option<String> = None;
    for line in markdown.lines() {
        let section = sections.last_mut().unwrap();
        if let Some(block) = fence.as_mut() {
            block.push_str(line);
            block.push('\n');
            if line.starts_with("```") {
                section.code.push(fence.take().unwrap());
            }
            section.text.push_str(line);
            section.text.push('\n');
            continue;
        }
        if line.starts_with("```") {
            fence = Some(format!("{line}\n"));
        } else if line.starts_with('#') && line.trim_start_matches('#').starts_with(' ') {
            sections.push(Section {
                heading: line.to_string(),
                text: format!("{line}\n"),
                code: Vec::new(),
                images: Vec::new(),
                marker: None,
            });
            continue;
        } else if let Some(digest) = line
            .strip_prefix(MARKER_PREFIX)
            .and_then(|rest| rest.strip_suffix(" -->"))
        {
            section.marker = Some(digest.to_string());
            continue;
        }
        section.images.extend(
            line.split("src=\"")
                .skip(1)
                .filter_map(|rest| rest.split('"').next())
                .map(|source| source.replace(".ja.", ".")),
        );
        section.text.push_str(line);
        section.text.push('\n');
    }
    sections
}

fn digest(text: &str) -> String {
    Sha256::digest(text.as_bytes())
        .iter()
        .take(6)
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn level(heading: &str) -> usize {
    heading.chars().take_while(|c| *c == '#').count()
}

/// Every way `ja` has fallen behind `en`, one line each, naming the section
/// and what to do.
fn translation_problems(en: &str, ja: &str) -> Vec<String> {
    let (en, ja) = (sections(en), sections(ja));
    if en.len() != ja.len() {
        return vec![format!(
            "README.md has {} sections and README.ja.md {}: add or remove the translated headings",
            en.len(),
            ja.len()
        )];
    }
    let mut problems = Vec::new();
    for (en, ja) in en.iter().zip(&ja) {
        let name = if en.heading.is_empty() {
            "the text before the first heading".to_string()
        } else {
            format!("`{}`", en.heading)
        };
        if level(&en.heading) != level(&ja.heading) {
            problems.push(format!(
                "{name} is translated by `{}`, a heading of another level",
                ja.heading
            ));
        }
        if en.code != ja.code {
            problems.push(format!(
                "{name}: the code blocks differ; README.ja.md keeps them byte for byte"
            ));
        }
        if en.images != ja.images {
            problems.push(format!(
                "{name}: the images differ ({:?} in English, {:?} in Japanese)",
                en.images, ja.images
            ));
        }
        let current = digest(&en.text);
        if ja.marker.as_deref() != Some(current.as_str()) {
            problems.push(format!(
                "{name} changed since README.ja.md translated it: update the translation, then set its marker to `{MARKER_PREFIX}{current} -->`"
            ));
        }
    }
    problems
}

#[test]
fn the_japanese_readme_translates_the_current_english_one() {
    let en = std::fs::read_to_string(root().join("README.md")).unwrap();
    let ja = std::fs::read_to_string(root().join("README.ja.md")).unwrap();
    let problems = translation_problems(&en, &ja);
    assert!(
        problems.is_empty(),
        "ARCH-045: README.ja.md is behind README.md:\n{}",
        problems.join("\n")
    );
}

const EN: &str =
    "intro\n\n## Usage\n\nRun it.\n\n```sh\nkurama env dev\n```\n\n<img src=\"a.png\">\n";

fn ja_for(en: &str, body: &str) -> String {
    let [intro, usage] = sections(en).try_into().ok().unwrap();
    format!(
        "{MARKER_PREFIX}{} -->\nはじめに\n\n## 使い方\n{MARKER_PREFIX}{} -->\n\n{body}",
        digest(&intro.text),
        digest(&usage.text)
    )
}

#[test]
fn an_english_edit_left_untranslated_is_found() {
    let ja = ja_for(
        EN,
        "実行する。\n\n```sh\nkurama env dev\n```\n\n<img src=\"a.png\">\n",
    );
    let edited = EN.replace("Run it.", "Run it once.");
    assert_detected(
        "ARCH-045",
        translation_problems(&edited, &ja)
            .iter()
            .any(|problem| problem.contains("`## Usage` changed")),
        "an English sentence edited after the translation",
    );
}

#[test]
fn a_code_block_or_image_that_differs_is_found() {
    let ja = ja_for(
        EN,
        "実行する。\n\n```sh\nkurama env stg\n```\n\n<img src=\"b.png\">\n",
    );
    let problems = translation_problems(EN, &ja);
    assert_detected(
        "ARCH-045",
        problems
            .iter()
            .any(|problem| problem.contains("code blocks differ")),
        "a code block changed in the translation",
    );
    assert_detected(
        "ARCH-045",
        problems
            .iter()
            .any(|problem| problem.contains("images differ")),
        "an image changed in the translation",
    );
}

#[test]
fn a_heading_missing_from_the_translation_is_found() {
    let ja = format!("{MARKER_PREFIX}{} -->\nはじめに\n", digest("intro\n\n"));
    assert_detected(
        "ARCH-045",
        translation_problems(EN, &ja)
            .iter()
            .any(|problem| problem.contains("2 sections")),
        "a section with no translated heading",
    );
}

#[test]
fn a_current_translation_passes() {
    let ja = ja_for(
        EN,
        "実行する。\n\n```sh\nkurama env dev\n```\n\n<img src=\"a.png\">\n",
    );
    assert_allowed(
        "ARCH-045",
        translation_problems(EN, &ja).is_empty(),
        "a translation of the current English with the same code and images",
    );
    let localized = ja_for(
        EN,
        "実行する。\n\n```sh\nkurama env dev\n```\n\n<img src=\"a.ja.png\">\n",
    );
    assert_allowed(
        "ARCH-045",
        translation_problems(EN, &localized).is_empty(),
        "the Japanese version of an image, named <name>.ja.<ext>",
    );
}
