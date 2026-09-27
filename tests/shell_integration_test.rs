//! `eval "$(kurama init zsh)"` runs at every shell startup, so the command
//! must succeed without valid configuration and must keep stderr silent.

use std::process::Command;

fn init_zsh() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_kurama"));
    command.args(["init", "zsh"]);
    command
}

#[test]
fn init_zsh_ignores_broken_config_and_aws_environment() {
    let dir = tempfile::tempdir().unwrap();
    let config = dir.path().join("config.toml");
    std::fs::write(&config, "[aws.session_cache]\nduration = 1\n").unwrap();

    let output = init_zsh()
        .env("KURAMA_CONFIG_PATH", &config)
        .env("AWS_ACCESS_KEY_ID", "AKIATEST")
        .output()
        .unwrap();

    assert!(output.status.success());
    assert!(
        output.stderr.is_empty(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("kurama() {"));
    // The wrapper calls the binary that printed it, so a development build
    // can be tried with `eval "$(target/debug/kurama init zsh)"`.
    assert!(stdout.contains(env!("CARGO_BIN_EXE_kurama")));
}

#[cfg(target_os = "macos")]
#[test]
fn init_zsh_defines_function_and_completion_when_evaluated() {
    let script = String::from_utf8(init_zsh().output().unwrap().stdout).unwrap();

    let output = Command::new("zsh")
        .args([
            "-fc",
            "autoload -Uz compinit; compinit -D; eval \"$KURAMA_INIT\"; \
             whence -w kurama _kurama; print -r -- \"completer=${_comps[kurama]}\"",
        ])
        .env("KURAMA_INIT", script)
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("kurama: function"), "{stdout}");
    assert!(stdout.contains("_kurama: function"), "{stdout}");
    assert!(stdout.contains("completer=_kurama"), "{stdout}");
}
