//! Zsh's dynamic completion protocol, with option names offered only after a dash.

use std::ffi::OsString;
use std::io::{self, Write};
use std::path::Path;

use clap_complete::engine::CompletionCandidate;
use clap_complete::env::EnvCompleter;

pub struct ZshCompletion;

/// Private clap tag carried to zsh as a leading tab, outside the escaped value/help.
pub const CONTINUE_WORD: &str = "kurama-continue-word";

/// The zsh script that decodes kurama's candidate protocol.
///
/// This is shared by `kurama init zsh` and clap's `COMPLETE=zsh` registration
/// path. The latter must not use clap's generic registration script because
/// kurama adds a leading-tab and escaping protocol of its own.
///
/// The completing child's stderr goes to `KURAMA_COMPLETION_STDERR` when it
/// is set, and to `/dev/null` otherwise: a person pressing Tab must never see
/// a diagnostic, but the scenarios point the variable at a file and fail when
/// anything lands in it, so a broken completion cannot pass unnoticed.
pub fn zsh_completions(binary: &Path) -> String {
    ZSH_COMPLETION.replace("{binary}", &quote_binary(binary))
}

const ZSH_COMPLETION: &str = r#"#compdef kurama
_kurama() {
    local -a regular=() nospace=()
    local -a completion_words=("${(@Q)words}")
    completion_words[CURRENT]="$PREFIX"
    local completion
    local suffix_pattern='^([^:\\]|\\.)*[=/](:|$)'
    local -a completions=("${(@f)$(_CLAP_IFS=$'\n' _CLAP_COMPLETE_INDEX=$((CURRENT - 1)) COMPLETE=zsh \
        command {binary} -- "${completion_words[@]}" 2>>"${KURAMA_COMPLETION_STDERR:-/dev/null}")}")
    for completion in "${completions[@]}"; do
        [[ -n "$completion" ]] || continue
        if [[ "$completion" == $'\t'* ]]; then
            nospace+=("${completion#$'\t'}")
        elif [[ "$completion" =~ $suffix_pattern ]]; then
            nospace+=("$completion")
        else
            regular+=("$completion")
        fi
    done
    local result=1
    if (( ${#nospace} )); then
        _describe -V 'values' nospace -S '' && result=0
    fi
    if (( ${#regular} )); then
        local -a suffix_options=()
        [[ -n $QISUFFIX ]] && suffix_options=(-S '')
        _describe -V 'values' regular "${suffix_options[@]}" && result=0
    fi
    return $result
}
if [[ "$funcstack[1]" == "_kurama" ]]; then
    _kurama "$@"
else
    compdef _kurama kurama
fi
"#;

pub fn quote_binary(binary: &Path) -> String {
    format!("'{}'", binary.to_string_lossy().replace('\'', "'\\''"))
}

impl EnvCompleter for ZshCompletion {
    fn name(&self) -> &'static str {
        "zsh"
    }

    fn is(&self, name: &str) -> bool {
        name == self.name()
    }

    fn write_registration(
        &self,
        _var: &str,
        _name: &str,
        _bin: &str,
        completer: &str,
        buf: &mut dyn Write,
    ) -> io::Result<()> {
        buf.write_all(zsh_completions(Path::new(completer)).as_bytes())
    }

    fn write_complete(
        &self,
        cmd: &mut clap::Command,
        mut args: Vec<OsString>,
        current_dir: Option<&Path>,
        buf: &mut dyn Write,
    ) -> io::Result<()> {
        let index: usize = std::env::var("_CLAP_COMPLETE_INDEX")
            .ok()
            .and_then(|index| index.parse().ok())
            .unwrap_or_default();
        if args.len() == index {
            args.push(OsString::new());
        }
        let empty_word = args.get(index).is_some_and(|word| word.is_empty());
        let candidates = clap_complete::engine::complete(cmd, args, index, current_dir)?;
        let separator = std::env::var("_CLAP_IFS").unwrap_or_else(|_| "\n".into());
        write_candidates(candidates, empty_word, &separator, buf)
    }
}

fn write_candidates(
    candidates: Vec<CompletionCandidate>,
    empty_word: bool,
    separator: &str,
    buf: &mut dyn Write,
) -> io::Result<()> {
    let mut first = true;
    for candidate in candidates {
        // clap marks option names with arg:: IDs. Value candidates, including
        // negative numbers or strings beginning with '-', must remain intact.
        if empty_word && candidate.get_id().is_some_and(|id| id.starts_with("arg::")) {
            continue;
        }
        // One candidate is one record: a value a control character would
        // split (a file name with a line break) cannot be offered at all.
        let value = candidate.get_value().to_string_lossy();
        if value.chars().any(char::is_control) {
            continue;
        }
        if !first {
            write!(buf, "{separator}")?;
        }
        first = false;
        if candidate
            .get_tag()
            .is_some_and(|tag| tag.to_string() == CONTINUE_WORD)
        {
            write!(buf, "\t")?;
        }
        let value = value.replace('\\', "\\\\").replace(':', "\\:");
        write!(buf, "{value}")?;
        if let Some(help) = candidate.get_help() {
            let help: String = help
                .to_string()
                .lines()
                .next()
                .unwrap_or_default()
                .chars()
                .filter(|c| !c.is_control())
                .collect();
            let help = help.replace('\\', "\\\\");
            write!(buf, ":{help}")?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn completion_protocol_marks_structural_candidates_without_changing_the_value() {
        let candidates = vec![
            CompletionCandidate::new(".content")
                .help(Some("array".into()))
                .tag(Some("kurama-continue-word".into())),
            CompletionCandidate::new(".page").help(Some("integer".into())),
        ];
        let mut output = Vec::new();
        write_candidates(candidates, false, "\n", &mut output).unwrap();
        assert_eq!(
            String::from_utf8(output).unwrap(),
            "\t.content:array\n.page:integer"
        );
    }

    #[test]
    fn completion_protocol_keeps_one_record_per_candidate() {
        let candidates = vec![
            CompletionCandidate::new("a\nb").help(Some("injected".into())),
            CompletionCandidate::new("tab\tvalue"),
            CompletionCandidate::new("kept").help(Some("one\rtwo\u{1b}\nthree".into())),
        ];
        let mut output = Vec::new();
        write_candidates(candidates, false, "\n", &mut output).unwrap();
        assert_eq!(String::from_utf8(output).unwrap(), "kept:onetwo");
    }

    #[test]
    fn the_script_sends_the_child_stderr_where_the_environment_says() {
        let script = zsh_completions(Path::new("/usr/local/bin/kurama"));
        assert!(
            script.contains(r#"2>>"${KURAMA_COMPLETION_STDERR:-/dev/null}""#),
            "a person pressing Tab sees nothing, but a scenario can point the \
             variable at a file and read what the child complained about:\n{script}"
        );
    }

    #[test]
    fn completion_protocol_omits_only_options_at_an_empty_word() {
        let mut command = clap::Command::new("test")
            .arg(clap::Arg::new("value").value_parser(["-1", "a:b\\c"]))
            .arg(
                clap::Arg::new("json")
                    .long("json")
                    .action(clap::ArgAction::SetTrue),
            );
        command.build();
        let values =
            clap_complete::engine::complete(&mut command, vec!["test".into(), "".into()], 1, None)
                .unwrap();
        let mut output = Vec::new();
        write_candidates(values, true, "\n", &mut output).unwrap();
        assert_eq!(String::from_utf8(output).unwrap(), "-1\na\\:b\\\\c");
        let flags = clap_complete::engine::complete(
            &mut command,
            vec!["test".into(), "--j".into()],
            1,
            None,
        )
        .unwrap();
        let mut output = Vec::new();
        write_candidates(flags, false, "\n", &mut output).unwrap();
        assert_eq!(String::from_utf8(output).unwrap(), "--json");
    }

    #[test]
    fn registration_uses_kuramas_decoder_for_the_custom_protocol() {
        let mut output = Vec::new();
        ZshCompletion
            .write_registration(
                "COMPLETE",
                "kurama",
                "kurama",
                "/opt/kurama/bin/kurama",
                &mut output,
            )
            .unwrap();
        assert_eq!(
            String::from_utf8(output).unwrap(),
            zsh_completions(Path::new("/opt/kurama/bin/kurama"))
        );
    }
}
