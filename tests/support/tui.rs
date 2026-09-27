//! Drive kurama and its real zsh completion through pseudo terminals.
//!
//! `launch` spawns the binary on a PTY of the requested size inside the
//! scenario sandbox (same fakes, same config as the CLI scenarios) and feeds
//! everything it prints into a terminal emulator (`vt100`). A scenario then
//! sends keys, resizes the terminal, waits for text, records checks and
//! captures the screen. Every capture writes `screen.txt`, `screen.ansi` and
//! `screen.png` under `target/agent/tui/<scenario>/<step>/`, the files an
//! agent reads to see what a user would see. `exit` collects the process
//! result and the fake services' records into the same report as a CLI run.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use portable_pty::{Child, CommandBuilder, MasterPty, PtySize, native_pty_system};
use unicode_width::UnicodeWidthStr;

use super::{
    RESULT_SECRET, RESULT_TOKEN, Run, SESSION_SECRET, SESSION_TOKEN, SOURCE_SECRET, Sandbox,
    Scenario, ScreenRecord, Verification, agent_dir, is_error_line, png,
};

/// Terminal size in columns and rows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Size {
    pub cols: u16,
    pub rows: u16,
}

/// The smallest supported terminal.
pub const SMALL: Size = Size { cols: 80, rows: 24 };
/// The size the screens are designed on.
pub const STANDARD: Size = Size {
    cols: 120,
    rows: 40,
};
/// A wide terminal.
pub const WIDE: Size = Size {
    cols: 160,
    rows: 50,
};

/// How long expected text may take to appear (binary start-up included),
/// unless `KURAMA_TEST_PTY_TIMEOUT_SECS` says otherwise.
const WAIT_TIMEOUT: Duration = Duration::from_secs(10);
/// How long the process may take to exit after the last key, unless
/// `KURAMA_TEST_PTY_TIMEOUT_SECS` says otherwise.
const EXIT_TIMEOUT: Duration = Duration::from_secs(10);
/// Seconds that replace both deadlines on a machine too loaded to draw in
/// time.
const PTY_TIMEOUT_ENV: &str = "KURAMA_TEST_PTY_TIMEOUT_SECS";
/// `limit`, or the seconds `KURAMA_TEST_PTY_TIMEOUT_SECS` holds.
fn pty_timeout(limit: Duration, seconds: Option<&str>) -> Duration {
    seconds.map_or(limit, |seconds| {
        Duration::from_secs(
            seconds.parse().unwrap_or_else(|_| {
                panic!("{PTY_TIMEOUT_ENV} holds whole seconds, not {seconds:?}")
            }),
        )
    })
}

/// [`pty_timeout`] of the environment this test runs in.
fn deadline_of(limit: Duration) -> Duration {
    pty_timeout(limit, std::env::var(PTY_TIMEOUT_ENV).ok().as_deref())
}

