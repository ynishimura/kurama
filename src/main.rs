//! The binary: parse the command line, run it, and on failure print one `error[CODE]` line and exit with its code.

use std::process::ExitCode;

use kurama::{ErrorCode, ZshCompletion, build_command, classify_invocation, run};

fn main() -> ExitCode {
    if std::env::var_os("COMPLETE").is_some() {
        let mut completion = clap_complete::CompleteEnv::with_factory(build_command)
            .bin("kurama")
            .shells(clap_complete::env::Shells(&[&ZshCompletion]));
        if let Ok(binary) = std::env::current_exe() {
            completion = completion.completer(binary.to_string_lossy().into_owned());
        }
        completion.complete();
    }
    run_command()
}

#[tokio::main]
async fn run_command() -> ExitCode {
    // Parse first so `--help`, argument errors and shell integration output
    // never depend on configuration or the AWS environment.
    let args: Vec<_> = std::env::args_os().collect();
    // A JSON client, and any subcommand with the JSON error contract run with
    // `--json`, answers its own usage failures, with its own code, before clap
    // prints the help text meant for a person.
    let client = classify_invocation(&args);
    let client_json = client.as_ref().is_some_and(|client| client.json);
    let reports_usage = client.as_ref().is_some_and(|client| client.reports_usage());
    let matches = match build_command().try_get_matches_from(args) {
        Ok(matches) => matches,
        Err(error) if reports_usage && error.use_stderr() => {
            let kind = client.expect("a client was classified").kind;
            let code = kind.usage_code();
            let message = ErrorCode::usage_message(&error, kind.as_str());
            if client_json {
                eprintln!("{}", code.json_error(&message, kind.usage_hint()));
            } else {
                eprintln!("error[{code}]: {message}");
            }
            return ExitCode::from(code.exit_code());
        }
        Err(error) => error.exit(),
    };

    match run(&matches).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            // One greppable line per failure: `error[CODE]: outer: inner: root`.
            let code = ErrorCode::classify(&error);
            if client_json {
                eprintln!("{}", code.json_error_with_context(&error));
                return ExitCode::from(code.exit_code());
            }
            let message = format!("{error:#}").replace(['\r', '\n'], " ");
            eprintln!("error[{code}]: {message}");
            if let Some(hint) = code.hint(&error) {
                eprintln!("hint: {hint}");
            }
            ExitCode::from(code.exit_code())
        }
    }
}
