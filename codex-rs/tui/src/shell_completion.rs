//! Complete `!` input with the local interactive shell and read its zsh plugin display state.

#[cfg(unix)]
use std::collections::HashMap;
use std::ops::Range;
#[cfg(unix)]
use std::path::Path;
#[cfg(unix)]
use std::path::PathBuf;
#[cfg(unix)]
use std::sync::Arc;
#[cfg(unix)]
use std::sync::Mutex;
#[cfg(unix)]
use std::sync::OnceLock;
#[cfg(unix)]
use std::sync::atomic::AtomicU64;
#[cfg(unix)]
use std::sync::atomic::Ordering;
#[cfg(unix)]
use std::time::Duration;

#[cfg(unix)]
use codex_utils_pty::ProcessHandle;
#[cfg(unix)]
use codex_utils_pty::TerminalSize;
#[cfg(unix)]
use codex_utils_pty::spawn_pty_process;

#[cfg(unix)]
const COMPLETION_TIMEOUT: Duration = Duration::from_secs(5);

#[cfg(unix)]
#[derive(Clone, Copy)]
enum ShellKind {
    Zsh,
    Bash,
}

#[cfg(unix)]
fn valid_shell_input(line: &str, cursor: usize) -> bool {
    line.len() <= 8_192
        && cursor <= line.len()
        && line.is_char_boundary(cursor)
        && !line.chars().any(char::is_control)
}

/// Values reported by the running zsh plugins for the current `!` line.
#[derive(Clone, Debug)]
pub(crate) struct ShellPreview {
    pub(crate) suggestion: String,
    pub(crate) suggestion_style: String,
    pub(crate) highlights: Vec<ShellHighlight>,
    pub(crate) accepted_highlights: Vec<ShellHighlight>,
}

#[derive(Clone, Debug)]
pub(crate) struct ShellHighlight {
    pub(crate) range: Range<usize>,
    pub(crate) style: String,
}

#[cfg(unix)]
static PREVIEW_SHELL: OnceLock<PreviewManager> = OnceLock::new();
#[cfg(unix)]
static COMPLETION_SHELL: OnceLock<CompletionManager> = OnceLock::new();
#[cfg(unix)]
static HISTORY_GENERATION: AtomicU64 = AtomicU64::new(0);
#[cfg(unix)]
static HISTORY_WRITE_GATE: OnceLock<tokio::sync::Semaphore> = OnceLock::new();

#[cfg(unix)]
struct PreviewManager {
    gate: tokio::sync::Semaphore,
    shell: std::sync::Mutex<Option<PreviewShell>>,
}

#[cfg(unix)]
struct CompletionManager {
    gate: tokio::sync::Semaphore,
    shell: Mutex<Option<CompletionShell>>,
}

#[cfg(unix)]
struct CompletionShell {
    cwd: PathBuf,
    shell: String,
    history_generation: u64,
    last_source_line: String,
    last_source_cursor: usize,
    last_source_generation: u64,
    line: String,
    cursor: usize,
    session: ShellSession,
    _temp_dir: tempfile::TempDir,
    result_path: PathBuf,
    screen: vt100::Parser,
}

/// The shell's edited line and its own rendered completion list.
#[cfg(unix)]
pub(crate) struct CompletionResult {
    pub(crate) source_line: String,
    pub(crate) source_cursor: usize,
    pub(crate) line: String,
    pub(crate) cursor: usize,
    pub(crate) menu: Vec<ShellMenuLine>,
}

#[cfg(not(unix))]
pub(crate) struct CompletionResult {
    pub(crate) source_line: String,
    pub(crate) source_cursor: usize,
    pub(crate) line: String,
    pub(crate) cursor: usize,
    pub(crate) menu: Vec<ShellMenuLine>,
}

/// A line of the shell's completion menu, with selected terminal-cell ranges.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ShellMenuLine {
    pub(crate) text: String,
    pub(crate) selected_cells: Option<Range<usize>>,
}

#[cfg(unix)]
extern "C" fn cleanup_completion_shell() {
    if let Some(manager) = COMPLETION_SHELL.get()
        && let Ok(mut shell) = manager.shell.try_lock()
    {
        *shell = None;
    }
}

#[cfg(unix)]
extern "C" fn cleanup_preview_shell() {
    if let Some(manager) = PREVIEW_SHELL.get()
        && let Ok(mut shell) = manager.shell.try_lock()
    {
        *shell = None;
    }
}
#[cfg(unix)]
static PREVIEW_GENERATION: AtomicU64 = AtomicU64::new(0);