/// Whether `done` holds before `limit` has passed. It is asked once more
/// after the deadline: a frame drawn while the deadline passed still counts.
fn poll_until(limit: Duration, mut done: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + limit;
    while Instant::now() < deadline {
        if done() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    done()
}

/// The machine's load averages, for a deadline that passed: a PTY scenario
/// that times out on a loaded machine and passes alone was slow, not wrong.
fn load_average() -> String {
    std::process::Command::new("/usr/sbin/sysctl")
        .args(["-n", "vm.loadavg"])
        .output()
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
        .unwrap_or_else(|error| format!("unknown ({error})"))
}

/// A key's effect is awaited as a screen change, at most this long; a key
/// without a visible effect costs the whole wait.
const SETTLE_TIMEOUT: Duration = Duration::from_secs(2);
/// A frame is complete once the screen stayed the same for this long.
const QUIET_PERIOD: Duration = Duration::from_millis(60);

/// Secrets the fakes hand out; a screen must never show one (the access key
/// id is not a secret and the success modal shows it).
const SCREEN_SECRETS: [&str; 5] = [
    RESULT_SECRET,
    RESULT_TOKEN,
    SESSION_SECRET,
    SESSION_TOKEN,
    SOURCE_SECRET,
];

/// Keys a scenario can press, sent as the byte sequences a terminal emits.
#[derive(Debug, Clone, Copy)]
pub enum Key {
    Up,
    Down,
    Tab,
    Left,
    Right,
    Delete,
    Enter,
    Esc,
    Backspace,
    Home,
    End,
    PageUp,
    PageDown,
    F1,
    Char(char),
    Ctrl(char),
}

impl Key {
    fn bytes(self) -> Vec<u8> {
        match self {
            Key::Up => b"\x1b[A".to_vec(),
            Key::Tab => b"\t".to_vec(),
            Key::Left => b"\x1b[D".to_vec(),
            Key::Right => b"\x1b[C".to_vec(),
            Key::Delete => b"\x1b[3~".to_vec(),
            Key::Down => b"\x1b[B".to_vec(),
            Key::Enter => b"\r".to_vec(),
            Key::Esc => b"\x1b".to_vec(),
            Key::Backspace => b"\x7f".to_vec(),
            Key::Home => b"\x1b[H".to_vec(),
            Key::End => b"\x1b[F".to_vec(),
            Key::PageUp => b"\x1b[5~".to_vec(),
            Key::PageDown => b"\x1b[6~".to_vec(),
            Key::F1 => b"\x1bOP".to_vec(),
            Key::Char(c) => c.to_string().into_bytes(),
            Key::Ctrl(c) => vec![(c.to_ascii_lowercase() as u8) & 0x1f],
        }
    }
}

/// Kills the process when a scenario panics before `exit`.
struct ChildGuard(Box<dyn Child + Send + Sync>);

impl Drop for ChildGuard {
    fn drop(&mut self) {
        if matches!(self.0.try_wait(), Ok(None)) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
}

pub struct Tui {
    id: &'static str,
    feature: &'static str,
    sandbox: Sandbox,
    /// Preparation runs (`then_run`) that ran before the TUI.
    runs: Vec<Run>,
    /// The program label and arguments recorded in the scenario report.
    command: Vec<String>,
    child: ChildGuard,
    master: Box<dyn MasterPty + Send>,
    writer: Box<dyn Write + Send>,
    parser: Arc<Mutex<vt100::Parser>>,
    raw_output: Arc<Mutex<Vec<u8>>>,
    size: Size,
    screens: Vec<ScreenRecord>,
    checks: Vec<(String, bool, String)>,
    artifact_dir: PathBuf,
}

/// Run the scenario's preparation runs, then start the TUI on a terminal of
/// `size`. The scenario's `runs` must not include the TUI itself.
pub fn launch(scenario: Scenario, size: Size) -> Tui {
    launch_command(scenario, size, &[])
}

/// Like `launch`, for a CLI command that needs a terminal (an OAuth `login`
/// that opens the browser): the process runs on the pseudo terminal with
/// `args`, and `exit` / `exit_then_run` collect its result.
pub fn launch_command(scenario: Scenario, size: Size, args: &[&str]) -> Tui {
    launch_program(scenario, size, Sandbox::binary(), "kurama", args, None)
}

/// Like `launch_command`, for a person's screen beside an agent's runs: the
/// scenario's `KURAMA_AGENT` reaches its CLI runs and not the process on the
/// terminal, which is the person's.
pub fn launch_command_for_a_person(scenario: Scenario, size: Size, args: &[&str]) -> Tui {
    launch_program(
        scenario,
        size,
        Sandbox::binary(),
        "kurama",
        args,
        Some("KURAMA_AGENT"),
    )
}

/// Run an isolated real zsh; `-f` skips startup files, so source our setup explicitly.
pub fn launch_zsh(mut scenario: Scenario, size: Size) -> Tui {
    scenario.completion_session = Some(super::CompletionSession::RealZsh);
    let mut terminal = launch_program(
        scenario.with_fake_tools(),
        size,
        "zsh",
        "zsh",
        &["-f", "-i"],
        None,
    );
    terminal
        .wait_for("BOOT>")
        .type_text("source ~/.zshrc")
        .key(Key::Enter);
    terminal.expect_command_line("");
    terminal
}

/// Run an isolated real zsh that evaluates `kurama init zsh` on its prompt,
/// for a scenario that calls the wrapper: unlike `launch_zsh`, the commands
/// typed may call the fake services.
pub fn launch_zsh_wrapper(scenario: Scenario, size: Size) -> Tui {
    let binary = Sandbox::binary().replace('\'', "'\\''");
    let mut terminal = launch_program(scenario, size, "zsh", "zsh", &["-f", "-i"], None);
    terminal
        .wait_for("BOOT>")
        .type_text(&format!(
            "autoload -Uz compinit; compinit -D -u; eval \"$('{binary}' init zsh)\"; PROMPT='K> '"
        ))
        .key(Key::Enter);
    terminal.expect_command_line("");
    terminal
}

/// Create only harness setup before the sandbox captures its file baseline.
pub(super) fn prepare_zsh_home(home: &Path) {
    let binary = Sandbox::binary().replace('\'', "'\\''");
    std::fs::write(home.join(".zshrc"), format!(
        "PROMPT='K> '\nRPROMPT=\nautoload -Uz compinit\ncompinit -D -u\neval \"$('{}' init zsh)\"\nbindkey -e\nbindkey '^[[3~' delete-char\n", binary
    )).unwrap();
}

fn launch_program(
    scenario: Scenario,
    size: Size,
    binary: &str,
    label: &str,
    args: &[&str],
    withheld: Option<&str>,
) -> Tui {
    let mut sandbox = Sandbox::create(&scenario);
    let runs = scenario
        .runs
        .iter()
        .map(|args| sandbox.run_cli(args))
        .collect();
    let artifact_dir = artifact_dir(scenario.id);
    let _ = std::fs::remove_dir_all(&artifact_dir);
    std::fs::create_dir_all(&artifact_dir).unwrap();

    let pty = native_pty_system()
        .openpty(pty_size(size))
        .expect("open a pseudo terminal");
    let mut command = CommandBuilder::new(binary);
    command.args(args);
    command.env_clear();
    command.env("TERM", "xterm-256color");
    for (name, value) in sandbox.env() {
        if Some(name.as_str()) != withheld {
            command.env(name, value);
        }
    }
    if label == "zsh" {
        command.env("ZDOTDIR", &sandbox.home);
        command.env("PROMPT", "BOOT> ");
    }
    command.cwd(&sandbox.home);
    let child = pty
        .slave
        .spawn_command(command)
        .expect("spawn on the pseudo terminal");
    drop(pty.slave);
    let mut reader = pty.master.try_clone_reader().expect("pty reader");
    let writer = pty.master.take_writer().expect("pty writer");

    let parser = Arc::new(Mutex::new(vt100::Parser::new(size.rows, size.cols, 0)));
    let raw_output = Arc::new(Mutex::new(Vec::new()));
    {
        let parser = Arc::clone(&parser);
        let raw_output = Arc::clone(&raw_output);
        std::thread::spawn(move || {
            let mut buffer = [0u8; 8192];
            loop {
                match reader.read(&mut buffer) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        raw_output.lock().unwrap().extend_from_slice(&buffer[..n]);
                        parser.lock().unwrap().process(&buffer[..n]);
                    }
                }
            }
        });
    }

    Tui {
        id: scenario.id,
        feature: scenario.feature,
        sandbox,
        runs,
        command: std::iter::once(label.to_string())
            .chain(args.iter().map(|arg| arg.to_string()))
            .collect(),
        child: ChildGuard(child),
        master: pty.master,
        writer,
        parser,
        raw_output,
        size,
        screens: Vec::new(),
        checks: Vec::new(),
        artifact_dir,
    }
}

