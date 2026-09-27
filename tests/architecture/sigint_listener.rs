//! Whether a `tokio::signal::ctrl_c()` listener exists before the work it
//! stops starts (ARCH-043), and the shapes that pin the answer. The
//! architecture rule reads it, and so does the harness's own scenario
//! (`tests/scenarios/verification_harness.rs`), which is why it names
//! nothing else of either test binary.

/// The lines (from 1) of `source` whose `ctrl_c()` may start listening only
/// after the work has started. The one shape accepted is
/// `let <name> = tokio::signal::ctrl_c();` followed, with no `.await` and no
/// `spawn(` in between, by a `select!` whose block polls `&mut <name>`.
///
/// It reads lines, not syntax: a comment line is skipped, and whether the
/// work itself is an arm of that `select!` is not checked.
pub(crate) fn late_ctrl_c_listeners(source: &str) -> Vec<usize> {
    let lines: Vec<&str> = source
        .lines()
        .map(|line| {
            if line.trim_start().starts_with("//") {
                ""
            } else {
                line
            }
        })
        .collect();
    (0..lines.len())
        .filter(|&at| lines[at].contains("signal::ctrl_c"))
        .filter(|&at| !first_polled_with_the_work(&lines, at))
        .map(|at| at + 1)
        .collect()
}

fn first_polled_with_the_work(lines: &[&str], at: usize) -> bool {
    let Some(name) = lines[at]
        .trim()
        .strip_prefix("let ")
        .and_then(|rest| rest.strip_suffix(" = tokio::signal::ctrl_c();"))
        .map(|name| name.trim_start_matches("mut "))
    else {
        return false;
    };
    let Some(select) = (at + 1..lines.len()).find(|&next| {
        lines[next].contains("select!")
            || lines[next].contains(".await")
            || lines[next].contains("spawn(")
    }) else {
        return false;
    };
    if !lines[select].contains("select!") {
        return false;
    }
    let polled = format!("&mut {name}");
    let mut depth = 0;
    for line in &lines[select..] {
        if line.match_indices(&polled).any(|(start, _)| {
            !line[start + polled.len()..].starts_with(|c: char| c.is_alphanumeric() || c == '_')
        }) {
            return true;
        }
        depth += line.matches('{').count() as i64 - line.matches('}').count() as i64;
        if depth <= 0 && line.contains('}') {
            return false;
        }
    }
    false
}

/// Late listeners: awaited after the work was spawned, bound but polled only
/// after the work was awaited or spawned, bound but never polled by the
/// `select!` that follows, and created afresh inside a `select!` arm.
pub(crate) const LATE_LISTENERS: [&str; 5] = [
    "let handle = tokio::spawn(work);\ntokio::signal::ctrl_c().await?;\nhandle.abort();\n",
    "let interrupt = tokio::signal::ctrl_c();\nlet rows = work.await?;\ntokio::select! {\n    _ = &mut interrupt => stop(),\n}\n",
    "let interrupt = tokio::signal::ctrl_c();\nlet handle = tokio::spawn(work);\ntokio::select! {\n    _ = &mut interrupt => handle.abort(),\n}\n",
    "let interrupt = tokio::signal::ctrl_c();\ntokio::pin!(interrupt);\ntokio::select! {\n    result = &mut work => result,\n}\ninterrupt.await;\n",
    "loop {\n    tokio::select! {\n        result = &mut work => break result,\n        _ = tokio::signal::ctrl_c() => stop(),\n    }\n}\n",
];

/// The shape `src/shell/cli/commands/db.rs` uses: the listener is bound and
/// pinned, and the first thing to poll it is the `select!` that polls the
/// work, with nothing awaited or spawned in between.
pub(crate) const REGISTERED_WITH_THE_WORK: &str = "\
let mut work = std::pin::pin!(work);
// `tokio::signal::ctrl_c()` registers when first polled.
let interrupt = tokio::signal::ctrl_c();
tokio::pin!(interrupt);
loop {
    tokio::select! {
        result = &mut work => break result,
        _ = tokio::time::sleep_until(deadline), if ended.is_none() => {
            cancel.stop().await;
        }
        _ = &mut interrupt, if ended.is_none() => {
            cancel.stop().await;
        }
    }
}
";
