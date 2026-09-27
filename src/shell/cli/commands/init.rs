//! `init` / `completions` command output
//!
//! Both commands print zsh code for `eval "$(kurama init zsh)"`; they are
//! pure string generators so shell startup never touches configuration.

use std::path::Path;

use crate::adapters::completion_protocol::{
    quote_binary, zsh_completions as protocol_zsh_completions,
};

/// Wrapper function template; `{binary}` is the quoted path of the binary
/// that printed the script, so `eval "$(target/debug/kurama init zsh)"`
/// switches one shell to a development build.
///
/// The binary writes `export` lines to the file named by `KURAMA_ENV_SCRIPT`
/// and the function sources it into the current shell. The file is written
/// right before exit, so a Ctrl-C leaves at most an empty mode-600 file in
/// `$TMPDIR`.
const ZSH_FUNCTION: &str = r#"
kurama() {
    local env_script exit_code
    env_script="$(mktemp "${TMPDIR:-/tmp}/kurama_env.XXXXXX")" || return 1
    KURAMA_ENV_SCRIPT="$env_script" {binary} "$@"
    exit_code=$?
    [[ -s "$env_script" ]] && source "$env_script"
    rm -f "$env_script"
    return $exit_code
}
"#;

pub fn zsh_completions(binary: &Path) -> String {
    protocol_zsh_completions(binary)
}

/// Everything `eval "$(kurama init zsh)"` needs: completions, then the
/// wrapper bound to `binary`.
pub fn zsh_init_script(binary: &Path) -> String {
    format!(
        "{}{}",
        zsh_completions(binary),
        ZSH_FUNCTION.replace("{binary}", &quote_binary(binary))
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn completions_define_and_register_the_zsh_completer() {
        let script = zsh_completions(Path::new("/usr/local/bin/kurama"));
        assert!(script.starts_with("#compdef kurama"));
        assert!(script.contains("_kurama()"));
        assert!(script.contains("compdef _kurama kurama"));
    }

    #[test]
    fn completions_call_the_absolute_binary_with_the_current_words() {
        let script = zsh_completions(Path::new("/Users/o'brien/my dir/kurama"));
        assert!(script.contains("COMPLETE=zsh"));
        assert!(script.contains("_CLAP_COMPLETE_INDEX=$((CURRENT - 1))"));
        assert!(
            script.contains(
                "command '/Users/o'\\''brien/my dir/kurama' -- \"${completion_words[@]}\""
            )
        );
        assert!(script.contains("_describe -V 'values' nospace -S ''"));
        assert!(script.contains("if [[ \"$funcstack[1]\" == \"_kurama\" ]]"));
    }

    #[test]
    fn init_script_wraps_the_given_binary_with_env_script_sourcing() {
        let script = zsh_init_script(Path::new("/opt/kurama/bin/kurama"));
        assert!(script.contains("kurama() {"));
        assert!(
            script.contains("KURAMA_ENV_SCRIPT=\"$env_script\" '/opt/kurama/bin/kurama' \"$@\"")
        );
        assert!(script.contains("source \"$env_script\""));
    }

    #[test]
    fn init_script_single_quotes_binary_path() {
        let script = zsh_init_script(Path::new("/Users/o'brien/my dir/kurama"));
        assert!(script.contains("'/Users/o'\\''brien/my dir/kurama' \"$@\""));
    }

    #[test]
    fn init_script_includes_completions() {
        let script = zsh_init_script(Path::new("/usr/local/bin/kurama"));
        assert!(script.contains(&zsh_completions(Path::new("/usr/local/bin/kurama"))));
    }
}