fn pty_size(size: Size) -> PtySize {
    PtySize {
        rows: size.rows,
        cols: size.cols,
        pixel_width: 0,
        pixel_height: 0,
    }
}

impl Tui {
    /// The single edited zsh row, retaining the space before the cursor.
    pub fn command_line(&self) -> Option<String> {
        let parser = self.parser.lock().unwrap();
        let screen = parser.screen();
        let (row, column) = screen.cursor_position();
        let text = screen_text(screen);
        let mut line = text.lines().nth(usize::from(row))?.to_string();
        let width = UnicodeWidthStr::width(line.as_str());
        line.extend(std::iter::repeat_n(
            ' ',
            usize::from(column).saturating_sub(width),
        ));
        line.strip_prefix("K> ").map(str::to_string)
    }

    /// Check the edited row, rather than finding the command in old screen output.
    pub fn expect_command_line(&mut self, expected: &str) -> &mut Self {
        let limit = deadline_of(WAIT_TIMEOUT);
        if !poll_until(limit, || self.command_line().as_deref() == Some(expected)) {
            self.fail(&format!(
                "edited command {:?}, expected {expected:?}, not within {limit:?}\nload average: {}",
                self.command_line(),
                load_average()
            ));
        }
        self
    }

    /// Cancel the edited command and terminate zsh through the shared verification path.
    pub fn exit_zsh(mut self) -> Verification {
        self.key(Key::Ctrl('u')).type_text("exit").key(Key::Enter);
        self.exit()
    }