#[cfg(unix)]
struct ShellSession {
    process: ProcessHandle,
    writer: tokio::sync::mpsc::Sender<Vec<u8>>,
    output: Arc<Mutex<Vec<u8>>>,
    output_drain: tokio::task::JoinHandle<()>,
}

#[cfg(unix)]
impl ShellSession {
    async fn start(
        shell: &str,
        args: &[String],
        cwd: &Path,
        env: &HashMap<String, String>,
        capture_output: bool,
    ) -> Option<Self> {
        let spawned = spawn_pty_process(shell, args, cwd, env, &None, TerminalSize::default(), &[])
            .await
            .ok()?;
        let process = spawned.session;
        let mut output = spawned.stdout_rx;
        let captured = Arc::new(Mutex::new(Vec::new()));
        let captured_for_task = Arc::clone(&captured);
        let output_drain = tokio::spawn(async move {
            while let Some(bytes) = output.recv().await {
                if capture_output && let Ok(mut buffer) = captured_for_task.lock() {
                    buffer.extend_from_slice(&bytes);
                    if buffer.len() > 65_536 {
                        let excess = buffer.len() - 65_536;
                        buffer.drain(..excess);
                    }
                }
            }
        });
        let writer = process.writer_sender();
        Some(Self {
            process,
            writer,
            output: captured,
            output_drain,
        })
    }

    fn take_output(&self) -> Option<Vec<u8>> {
        Some(std::mem::take(&mut *self.output.lock().ok()?))
    }
}

#[cfg(unix)]
fn zsh_wrapped_startup_args(
    temp_dir: &Path,
    env: &mut HashMap<String, String>,
    setup: &str,
) -> Option<Vec<String>> {
    let original_zdotdir = env
        .get("ZDOTDIR")
        .filter(|value| !value.is_empty())
        .or_else(|| env.get("HOME"))?
        .clone();
    let zdotdir = temp_dir.join("zdotdir");
    std::fs::create_dir(&zdotdir).ok()?;

    // Run Codex's setup from .zshrc instead of sending it as interactive input, which would make
    // zsh history options treat the bootstrap commands as user input.
    std::fs::write(
        zdotdir.join(".zshenv"),
        r#"typeset -g _codex_wrapper_zdotdir="$ZDOTDIR"
typeset -g _codex_user_zdotdir="$CODEX_ORIGINAL_ZDOTDIR"
if [[ -r "$_codex_user_zdotdir/.zshenv" ]]; then
  ZDOTDIR="$_codex_user_zdotdir"
  source "$_codex_user_zdotdir/.zshenv"
  _codex_user_zdotdir="${ZDOTDIR:-$HOME}"
fi
ZDOTDIR="$_codex_wrapper_zdotdir"
"#,
    )
    .ok()?;
    let mut zshrc = r#"ZDOTDIR="$_codex_user_zdotdir"
if [[ -r "$ZDOTDIR/.zshrc" ]]; then
  source "$ZDOTDIR/.zshrc"
fi
"#
    .to_string();
    zshrc.push_str(setup);
    if !zshrc.ends_with('\n') {
        zshrc.push('\n');
    }
    std::fs::write(zdotdir.join(".zshrc"), zshrc).ok()?;

    env.insert("CODEX_ORIGINAL_ZDOTDIR".to_string(), original_zdotdir);
    env.insert(
        "ZDOTDIR".to_string(),
        zdotdir.to_string_lossy().into_owned(),
    );
    Some(vec!["-i".to_string()])
}

#[cfg(unix)]
fn zsh_startup_args(
    temp_dir: &Path,
    env: &mut HashMap<String, String>,
    setup: &str,
) -> Option<Vec<String>> {
    env.insert(
        "CODEX_PRIVATE_HISTFILE".to_string(),
        temp_dir.join("history").to_string_lossy().into_owned(),
    );
    let private_setup = format!(
        r#"if [[ -n "${{HISTFILE:-}}" && -r "$HISTFILE" ]]; then
  command cat "$HISTFILE" >| "$CODEX_PRIVATE_HISTFILE"
else
  : >| "$CODEX_PRIVATE_HISTFILE"
fi
HISTFILE="$CODEX_PRIVATE_HISTFILE"
SAVEHIST=0
fc -p "$HISTFILE"
fc -R "$HISTFILE"
{setup}"#
    );
    zsh_wrapped_startup_args(temp_dir, env, &private_setup)
}

