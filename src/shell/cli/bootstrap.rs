//! Process bootstrap for commands that need configuration: load `config.toml`
//! and initialize logging.
//!
//! `init` and `agent` skip this (see `CliCommand::needs_bootstrap`)
//! because they run at every shell startup.

use anyhow::Result;
use tracing::debug;
use tracing_subscriber::{EnvFilter, fmt, prelude::*};

use crate::adapters::config::Config;

pub async fn bootstrap() -> Result<Config> {
    let config = Config::load().await?;
    init_tracing(&config);
    debug!("Starting kurama v{}", env!("CARGO_PKG_VERSION"));
    Ok(config)
}

/// Log filter, most specific source first: `RUST_LOG`, `[core] log_level`,
/// then `warn` on a terminal and `info` when piped.
fn log_filter(config: &Config, rust_log: Option<&str>, tty: bool) -> String {
    if let Some(filter) = rust_log {
        return filter.to_string();
    }
    if let Some(level) = &config.core.log_level {
        return format!("kurama={level}");
    }
    if tty {
        "kurama=warn".to_string()
    } else {
        "kurama=info".to_string()
    }
}

fn init_tracing(config: &Config) {
    let rust_log = std::env::var("RUST_LOG").ok();
    let filter = EnvFilter::new(log_filter(
        config,
        rust_log.as_deref(),
        crate::shell::tui::terminal::stdout_is_interactive(),
    ));

    // Logs go to stderr (held while the TUI owns the terminal, see
    // `crate::console`) so stdout stays reserved for the export script / JSON.
    // `KURAMA_LOG_FORMAT=json` prints one JSON object per line with the
    // module path in `target`, so a log line points at the source module
    // without further searching.
    let json_format = std::env::var("KURAMA_LOG_FORMAT").as_deref() == Ok("json");
    let registry = tracing_subscriber::registry().with(filter);
    if json_format {
        registry
            .with(
                fmt::layer()
                    .json()
                    .with_writer(crate::console::LogWriter::new())
                    .with_target(true)
                    .with_file(true)
                    .with_line_number(true),
            )
            .init();
    } else {
        // Piped stderr, dumb terminals and a run declared non-interactive
        // must not receive ANSI sequences, including logs emitted before the
        // TUI's terminal check.
        registry
            .with(
                fmt::layer()
                    .with_writer(crate::console::LogWriter::new())
                    .with_ansi(crate::shell::tui::terminal::rewrites_stderr_lines())
                    .with_target(false)
                    .with_thread_ids(false)
                    .with_thread_names(false)
                    .with_file(false)
                    .with_line_number(false),
            )
            .init();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rust_log_wins_then_config_then_tty() {
        let mut config = Config::default();
        assert_eq!(log_filter(&config, Some("trace"), true), "trace");
        config.core.log_level = Some("error".to_string());
        assert_eq!(log_filter(&config, None, true), "kurama=error");
        config.core.log_level = None;
        assert_eq!(log_filter(&config, None, true), "kurama=warn");
        assert_eq!(log_filter(&config, None, false), "kurama=info");
    }
}