    /// The screen as text: one line per row, trailing spaces removed.
    pub fn screen_text(&self) -> String {
        screen_text(self.parser.lock().unwrap().screen())
    }

    /// Every byte the terminal received so far, before any emulation: the
    /// line discipline's `\r` before each `\n` included.
    pub fn raw_output(&self) -> Vec<u8> {
        self.raw_output.lock().unwrap().clone()
    }

    /// The screen with its styles, for change detection: a key that only
    /// changes a color (a badge toggle) is a visible reaction too.
    fn screen_state(&self) -> Vec<u8> {
        self.parser.lock().unwrap().screen().contents_formatted()
    }

    /// Wait until the screen shows `text`; a scenario failure with the
    /// screen if it does not appear in time.
    pub fn wait_for(&mut self, text: &str) -> &mut Self {
        let text = text.to_string();
        self.wait_until(&format!("{text:?} appears"), move |screen| {
            screen.contains(&text)
        })
    }

    /// Wait until `predicate` holds for the screen text.
    pub fn wait_until(&mut self, what: &str, predicate: impl Fn(&str) -> bool) -> &mut Self {
        let limit = deadline_of(WAIT_TIMEOUT);
        if !poll_until(limit, || predicate(&self.screen_text())) {
            self.fail(&format!(
                "{what}: not within {limit:?}\nload average: {}",
                load_average()
            ));
        }
        self.quiet();
        self
    }

    /// Press one key and wait for the screen to react.
    pub fn key(&mut self, key: Key) -> &mut Self {
        self.write_keys(&key.bytes());
        self
    }

    /// Write input to the terminal and wait for the screen to react to it.
    fn write_keys(&mut self, bytes: &[u8]) {
        let before = self.screen_state();
        self.writer.write_all(bytes).expect("write to the pty");
        self.writer.flush().expect("flush the pty");
        self.settle(&before);
    }

    /// Type text as one write, the way a paste arrives, and settle once. Per
    /// character it settled per character: a 60ms quiet period each, which is
    /// what made a zsh completion scenario that types 380 of them take 40
    /// seconds. Keys that make the program do something -- Tab, Enter, Ctrl-U
    /// -- stay one `key` call each, so what is being waited for is still one
    /// reaction at a time.
    pub fn type_text(&mut self, text: &str) -> &mut Self {
        let bytes: Vec<u8> = text
            .chars()
            .flat_map(|c| Key::Char(c).bytes())
            .collect::<Vec<u8>>();
        self.write_keys(&bytes);
        self
    }

    /// Resize the terminal; the process receives SIGWINCH and redraws.
    /// Scenarios `wait_for` the layout they expect afterwards.
    pub fn resize(&mut self, size: Size) -> &mut Self {
        self.master.resize(pty_size(size)).expect("resize the pty");
        self.parser
            .lock()
            .unwrap()
            .screen_mut()
            .set_size(size.rows, size.cols);
        self.size = size;
        self.quiet();
        self
    }

    /// Record one named check; failures are reported together by `finish`.
    pub fn check(&mut self, name: &str, ok: bool, detail: impl Into<String>) -> &mut Self {
        self.checks.push((name.to_string(), ok, detail.into()));
        self
    }

    /// One check on the screen text; the screen is the detail when it fails.
    fn expect_screen(&mut self, name: String, ok: impl Fn(&str) -> bool) -> &mut Self {
        let screen = self.screen_text();
        let ok = ok(&screen);
        self.check(&name, ok, if ok { String::new() } else { screen })
    }

    pub fn expect_text(&mut self, text: &str) -> &mut Self {
        self.expect_screen(format!("screen shows {text:?}"), |screen| {
            screen.contains(text)
        })
    }

    pub fn expect_no_text(&mut self, text: &str) -> &mut Self {
        self.expect_screen(format!("screen does not show {text:?}"), |screen| {
            !screen.contains(text)
        })
    }