#[cfg(unix)]
fn bash_wrapped_startup_args(temp_dir: &Path, setup: &str) -> Option<Vec<String>> {
    let rc_path = temp_dir.join("bashrc");
    let mut bashrc = r#"if [[ -r "$HOME/.bashrc" ]]; then
  source "$HOME/.bashrc"
fi
"#
    .to_string();
    bashrc.push_str(setup);
    if !bashrc.ends_with('\n') {
        bashrc.push('\n');
    }
    std::fs::write(&rc_path, bashrc).ok()?;
    Some(vec![
        "--rcfile".to_string(),
        rc_path.to_string_lossy().into_owned(),
        "-i".to_string(),
    ])
}

#[cfg(unix)]
fn bash_startup_args(
    temp_dir: &Path,
    env: &mut HashMap<String, String>,
    setup: &str,
) -> Option<Vec<String>> {
    env.insert(
        "CODEX_PRIVATE_HISTFILE".to_string(),
        temp_dir.join("history").to_string_lossy().into_owned(),
    );
    let private_setup = format!(
        r#"if [[ -n "${{HISTFILE:-}}" && -r "$HISTFILE" ]]; then
  command cat "$HISTFILE" >| "$CODEX_PRIVATE_HISTFILE"
else
  : >| "$CODEX_PRIVATE_HISTFILE"
fi
HISTFILE="$CODEX_PRIVATE_HISTFILE"
history -c
history -r "$HISTFILE"
{setup}"#
    );
    bash_wrapped_startup_args(temp_dir, &private_setup)
}

/// Add an accepted `!` command to the current shell's real history using that shell's own history
/// implementation. Hidden preview and completion sessions use private copies and are restarted
/// after this finishes so the new entry is immediately available to plugins and completers.
#[cfg(unix)]
pub(crate) async fn record_shell_history(command: &str, cwd: &Path) {
    if command.is_empty() {
        return;
    }

    let gate = HISTORY_WRITE_GATE.get_or_init(|| tokio::sync::Semaphore::new(1));
    let Ok(_permit) = gate.acquire().await else {
        return;
    };
    // Mark existing hidden shells stale before and after the write. If a request races with the
    // short-lived writer, the second increment makes that newly started session stale too.
    HISTORY_GENERATION.fetch_add(1, Ordering::Relaxed);
    let _ = record_shell_history_inner(command, cwd).await;
    HISTORY_GENERATION.fetch_add(1, Ordering::Relaxed);
}

