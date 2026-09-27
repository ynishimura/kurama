//! `kurama audit [--since DURATION] [--json] | --watch`: the calls the audit log recorded, oldest first and read no configuration, or the activity monitor that shows them as they are appended.

use clap::{Arg, ArgAction, Command};

use crate::adapters::audit_log::{audit_file, read_entries};
use crate::domain::functions::audit::{entries_since, parse_since, render_listing};

pub fn command() -> Command {
    Command::new("audit")
        .about("List the api, exec, db and data calls the audit log recorded")
        .long_about(
            "List the api, exec, db and data calls the audit log recorded, oldest first.\n\n\
             The log is ~/.local/state/kurama/audit.jsonl (and audit.jsonl.1, the generation\n\
             before it). [audit] enabled decides what is recorded: absent, the runs whose\n\
             environment sets KURAMA_AGENT; true, every run; false, none. An entry holds the\n\
             time, the command, the API, source, database or workspace, the method and path\n\
             (never the query), the status, the exit code and the error code, the duration,\n\
             whether an agent ran it, exec's program (never its arguments) and the SHA-256\n\
             of db / data SQL -- never a secret, a header or a body.\n\n\
             --watch opens the activity monitor on a terminal: the calls newest first, each\n\
             new one within a second of its end, a call the [agent] policy refused in its\n\
             own color, and the command that makes the selected call again, to copy.",
        )
        .arg(
            Arg::new("since")
                .long("since")
                .value_name("DURATION")
                .value_hint(clap::ValueHint::Other)
                .value_parser(parse_since)
                .help("Only the calls of the last DURATION: 30m, 12h, 7d"),
        )
        .arg(
            Arg::new("json")
                .long("json")
                .help("Print {\"entries\": [...]} instead of the table")
                .action(ArgAction::SetTrue),
        )
        .arg(
            Arg::new("watch")
                .long("watch")
                .help("Show the calls live on a terminal, as they are appended")
                .conflicts_with_all(["since", "json"])
                .action(ArgAction::SetTrue),
        )
}

pub fn run(since: Option<chrono::Duration>, json: bool) -> anyhow::Result<()> {
    let cutoff = since.map(|duration| chrono::Utc::now() - duration);
    let file = audit_file()?;
    let entries = entries_since(read_entries(&file)?, cutoff);
    if json {
        println!("{}", serde_json::json!({ "entries": entries }));
    } else {
        print!("{}", render_listing(&entries));
    }
    Ok(())
}