    /// Inspect actual terminal colors, after the backend emitted its escapes.
    pub fn expect_default_colors(&mut self) -> &mut Self {
        let colored = self.colored_cells();
        self.check(
            "all cells use default colors",
            colored == 0,
            format!("{colored} colored cells"),
        )
    }

    pub fn expect_colors(&mut self) -> &mut Self {
        let colored = self.colored_cells();
        self.check(
            "the screen uses colors",
            colored > 0,
            format!("{colored} colored cells"),
        )
    }

    fn colored_cells(&self) -> usize {
        let parser = self.parser.lock().unwrap();
        let screen = parser.screen();
        (0..self.size.rows)
            .flat_map(|row| (0..self.size.cols).map(move |col| (row, col)))
            .filter(|&(row, col)| {
                let cell = screen.cell(row, col).unwrap();
                cell.fgcolor() != vt100::Color::Default || cell.bgcolor() != vt100::Color::Default
            })
            .count()
    }

    /// Color suppression must preserve the emphasis that identifies selection.
    pub fn expect_bold_text(&mut self, text: &str) -> &mut Self {
        let bold = {
            let parser = self.parser.lock().unwrap();
            let screen = parser.screen();
            screen_text(screen).lines().enumerate().any(|(row, line)| {
                line.find(text).is_some_and(|start| {
                    let col = line[..start].width();
                    (col..col + text.width()).all(|col| {
                        screen
                            .cell(row as u16, col as u16)
                            .is_some_and(|cell| cell.bold())
                    })
                })
            })
        };
        self.check(&format!("{text:?} is bold"), bold, self.screen_text())
    }

    /// The terminal is back in its normal state: the main screen, and the
    /// line discipline in canonical mode (raw mode clears `ICANON`). Read
    /// after the process has exited, from the emulator and the PTY itself.
    pub fn expect_restored_terminal(&mut self) -> &mut Self {
        let alternate = self.parser.lock().unwrap().screen().alternate_screen();
        self.check("the terminal left the alternate screen", !alternate, "");
        let flags = self
            .master
            .get_termios()
            .map(|termios| format!("{:?}", termios.local_flags))
            .unwrap_or_default();
        self.check(
            "the terminal is out of raw mode (ICANON set)",
            flags.contains("ICANON"),
            flags,
        )
    }

    /// A file under the sandbox HOME as it is now (empty when absent), for a
    /// scenario that checks what the binary wrote while it still runs.
    pub fn home_file(&self, relative: &str) -> String {
        std::fs::read_to_string(self.sandbox.home.join(relative)).unwrap_or_default()
    }

    /// A rejected dumb terminal must receive no alternate-screen or style codes.
    pub fn expect_plain_output(&mut self) -> &mut Self {
        let plain = !self.raw_output.lock().unwrap().contains(&0x1b);
        self.check("output contains no terminal escape sequences", plain, "")
    }

    /// One row of the screen contains every needle.
    pub fn expect_row_with(&mut self, needles: &[&str]) -> &mut Self {
        self.expect_screen(format!("one row shows all of {needles:?}"), |screen| {
            screen
                .lines()
                .any(|line| needles.iter().all(|needle| line.contains(needle)))
        })
    }

    /// The last screen row (the key hints) contains `text`.
    pub fn expect_footer_contains(&mut self, text: &str) -> &mut Self {
        self.expect_screen(format!("footer shows {text:?}"), |screen| {
            screen.lines().last().unwrap_or_default().contains(text)
        })
    }

    /// Save the screen as `screen.txt`, `screen.ansi` and `screen.png` under
    /// `target/agent/tui/<scenario>/<step>/`. The files are artifacts outside
    /// the sandbox, so the screen is checked for secrets here.
    pub fn snapshot(&mut self, step: &str) -> &mut Self {
        let dir = self.artifact_dir.join(step);
        std::fs::create_dir_all(&dir).unwrap();
        let text = {
            let parser = self.parser.lock().unwrap();
            let screen = parser.screen();
            std::fs::write(dir.join("screen.ansi"), screen.contents_formatted()).unwrap();
            png::write_png(screen, &dir.join("screen.png")).expect("write screen.png");
            screen_text(screen)
        };
        std::fs::write(dir.join("screen.txt"), format!("{text}\n")).unwrap();
        let leaked = leaked_secrets(&text);
        self.check(
            &format!("screen {step:?} shows no secret"),
            leaked.is_empty(),
            format!("found {leaked:?}"),
        );
        self.screens.push(ScreenRecord {
            step: step.to_string(),
            cols: self.size.cols,
            rows: self.size.rows,
            dir: dir
                .strip_prefix(env!("CARGO_MANIFEST_DIR"))
                .unwrap_or(&dir)
                .to_string_lossy()
                .into_owned(),
        });
        self
    }