#[cfg(unix)]
async fn record_shell_history_inner(command: &str, cwd: &Path) -> Option<()> {
    let shell = std::env::var("SHELL")
        .ok()
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "zsh".to_string());
    let shell_kind = match Path::new(&shell).file_name()?.to_str()? {
        "zsh" => ShellKind::Zsh,
        "bash" => ShellKind::Bash,
        _ => return None,
    };
    let temp_dir = tempfile::tempdir().ok()?;
    let mut env: HashMap<String, String> = std::env::vars().collect();
    env.insert("CODEX_HISTORY_ENTRY".to_string(), command.to_string());
    let setup = match shell_kind {
        ShellKind::Zsh => {
            r#"if [[ -n "${HISTFILE:-}" ]]; then
  print -sr -- "$CODEX_HISTORY_ENTRY"
  fc -AI "$HISTFILE"
  SAVEHIST=0
  unset HISTFILE
fi
builtin exit
"#
        }
        ShellKind::Bash => {
            r#"if [[ -n "${HISTFILE:-}" ]]; then
  history -s "$CODEX_HISTORY_ENTRY"
  history -a
  HISTFILE=
fi
builtin exit
"#
        }
    };
    let args = match shell_kind {
        ShellKind::Zsh => zsh_wrapped_startup_args(temp_dir.path(), &mut env, setup)?,
        ShellKind::Bash => bash_wrapped_startup_args(temp_dir.path(), setup)?,
    };
    let session = ShellSession::start(&shell, &args, cwd, &env, /*capture_output*/ false).await?;
    tokio::time::timeout(COMPLETION_TIMEOUT, async {
        while !session.process.has_exited() {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .ok()?;
    Some(())
}

#[cfg(not(unix))]
pub(crate) async fn record_shell_history(_command: &str, _cwd: &std::path::Path) {}

#[cfg(unix)]
impl Drop for ShellSession {
    fn drop(&mut self) {
        self.process.terminate();
        self.output_drain.abort();
    }
}

#[cfg(unix)]
struct PreviewShell {
    cwd: PathBuf,
    history_generation: u64,
    session: ShellSession,
    _temp_dir: tempfile::TempDir,
    input_path: PathBuf,
    result_path: PathBuf,
}

/// Skip outdated requests when the user types another character before the shell responds.
#[cfg(unix)]
pub(crate) fn next_preview_generation() -> u64 {
    PREVIEW_GENERATION.fetch_add(1, Ordering::Relaxed) + 1
}

#[cfg(not(unix))]
pub(crate) fn next_preview_generation() -> u64 {
    0
}

/// Ask the current container's interactive zsh to run its loaded plugins on `line`.
#[cfg(unix)]
pub(crate) async fn preview(
    line: &str,
    cursor: usize,
    cwd: &Path,
    generation: u64,
) -> Option<ShellPreview> {
    if line.is_empty() || !valid_shell_input(line, cursor) {
        return None;
    }
    tokio::time::sleep(Duration::from_millis(75)).await;
    if generation != PREVIEW_GENERATION.load(Ordering::Relaxed) {
        return None;
    }
    let shell = std::env::var("SHELL")
        .ok()
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "zsh".to_string());
    if Path::new(&shell).file_name()?.to_str()? != "zsh" {
        return None;
    }
    let manager = PREVIEW_SHELL.get_or_init(|| {
        // Static values are not dropped on process exit, so close the shell and its temp files.
        unsafe { libc::atexit(cleanup_preview_shell) };
        PreviewManager {
            gate: tokio::sync::Semaphore::new(1),
            shell: std::sync::Mutex::new(None),
        }
    });
    let _permit = manager.gate.acquire().await.ok()?;
    if generation != PREVIEW_GENERATION.load(Ordering::Relaxed) {
        return None;
    }
    let mut session = manager.shell.lock().ok()?.take();
    let history_generation = HISTORY_GENERATION.load(Ordering::Relaxed);
    if session.as_ref().is_none_or(|session| {
        session.cwd != cwd
            || session.history_generation != history_generation
            || session.session.process.has_exited()
    }) {
        session = Some(PreviewShell::start(&shell, cwd).await?);
    }
    let result = session.as_mut()?.snapshot(line, cursor).await;
    if result.is_some() {
        *manager.shell.lock().ok()? = session;
    }
    result.filter(|_| generation == PREVIEW_GENERATION.load(Ordering::Relaxed))
}

#[cfg(not(unix))]
pub(crate) async fn preview(
    _line: &str,
    _cursor: usize,
    _cwd: &std::path::Path,
    _generation: u64,
) -> Option<ShellPreview> {
    None
}

#[cfg(unix)]
impl PreviewShell {
    async fn start(shell: &str, cwd: &Path) -> Option<Self> {
        let temp_dir = tempfile::tempdir().ok()?;
        let input_path = temp_dir.path().join("input");
        let result_path = temp_dir.path().join("result");
        let mut env: HashMap<String, String> = std::env::vars().collect();
        env.insert(
            "CODEX_PREVIEW_INPUT_FILE".to_string(),
            input_path.to_string_lossy().into_owned(),
        );
        env.insert(
            "CODEX_PREVIEW_RESULT_FILE".to_string(),
            result_path.to_string_lossy().into_owned(),
        );
        let args = zsh_startup_args(
            temp_dir.path(),
            &mut env,
            include_str!("shell_preview_setup.zsh"),
        )?;
        let session =
            ShellSession::start(shell, &args, cwd, &env, /*capture_output*/ false).await?;
        let result = Self {
            cwd: cwd.to_path_buf(),
            history_generation: HISTORY_GENERATION.load(Ordering::Relaxed),
            _temp_dir: temp_dir,
            input_path,
            result_path,
            session,
        };
        tokio::time::timeout(COMPLETION_TIMEOUT, async {
            loop {
                if std::fs::read(&result.result_path).ok().as_deref() == Some(b"R") {
                    return Some(());
                }
                if result.session.process.has_exited() {
                    return None;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .ok()
        .flatten()?;
        Some(result)
    }

    async fn snapshot(&mut self, line: &str, cursor: usize) -> Option<ShellPreview> {
        let shell_cursor = line[..cursor].chars().count();
        let mut input = line.as_bytes().to_vec();
        input.push(0);
        input.extend_from_slice(shell_cursor.to_string().as_bytes());
        input.push(0);
        std::fs::write(&self.input_path, input).ok()?;
        std::fs::write(&self.result_path, b"R").ok()?;
        self.session.writer.send(b"\x18\x06".to_vec()).await.ok()?;
        tokio::time::timeout(COMPLETION_TIMEOUT, async {
            loop {
                if let Ok(bytes) = std::fs::read(&self.result_path)
                    && let Some((reported_line, preview)) = parse_preview(&bytes)
                    && reported_line == line
                {
                    return Some(preview);
                }
                if self.session.process.has_exited() {
                    return None;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .ok()
        .flatten()
    }
}

#[cfg(unix)]
fn parse_preview(bytes: &[u8]) -> Option<(String, ShellPreview)> {
    let mut fields = bytes.strip_prefix(b"P\0")?.split(|byte| *byte == 0);
    let line = String::from_utf8(fields.next()?.to_vec()).ok()?;
    let cursor = std::str::from_utf8(fields.next()?)
        .ok()?
        .parse::<usize>()
        .ok()?;
    let suggestion = String::from_utf8(fields.next()?.to_vec()).ok()?;
    let suggestion_style = std::str::from_utf8(fields.next()?).ok()?.to_string();
    if line.len() > 8_192 || suggestion.len() > 8_192 || suggestion.chars().any(char::is_control) {
        return None;
    }
    let rest = fields.collect::<Vec<_>>();
    if rest.len() < 3
        || rest[rest.len() - 2] != b"E"
        || !rest.last()?.is_empty()
        || rest.len() > 515
    {
        return None;
    }
    let regions = &rest[..rest.len() - 2];
    let accepted_marker = regions.iter().position(|field| *field == b"A")?;
    let highlights = parse_highlights(&regions[..accepted_marker], &line)?;
    let accepted_line = format!("{line}{suggestion}");
    let accepted_highlights = parse_highlights(&regions[accepted_marker + 1..], &accepted_line)?;
    if cursor > line.chars().count() {
        return None;
    }
    Some((
        line,
        ShellPreview {
            suggestion,
            suggestion_style,
            highlights,
            accepted_highlights,
        },
    ))
}

#[cfg(unix)]
fn parse_highlights(raw_regions: &[&[u8]], line: &str) -> Option<Vec<ShellHighlight>> {
    let mut highlights = Vec::new();
    for raw in raw_regions {
        let raw = std::str::from_utf8(raw).ok()?;
        if !raw.contains("memo=zsh-syntax-highlighting") {
            continue;
        }
        let mut parts = raw.splitn(3, ' ');
        let start = parts.next()?.parse::<usize>().ok()?;
        let end = parts.next()?.parse::<usize>().ok()?;
        let style = parts.next()?.split(" memo=").next()?.trim().to_string();
        let start = char_to_byte(line, start)?;
        let end = char_to_byte(line, end)?;
        if start < end {
            highlights.push(ShellHighlight {
                range: start..end,
                style,
            });
        }
    }
    Some(highlights)
}

#[cfg(unix)]
fn char_to_byte(text: &str, offset: usize) -> Option<usize> {
    if offset == text.chars().count() {
        Some(text.len())
    } else {
        text.char_indices().nth(offset).map(|(byte, _)| byte)
    }
}

/// Return the edited shell line and the menu rendered by the current shell.
#[cfg(unix)]
pub(crate) async fn complete(
    line: &str,
    cursor: usize,
    generation: u64,
    cwd: &Path,
) -> Option<CompletionResult> {
    if !valid_shell_input(line, cursor) {
        return None;
    }

    let shell = std::env::var("SHELL")
        .ok()
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "zsh".to_string());
    let shell_kind = match Path::new(&shell).file_name()?.to_str()? {
        "zsh" => ShellKind::Zsh,
        "bash" => ShellKind::Bash,
        _ => return None,
    };
    let manager = COMPLETION_SHELL.get_or_init(|| {
        unsafe { libc::atexit(cleanup_completion_shell) };
        CompletionManager {
            gate: tokio::sync::Semaphore::new(1),
            shell: Mutex::new(None),
        }
    });
    let _permit = manager.gate.acquire().await.ok()?;
    let mut active = manager.shell.lock().ok()?.take();
    let reuse_current = active
        .as_ref()
        .is_some_and(|active| active.line == line && active.cursor == cursor);
    let reuse_generation_source = active.as_ref().is_some_and(|active| {
        active.last_source_line == line
            && active.last_source_cursor == cursor
            && active.last_source_generation == generation
    });
    let restart = active.as_ref().is_none_or(|active| {
        active.cwd != cwd
            || active.shell != shell
            || active.history_generation != HISTORY_GENERATION.load(Ordering::Relaxed)
            || active.last_source_generation != generation
            || active.session.process.has_exited()
            || (!reuse_current && !reuse_generation_source)
    });
    if restart {
        active =
            Some(CompletionShell::start(&shell, shell_kind, cwd, line, cursor, generation).await?);
    }
    // Multiple Tab keypresses can be queued before the first result reaches the composer. In that
    // case they all carry the cycle's original input, but the persistent shell has already moved
    // on. Apply the queued Tab to the shell's current line and report that line as the result's
    // source so the composer can accept each step in order.
    let use_current_line = !restart && !reuse_current && reuse_generation_source;
    let result = active
        .as_mut()?
        .complete(line, cursor, generation, shell_kind, use_current_line)
        .await;
    if result.is_some() {
        *manager.shell.lock().ok()? = active;
    }
    result
}

#[cfg(unix)]
impl CompletionShell {
    async fn start(
        shell: &str,
        shell_kind: ShellKind,
        cwd: &Path,
        origin_line: &str,
        origin_cursor: usize,
        origin_generation: u64,
    ) -> Option<Self> {
        let temp_dir = tempfile::tempdir().ok()?;
        let result_path = temp_dir.path().join("completion");
        let mut env: HashMap<String, String> = std::env::vars().collect();
        env.insert(
            "CODEX_COMPLETION_FILE".to_string(),
            result_path.to_string_lossy().into_owned(),
        );
        // Keep the user's completion widgets, while using a recognizable prompt to omit
        // prompt redraws from the shell's native completion list.
        let setup = match shell_kind {
            ShellKind::Zsh => {
                "typeset -g _codex_original_tab=\"${${(z)$(bindkey '^I')}[-1]}\"; function _codex_complete { zle \"$_codex_original_tab\"; printf 'C\\0%s\\0%s\\0' \"$BUFFER\" \"$CURSOR\" > \"$CODEX_COMPLETION_FILE\"; }; zle -N _codex_complete; bindkey '^I' _codex_complete; zmodload -i zsh/complist; zstyle ':completion:*' menu select; PROMPT='CODEX> '; RPROMPT=; printf '\\033[2J\\033[H'; printf R > \"$CODEX_COMPLETION_FILE\"\n"
            }
            ShellKind::Bash => {
                "function _codex_snapshot { printf 'C\\0%s\\0%s\\0' \"$READLINE_LINE\" \"$READLINE_POINT\" > \"$CODEX_COMPLETION_FILE\"; }; bind -x '\"\\C-x\\C-e\":_codex_snapshot'; PS1='CODEX> '; printf '\\033[2J\\033[H'; printf R > \"$CODEX_COMPLETION_FILE\"\n"
            }
        };
        let args = match shell_kind {
            ShellKind::Zsh => zsh_startup_args(temp_dir.path(), &mut env, setup)?,
            ShellKind::Bash => bash_startup_args(temp_dir.path(), &mut env, setup)?,
        };
        let session = ShellSession::start(shell, &args, cwd, &env, /*capture_output*/ true).await?;
        let result = Self {
            cwd: cwd.to_path_buf(),
            shell: shell.to_string(),
            history_generation: HISTORY_GENERATION.load(Ordering::Relaxed),
            last_source_line: origin_line.to_string(),
            last_source_cursor: origin_cursor,
            last_source_generation: origin_generation,
            line: String::new(),
            cursor: 0,
            session,
            _temp_dir: temp_dir,
            result_path,
            screen: vt100::Parser::new(24, 80, 0),
        };
        tokio::time::timeout(COMPLETION_TIMEOUT, async {
            loop {
                if std::fs::read(&result.result_path).ok().as_deref() == Some(b"R") {
                    return Some(());
                }
                if result.session.process.has_exited() {
                    return None;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .ok()
        .flatten()?;
        Some(result)
    }

    async fn complete(
        &mut self,
        line: &str,
        cursor: usize,
        generation: u64,
        shell_kind: ShellKind,
        use_current_line: bool,
    ) -> Option<CompletionResult> {
        let (source_line, source_cursor) = if use_current_line {
            (self.line.clone(), self.cursor)
        } else {
            (line.to_string(), cursor)
        };
        let new_line = self.line != source_line || self.cursor != source_cursor;
        let _ = self.take_screen_output();
        std::fs::write(&self.result_path, b"R").ok()?;
        let mut input = Vec::new();
        if new_line {
            input.extend_from_slice(source_line.as_bytes());
            for _ in 0..source_line[source_cursor..].chars().count() {
                input.extend_from_slice(b"\x1b[D");
            }
        }
        input.push(b'\t');
        if matches!(shell_kind, ShellKind::Bash) {
            input.extend_from_slice(b"\x18\x05");
        }
        self.session.writer.send(input).await.ok()?;
        let (completed, completed_cursor) = tokio::time::timeout(COMPLETION_TIMEOUT, async {
            loop {
                if let Ok(bytes) = std::fs::read(&self.result_path)
                    && bytes.first() == Some(&b'C')
                    && let Some(result) = parse_result(&bytes, shell_kind)
                {
                    return Some(result);
                }
                if matches!(shell_kind, ShellKind::Zsh) {
                    let updated = self.process_screen_output()?;
                    if updated && self.has_selected_menu_item() {
                        tokio::time::sleep(Duration::from_millis(40)).await;
                        self.process_screen_output()?;
                    }
                    if updated
                        && self.has_selected_menu_item()
                        && let Some((line, cursor)) = self.visible_shell_line()
                        && valid_shell_input(&line, cursor)
                    {
                        return Some((line, cursor));
                    }
                }
                if self.session.process.has_exited() {
                    return None;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .ok()
        .flatten()?;
        // The completion list can be drawn after the snapshot widget returns.
        tokio::time::sleep(Duration::from_millis(80)).await;
        let menu = self.take_screen_output()?;
        self.last_source_line.clone_from(&source_line);
        self.last_source_cursor = source_cursor;
        self.last_source_generation = generation;
        self.line = completed.clone();
        self.cursor = completed_cursor;
        Some(CompletionResult {
            source_line,
            source_cursor,
            line: completed,
            cursor: completed_cursor,
            menu,
        })
    }

    fn take_screen_output(&mut self) -> Option<Vec<ShellMenuLine>> {
        self.process_screen_output()?;
        Some(shell_menu_lines(self.screen.screen()))
    }

    fn process_screen_output(&mut self) -> Option<bool> {
        let output = self.session.take_output()?;
        let updated = !output.is_empty();
        self.screen.process(&output);
        Some(updated)
    }

    fn visible_shell_line(&self) -> Option<(String, usize)> {
        let screen = self.screen.screen();
        let (row, col) = screen.cursor_position();
        if col < 7
            || !screen
                .rows(0, 80)
                .nth(usize::from(row))?
                .starts_with("CODEX> ")
        {
            return None;
        }
        let mut line = screen.contents_between(row, 7, row, col);
        let cursor = line.len();
        line.push_str(self.line.get(self.cursor..)?);
        Some((line, cursor))
    }

    fn has_selected_menu_item(&self) -> bool {
        let screen = self.screen.screen();
        (0..24)
            .any(|row| (0..80).any(|col| screen.cell(row, col).is_some_and(vt100::Cell::inverse)))
    }
}

#[cfg(unix)]
fn shell_menu_lines(screen: &vt100::Screen) -> Vec<ShellMenuLine> {
    let mut lines = screen
        .rows(0, 80)
        .enumerate()
        .filter_map(|(row, raw)| {
            let text = raw.trim();
            if text.is_empty() || text.contains("CODEX>") {
                return None;
            }
            let first_cell = crate::width::display_width(&raw)
                .saturating_sub(crate::width::display_width(raw.trim_start()));
            let text_width = crate::width::display_width(text);
            let mut selected_cells: Option<Range<usize>> = None;
            for col in 0..text_width {
                let inverse = screen
                    .cell(
                        u16::try_from(row).ok()?,
                        u16::try_from(first_cell + col).ok()?,
                    )
                    .is_some_and(vt100::Cell::inverse);
                if inverse {
                    if let Some(range) = &mut selected_cells {
                        range.end = col + 1;
                    } else {
                        selected_cells = Some(col..col + 1);
                    }
                }
            }
            Some(ShellMenuLine {
                text: text.to_string(),
                selected_cells,
            })
        })
        .collect::<Vec<_>>();
    if lines.len() > 8 {
        let selected = lines.iter().position(|line| line.selected_cells.is_some());
        let start = selected
            .map(|selected| selected.saturating_sub(4).min(lines.len() - 8))
            .unwrap_or(lines.len() - 8);
        lines.drain(..start);
        lines.truncate(8);
    }
    lines
}

#[cfg(unix)]
fn parse_result(bytes: &[u8], shell_kind: ShellKind) -> Option<(String, usize)> {
    let mut fields = bytes.strip_prefix(b"C\0")?.split(|byte| *byte == 0);
    let completed = String::from_utf8(fields.next()?.to_vec()).ok()?;
    let shell_cursor = std::str::from_utf8(fields.next()?)
        .ok()?
        .parse::<usize>()
        .ok()?;
    if !fields.next()?.is_empty() || fields.next().is_some() {
        return None;
    }
    let byte_cursor = match shell_kind {
        ShellKind::Zsh => char_to_byte(&completed, shell_cursor)?,
        ShellKind::Bash => shell_cursor,
    };
    valid_shell_input(&completed, byte_cursor).then_some((completed, byte_cursor))
}

#[cfg(not(unix))]
pub(crate) async fn complete(
    _line: &str,
    _cursor: usize,
    _generation: u64,
    _cwd: &std::path::Path,
) -> Option<CompletionResult> {
    None
}

#[cfg(all(test, unix))]
mod tests {
    use super::ShellKind;
    use super::ShellMenuLine;
    use super::parse_preview;
    use super::parse_result;
    use super::shell_menu_lines;

    #[test]
    fn converts_zsh_character_cursor_and_rejects_invalid_bash_byte_cursor() {
        assert_eq!(
            parse_result(concat!("C\0écho\0", "1\0").as_bytes(), ShellKind::Zsh),
            Some(("écho".to_string(), 2))
        );
        assert_eq!(
            parse_result(concat!("C\0écho\0", "1\0").as_bytes(), ShellKind::Bash),
            None
        );
    }

    #[test]
    fn parses_zsh_plugin_snapshot_with_unicode_ranges() {
        let (_, preview) = parse_preview(
            b"P\0\xc3\xa9cho h\x006\0ello-world\0fg=8\x000 4 fg=green memo=zsh-syntax-highlighting\x005 6 none memo=zsh-syntax-highlighting\0A\x000 16 fg=green memo=zsh-syntax-highlighting\0E\0",
        )
        .expect("valid plugin snapshot");
        assert_eq!(preview.suggestion, "ello-world");
        assert_eq!(preview.suggestion_style, "fg=8");
        assert_eq!(preview.highlights[0].range, 0..5);
        assert_eq!(preview.highlights[0].style, "fg=green");
        assert_eq!(preview.highlights[1].range, 6..7);
        assert_eq!(preview.accepted_highlights[0].range, 0..17);
        assert_eq!(preview.accepted_highlights[0].style, "fg=green");
    }

    #[test]
    fn preserves_shell_menu_selection() {
        let mut screen = vt100::Parser::new(24, 80, 0);
        screen.process(b"echo  \x1b[7mechotc\x1b[27m  echoti");
        assert_eq!(
            shell_menu_lines(screen.screen()),
            vec![ShellMenuLine {
                text: "echo  echotc  echoti".to_string(),
                selected_cells: Some(6..12),
            }]
        );
    }

    #[test]
    fn keeps_selected_shell_menu_line_in_visible_window() {
        let mut screen = vt100::Parser::new(12, 80, 0);
        screen.process(
            b"item0\r\n\x1b[7mitem1\x1b[27m\r\nitem2\r\nitem3\r\nitem4\r\nitem5\r\nitem6\r\nitem7\r\nitem8\r\nitem9",
        );
        let lines = shell_menu_lines(screen.screen());
        assert_eq!(lines.len(), 8);
        assert_eq!(lines[1].text, "item1");
        assert_eq!(lines[1].selected_cells, Some(0..5));
    }
}