    fn fail(&mut self, message: &str) -> ! {
        self.snapshot("failure");
        panic!(
            "scenario {}: {message}\nscreen ({}x{}):\n{}",
            self.id,
            self.size.cols,
            self.size.rows,
            self.screen_text()
        );
    }

    /// Wait for the screen to differ from `before`, then for a quiet period
    /// so the frame is complete. The view has no clock or spinner, so a
    /// screen that stays the same is a frame that is done.
    fn settle(&self, before: &[u8]) {
        let deadline = Instant::now() + SETTLE_TIMEOUT;
        while self.screen_state() == before && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        self.quiet();
    }

    /// Wait until the screen stayed the same for a quiet period.
    fn quiet(&self) {
        let deadline = Instant::now() + SETTLE_TIMEOUT;
        let mut seen = self.screen_state();
        loop {
            std::thread::sleep(QUIET_PERIOD);
            let now = self.screen_state();
            if now == seen || Instant::now() > deadline {
                return;
            }
            seen = now;
        }
    }

    /// Run `args` as a CLI run in the same sandbox while the process on the
    /// terminal keeps running: something that screen is expected to notice.
    pub fn run_beside(&mut self, args: &[&str]) -> &mut Self {
        let run = self.sandbox.run_cli(args);
        self.runs.push(run);
        self
    }

    /// Press `q` and collect the result.
    pub fn quit(mut self) -> Verification {
        self.key(Key::Char('q'));
        self.exit()
    }

    /// Wait for the process to end, then report what the terminal showed
    /// after the TUI, the exit code and the fake services' records.
    pub fn exit(self) -> Verification {
        self.exit_then_run(&[])
    }

    /// Wait for the process on the terminal to end, run `runs` as CLI runs
    /// in the same sandbox, then report everything.
    pub fn exit_then_run(mut self, runs: &[&[&str]]) -> Verification {
        let limit = deadline_of(EXIT_TIMEOUT);
        let mut status = None;
        poll_until(limit, || {
            status = self.child.0.try_wait().expect("wait for kurama");
            status.is_some()
        });
        let (exit_code, timed_out) = match status {
            Some(status) => (Some(status.exit_code() as i32), false),
            None => {
                let _ = self.child.0.kill();
                let _ = self.child.0.wait();
                (None, true)
            }
        };
        if timed_out {
            self.checks.push((
                format!("the process exits within {limit:?} of the last key"),
                false,
                format!("load average: {}", load_average()),
            ));
        }
        self.quiet();
        let terminal = self.screen_text();
        let command = self.command.clone();
        // A PTY merges both streams. Attribute a failed command's terminal
        // output to stderr so the common error-line contract can inspect it;
        // this does not prove which descriptor the command wrote to.
        let (stdout, stderr) = if exit_code == Some(0) {
            (terminal.clone(), String::new())
        } else {
            (String::new(), terminal.clone())
        };
        let run = self
            .sandbox
            .record_run(command, exit_code, timed_out, stdout, stderr);
        self.runs.push(run);
        for args in runs {
            let run = self.sandbox.run_cli(args);
            self.runs.push(run);
        }
        let completion_session = self.sandbox.completion_session;
        let completion_stderr = self.sandbox.completion_stderr();
        let completion_output =
            String::from_utf8_lossy(&self.raw_output.lock().unwrap()).into_owned();
        let mut verification = self
            .sandbox
            .finish(self.id, self.feature, self.runs, self.screens);
        if completion_session == Some(super::CompletionSession::RealZsh) {
            // This covers visible diagnostics across the entire PTY capture, including
            // text erased by later redraws. It cannot see stderr redirected by init zsh.
            let diagnostics: Vec<_> = ["error[", " ERROR ", "panicked at", "zsh: "]
                .into_iter()
                .filter(|needle| completion_output.contains(needle))
                .collect();
            verification.check(
                "the completion PTY stream contains no visible error diagnostics",
                diagnostics.is_empty(),
                format!("visible markers: {diagnostics:?}; child exit codes are not observed"),
            );
            // `init zsh` hides the child's stderr from the user, not from us:
            // the scenarios point `KURAMA_COMPLETION_STDERR` at a file.
            verification.check(
                "the completing child wrote nothing to stderr",
                completion_stderr.is_empty(),
                completion_stderr.clone(),
            );
        }
        for (name, ok, detail) in self.checks {
            verification.check(&name, ok, detail);
        }
        let errors: Vec<&str> = terminal
            .lines()
            .filter(|line| is_error_line(line))
            .collect();
        verification.check(
            "the terminal shows no error line after a successful TUI exit",
            exit_code != Some(0) || errors.is_empty(),
            format!("error lines: {errors:?}"),
        );
        verification
    }
}

/// One line per row; a wide character counts once; trailing spaces removed.
pub fn screen_text(screen: &vt100::Screen) -> String {
    let (rows, cols) = screen.size();
    (0..rows)
        .map(|row| {
            let mut line = String::new();
            for col in 0..cols {
                let cell = screen.cell(row, col).expect("cell inside the screen");
                if cell.is_wide_continuation() {
                    continue;
                }
                let contents = cell.contents();
                if contents.is_empty() {
                    line.push(' ');
                } else {
                    line.push_str(contents);
                }
            }
            line.trim_end().to_string()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// The fake secrets that appear in `text`.
fn leaked_secrets(text: &str) -> Vec<&'static str> {
    SCREEN_SECRETS
        .into_iter()
        .filter(|secret| text.contains(secret))
        .collect()
}

/// The scenario's own artifact directory, for assertions on written files.
pub fn artifact_dir(scenario: &str) -> PathBuf {
    agent_dir().join("tui").join(scenario)
}

/// Read a captured `screen.txt`.
pub fn read_screen(dir: &Path) -> String {
    std::fs::read_to_string(dir.join("screen.txt")).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[cfg_attr(not(target_os = "macos"), ignore = "real zsh on macOS")]
    fn completion_session_real_zsh_always_gets_effect_and_file_checks() {
        let terminal = launch_zsh(
            Scenario::tui("completion_session_guard_regression"),
            STANDARD,
        );
        let verification = terminal.exit_zsh();
        for name in [
            "completion makes no service, secret, browser, clipboard or editor calls",
            "no file written under HOME or TMPDIR",
        ] {
            let check = verification.checks.iter().find(|check| check.name == name);
            assert!(
                check.is_some_and(|check| check.ok),
                "missing or failed: {name}"
            );
        }
        for name in [
            "every completion request exits zero",
            "every completion request keeps stderr empty",
        ] {
            assert!(
                verification.checks.iter().all(|check| check.name != name),
                "PTY cannot observe individual completion children: {name}"
            );
        }
    }

    #[test]
    fn the_pty_deadlines_default_to_ten_seconds_and_the_environment_lengthens_them() {
        assert_eq!(WAIT_TIMEOUT, Duration::from_secs(10));
        assert_eq!(EXIT_TIMEOUT, Duration::from_secs(10));
        assert_eq!(pty_timeout(WAIT_TIMEOUT, None), WAIT_TIMEOUT);
        assert_eq!(
            pty_timeout(EXIT_TIMEOUT, Some("45")),
            Duration::from_secs(45)
        );
    }

    #[test]
    fn a_frame_drawn_as_the_deadline_passes_is_read_once_more() {
        let mut reads = 0;
        let found = poll_until(Duration::ZERO, || {
            reads += 1;
            true
        });
        assert!(found && reads == 1, "found {found}, after {reads} reads");
        assert!(!poll_until(Duration::ZERO, || false));
    }

    #[test]
    fn a_secret_on_the_screen_is_reported_and_the_key_id_is_not() {
        assert!(leaked_secrets("Access key  ASIARESULTKEY000001").is_empty());
        assert_eq!(
            leaked_secrets(&format!("Secret {RESULT_SECRET}")),
            [RESULT_SECRET]
        );
    }
}
