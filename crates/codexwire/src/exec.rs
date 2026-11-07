//! One-shot `codex exec` runs on a `ChatGPT` login, each in a fresh private
//! `CODEX_HOME`.
//!
//! [`Exec::run`] makes `<tmp_root>/<run>/` (0700) with `home/` (the run's
//! `CODEX_HOME`, holding only a copy of the real `auth.json`), `work/` (its
//! working directory) and `refs/` (the input images), runs `codex exec`
//! there with the prompt on stdin and `--json` events on stdout, and copies
//! the login back when Codex refreshed it ([`crate::auth::copy_back`]'s
//! rule). A run that [`may_refresh`] the login goes alone
//! ([`crate::auth::refresh_lock`]). The [`Run`] it returns holds the run's
//! directory, removed when it is dropped (or [`Run::remove`]).
//!
//! [`failure`] reads why a run failed from its events and log;
//! [`image_prompt`] and [`Run::generated_images`] are for image generation
//! (Codex's image tool writes to `<CODEX_HOME>/generated_images/`).
//!
//! Unix only.

use core::{
    fmt,
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};
use std::{
    path::{Path, PathBuf},
    process::Stdio,
};

use crate::auth::{Copied, may_refresh, mkdir_private, refresh_lock, write_private_atomic};

/// How to run `codex exec`.
#[derive(Clone, Debug)]
pub struct Exec {
    /// The `codex` binary.
    pub command: String,
    /// The real `CODEX_HOME`: only its `auth.json` is used (copied in, and
    /// back when refreshed).
    pub codex_home: PathBuf,
    /// Where the per-run directories go (made 0700 if missing).
    pub tmp_root: PathBuf,
    /// `--model`, if any (else Codex's default).
    pub model: Option<String>,
    /// `--sandbox` (`read-only`, `workspace-write`, `danger-full-access`).
    pub sandbox: String,
    /// Killed after this long.
    pub timeout: Duration,
    /// The child's environment on top of the parent's allowlisted variables
    /// ([`crate::client::child_env`]), e.g. `PATH`. `HOME`, `CODEX_HOME`
    /// and `TMPDIR` are always the run's; `LANG` is `C.UTF-8` unless given.
    pub env: Vec<(String, String)>,
    /// More `codex exec` arguments, before the prompt.
    pub args: Vec<String>,
}

impl Exec {
    /// `command` on the login in `codex_home`, runs under `tmp_root`: a
    /// read-only sandbox, a 5 minute timeout.
    pub fn new(
        command: impl Into<String>,
        codex_home: impl Into<PathBuf>,
        tmp_root: impl Into<PathBuf>,
    ) -> Self {
        Self {
            command: command.into(),
            codex_home: codex_home.into(),
            tmp_root: tmp_root.into(),
            model: None,
            sandbox: "read-only".into(),
            timeout: Duration::from_secs(300),
            env: Vec::new(),
            args: Vec::new(),
        }
    }
}

/// What a run is given.
#[derive(Clone, Debug, Default)]
pub struct Input {
    /// The prompt (on stdin).
    pub prompt: String,
    /// Images to attach (`--image`): a file name (e.g. `ref-1.png`; only
    /// its last component is used) and the bytes.
    pub images: Vec<(String, Vec<u8>)>,
}

/// Why a run didn't start or didn't finish setting up.
#[derive(Debug)]
pub enum ExecError {
    /// The real `auth.json` can't be read (no login, or no access).
    Login(std::io::Error),
    /// Making the run's directory or files (what, and why).
    Setup(&'static str, std::io::Error),
    /// The `codex` binary didn't start.
    Spawn(std::io::Error),
}

impl fmt::Display for ExecError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Login(e) => write!(f, "Codex's auth.json isn't readable: {e}"),
            Self::Setup(what, e) => write!(f, "codex run {what}: {e}"),
            Self::Spawn(e) => write!(f, "codex didn't start: {e}"),
        }
    }
}

impl core::error::Error for ExecError {
    fn source(&self) -> Option<&(dyn core::error::Error + 'static)> {
        match self {
            Self::Login(e) | Self::Setup(_, e) | Self::Spawn(e) => Some(e),
        }
    }
}

/// A finished (or timed out) run. Its directory goes when it is dropped.
#[derive(Debug)]
pub struct Run {
    dir: RunDir,
    /// The exit code; `None` when it timed out and was killed (`-1` when it
    /// died of a signal).
    pub code: Option<i32>,
    /// Its `--json` events.
    pub stdout: Vec<u8>,
    /// Its log.
    pub stderr: Vec<u8>,
    /// `--output-last-message`: the final answer, if it wrote one.
    pub last_message: Vec<u8>,
    /// Whether Codex refreshed the login and it was copied back (or why
    /// copying it back failed).
    pub copied_back: std::io::Result<bool>,
}

impl Run {
    /// The run's `CODEX_HOME`.
    pub fn home(&self) -> PathBuf {
        self.dir.0.join("home")
    }

    /// Why it failed, from its events and log ([`failure`]).
    pub fn failure(&self) -> Option<Failure> {
        failure(&self.stdout, &self.stderr)
    }

    /// The images Codex made (regular files under
    /// `<CODEX_HOME>/generated_images/`, no symlinks, at most `max_bytes`
    /// each), oldest first. Not checked to be images.
    pub async fn generated_images(&self, max_bytes: u64) -> Vec<PathBuf> {
        files_oldest_first(&self.home().join("generated_images"), max_bytes).await
    }

    /// Remove the run's directory now.
    pub async fn remove(mut self) -> std::io::Result<()> {
        let p = core::mem::take(&mut self.dir.0);
        match tokio::fs::remove_dir_all(&p).await {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        }
    }
}

/// Removes its directory when dropped (unless emptied by [`Run::remove`]).
#[derive(Debug)]
struct RunDir(PathBuf);

impl Drop for RunDir {
    fn drop(&mut self) {
        if !self.0.as_os_str().is_empty() {
            // Drop can't await; a run's directory is small.
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
}

/// Serializes copying refreshed logins back within this process.
fn copy_back_lock() -> &'static tokio::sync::Mutex<()> {
    static L: std::sync::OnceLock<tokio::sync::Mutex<()>> = std::sync::OnceLock::new();
    L.get_or_init(tokio::sync::Mutex::default)
}

/// A name no other run of this process (or another, mostly) has.
fn run_name() -> String {
    static N: AtomicU64 = AtomicU64::new(0);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    format!(
        "run-{}-{nanos}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    )
}

/// The last component of `name`, or `image-<i>` if it has none.
fn safe_name(name: &str, i: usize) -> String {
    Path::new(name)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .filter(|n| !n.starts_with('.'))
        .unwrap_or_else(|| format!("image-{}", i + 1))
}

impl Exec {
    /// The `codex exec` arguments for a run in `dir` with `images`.
    fn argv(&self, dir: &Path, images: &[PathBuf]) -> Vec<String> {
        let s = |p: &Path| p.to_string_lossy().into_owned();
        let mut argv: Vec<String> = [
            "exec",
            "--ephemeral",
            "--ignore-user-config",
            "--ignore-rules",
            "--skip-git-repo-check",
            "--sandbox",
            &self.sandbox,
            "--color",
            "never",
            "--json",
            "--cd",
        ]
        .map(String::from)
        .into();
        argv.push(s(&dir.join("work")));
        argv.push("--output-last-message".into());
        argv.push(s(&dir.join("last.txt")));
        if let Some(m) = &self.model {
            argv.extend(["--model".into(), m.clone()]);
        }
        for p in images {
            argv.push(format!("--image={}", s(p)));
        }
        argv.extend(self.args.iter().cloned());
        argv.extend(["--".into(), "-".into()]);
        argv
    }

    /// The child's environment for a run in `dir`.
    fn child_env(&self, dir: &Path) -> Vec<(String, String)> {
        let s = |p: &Path| p.to_string_lossy().into_owned();
        let mut extra: Vec<(String, String)> = Vec::new();
        if !self.env.iter().any(|(k, _)| k == "LANG") {
            extra.push(("LANG".into(), "C.UTF-8".into()));
        }
        extra.extend(
            self.env
                .iter()
                .filter(|(k, _)| !matches!(k.as_str(), "HOME" | "CODEX_HOME" | "TMPDIR"))
                .cloned(),
        );
        extra.extend([
            ("HOME".into(), s(dir)),
            ("CODEX_HOME".into(), s(&dir.join("home"))),
            ("TMPDIR".into(), s(dir)),
        ]);
        crate::client::child_env(std::env::vars(), &extra)
    }

    /// Run `codex exec` once on `input` (see the module docs).
    pub async fn run(&self, input: &Input) -> Result<Run, ExecError> {
        let real = self.codex_home.join("auth.json");
        let read_login = || async { tokio::fs::read(&real).await.map_err(ExecError::Login) };
        // A run that may refresh the login goes alone; the next one reads
        // the login it copied back.
        let mut orig = read_login().await?;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX));
        let _alone = if may_refresh(&orig, now) {
            let g = refresh_lock().lock().await;
            orig = read_login().await?;
            Some(g)
        } else {
            None
        };
        let setup = |what| move |e| ExecError::Setup(what, e);
        mkdir_private(&self.tmp_root)
            .await
            .map_err(setup("directory"))?;
        let dir = RunDir(self.tmp_root.join(run_name()));
        for d in ["", "home", "work", "refs"] {
            tokio::fs::DirBuilder::new()
                .mode(0o700)
                .create(dir.0.join(d))
                .await
                .map_err(setup("directory"))?;
        }
        let ours = dir.0.join("home/auth.json");
        write_private_atomic(&ours, &orig)
            .await
            .map_err(setup("login"))?;
        let mut images = Vec::with_capacity(input.images.len());
        for (i, (name, data)) in input.images.iter().enumerate() {
            let p = dir.0.join("refs").join(safe_name(name, i));
            write_private_atomic(&p, data)
                .await
                .map_err(setup("image"))?;
            images.push(p);
        }
        let out = run_capture(
            &self.command,
            &self.argv(&dir.0, &images),
            &dir.0.join("work"),
            &self.child_env(&dir.0),
            input.prompt.as_bytes(),
            self.timeout,
        )
        .await
        .map_err(ExecError::Spawn)?;
        let copied_back = {
            let _g = copy_back_lock().lock().await;
            crate::auth::copy_back(&Copied::new(real.clone(), ours, orig)).await
        };
        let last_message = tokio::fs::read(dir.0.join("last.txt"))
            .await
            .unwrap_or_default();
        Ok(Run {
            dir,
            code: out.0,
            stdout: out.1,
            stderr: out.2,
            last_message,
            copied_back,
        })
    }
}

/// Run `command argv` in `cwd` with only `env`, `input` on stdin; its exit
/// code (`None`: killed after `timeout`), stdout and stderr.
async fn run_capture(
    command: &str,
    argv: &[String],
    cwd: &Path,
    env: &[(String, String)],
    input: &[u8],
    timeout: Duration,
) -> std::io::Result<(Option<i32>, Vec<u8>, Vec<u8>)> {
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
    let mut child = tokio::process::Command::new(command)
        .args(argv)
        .current_dir(cwd)
        .env_clear()
        .envs(env.iter().map(|(k, v)| (k, v)))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()?;
    if let Some(mut stdin) = child.stdin.take() {
        let data = input.to_vec();
        tokio::spawn(async move {
            let _ = stdin.write_all(&data).await;
        });
    }
    let read = |r: Option<Box<dyn tokio::io::AsyncRead + Send + Unpin>>| {
        tokio::spawn(async move {
            let mut b = Vec::new();
            if let Some(mut r) = r {
                let _ = r.read_to_end(&mut b).await;
            }
            b
        })
    };
    let t_out = read(child.stdout.take().map(|o| Box::new(o) as _));
    let t_err = read(child.stderr.take().map(|e| Box::new(e) as _));
    let code = if let Ok(s) = tokio::time::timeout(timeout, child.wait()).await {
        Some(s?.code().unwrap_or(-1))
    } else {
        let _ = child.kill().await;
        None
    };
    // A grandchild may still hold the pipes open after a kill: don't wait
    // on it for long.
    let grace = if code.is_some() {
        timeout
    } else {
        Duration::from_secs(1)
    };
    let collect = |t: tokio::task::JoinHandle<Vec<u8>>| async move {
        let abort = t.abort_handle();
        tokio::time::timeout(grace, t).await.map_or_else(
            |_| {
                abort.abort();
                Vec::new()
            },
            Result::unwrap_or_default,
        )
    };
    let out = collect(t_out).await;
    let err = collect(t_err).await;
    Ok((code, out, err))
}

/// The regular files under `dir` (recursively; no symlinks) of at most
/// `max_bytes`, oldest first.
async fn files_oldest_first(dir: &Path, max_bytes: u64) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let mut dirs = vec![dir.to_path_buf()];
    while let Some(d) = dirs.pop() {
        let Ok(mut rd) = tokio::fs::read_dir(&d).await else {
            continue;
        };
        while let Ok(Some(e)) = rd.next_entry().await {
            let Ok(ft) = e.file_type().await else {
                continue;
            };
            if ft.is_dir() {
                dirs.push(e.path());
            } else if ft.is_file() {
                match e.metadata().await {
                    Ok(m) if m.len() <= max_bytes => {
                        files.push((m.modified().unwrap_or(std::time::UNIX_EPOCH), e.path()));
                    }
                    _ => {}
                }
            }
        }
    }
    files.sort();
    files.into_iter().map(|(_, p)| p).collect()
}

/// Why a run failed, from its `--json` events and its log.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Failure {
    /// The `ChatGPT` login is spent or revoked (its refresh token was used
    /// up, expired or invalidated): someone must run `codex login` again.
    LoginDead,
    /// The service refused the login (a 401): what it said. Maybe a login.
    LoginRefused(String),
    /// The turn's failure, or the last error event.
    Said(String),
}

impl fmt::Display for Failure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::LoginDead => f.write_str(
                "Codex's ChatGPT login is no longer valid (its refresh token was used up or \
                 revoked): run `codex login` again",
            ),
            Self::LoginRefused(m) => write!(f, "Codex's ChatGPT login was refused ({m})"),
            Self::Said(m) => f.write_str(m),
        }
    }
}

/// Why a `codex exec --json` run failed, from its events (`stdout`) and
/// its log (`stderr`) (pure): a dead login; else a 401; else the turn's
/// failure or the last error event (retry notices skipped), if any.
pub fn failure(stdout: &[u8], stderr: &[u8]) -> Option<Failure> {
    const DEAD: [&str; 6] = [
        "refresh_token_reused",
        "refresh token was already used",
        "refresh_token_expired",
        "refresh_token_invalidated",
        "Please log out and sign in again",
        "Please try signing in again",
    ];
    let err = String::from_utf8_lossy(stderr);
    let out = String::from_utf8_lossy(stdout);
    if DEAD.iter().any(|d| err.contains(d) || out.contains(d)) {
        return Some(Failure::LoginDead);
    }
    let mut said = None;
    for line in out.lines() {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        let m = match v["type"].as_str() {
            Some("turn.failed") => v["error"]["message"].as_str(),
            Some("error") => v["message"].as_str(),
            _ => None,
        };
        if let Some(m) = m.filter(|m| !m.starts_with("Reconnecting...")) {
            said = Some(m.to_owned());
        }
    }
    said.map(|m| {
        if m.contains("401") {
            Failure::LoginRefused(m)
        } else {
            Failure::Said(m)
        }
    })
}

/// A prompt that has Codex make exactly `count` images from `brief` and
/// do nothing else; `aspect_ratio` like `16:9`; `with_images` when the
/// run attaches reference images.
pub fn image_prompt(
    brief: &str,
    count: u64,
    aspect_ratio: Option<&str>,
    with_images: bool,
) -> String {
    use fmt::Write as _;
    let mut t = format!(
        "Use your image generation tool to create exactly {count} image{} from the brief below",
        if count == 1 { "" } else { "s" }
    );
    if let Some(a) = aspect_ratio {
        let _ = write!(t, ", aspect ratio {a} (width:height)");
    }
    if with_images {
        t.push_str(", using the attached image(s) as the reference the brief refers to");
    }
    t.push_str(
        ". Generate each image separately. Do nothing else: no shell commands, no reading or \
         writing files, no web searches, no questions. When the images are made, reply with \
         only the word done.\n\nBrief:\n",
    );
    t.push_str(brief);
    t
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn image_prompt_says_what() {
        let t = image_prompt("a red apple", 2, Some("16:9"), true);
        assert!(t.contains("exactly 2 images") && t.contains("16:9") && t.contains("attached"));
        assert!(t.ends_with("Brief:\na red apple"), "{t}");
        let t = image_prompt("x", 1, None, false);
        assert!(
            t.contains("exactly 1 image ") && !t.contains("attached"),
            "{t}"
        );
    }

    #[test]
    fn failure_says_why() {
        // A spent login: stderr has the refresh error, the events only
        // "unauthorized (401)".
        let out = br#"{"type":"thread.started","thread_id":"t"}
{"type":"error","message":"Reconnecting... 2/5 (workspace routing discovery unauthorized (401))"}
{"type":"turn.failed","error":{"message":"workspace routing discovery unauthorized (401)"}}"#;
        let err = b"ERROR codex_login::auth::manager: Failed to refresh token: Your access token \
could not be refreshed because your refresh token was already used. Please log out and sign in again.";
        assert_eq!(failure(out, err), Some(Failure::LoginDead));
        assert!(Failure::LoginDead.to_string().contains("codex login"));
        assert_eq!(
            failure(out, b""),
            Some(Failure::LoginRefused(
                "workspace routing discovery unauthorized (401)".into()
            ))
        );
        let out = br#"{"type":"turn.failed","error":{"message":"You've hit your usage limit."}}"#;
        assert_eq!(
            failure(out, b""),
            Some(Failure::Said("You've hit your usage limit.".into()))
        );
        assert_eq!(failure(b"not json", b"noise"), None);
    }

    #[test]
    fn names_and_args() {
        assert_eq!(safe_name("../../etc/x.png", 0), "x.png");
        assert_eq!(safe_name("..", 1), "image-2");
        assert_eq!(safe_name(".hidden", 2), "image-3");
        let mut e = Exec::new("codex", "/h", "/t");
        e.model = Some("m".into());
        let a = e.argv(Path::new("/t/r"), &[PathBuf::from("/t/r/refs/a.png")]);
        assert_eq!(a[0], "exec");
        assert!(a.windows(2).any(|w| w == ["--sandbox", "read-only"]));
        assert!(a.windows(2).any(|w| w == ["--cd", "/t/r/work"]));
        assert!(a.windows(2).any(|w| w == ["--model", "m"]));
        assert!(a.contains(&"--image=/t/r/refs/a.png".to_owned()));
        assert!(a.ends_with(&["--".into(), "-".into()]));
        e.env = vec![
            ("PATH".into(), "/bin".into()),
            ("HOME".into(), "/nope".into()),
        ];
        let env = e.child_env(Path::new("/t/r"));
        let get = |k: &str| {
            env.iter()
                .filter(|(n, _)| n == k)
                .map(|(_, v)| v.as_str())
                .collect::<Vec<_>>()
        };
        assert_eq!(get("HOME"), ["/t/r"]);
        assert_eq!(get("CODEX_HOME"), ["/t/r/home"]);
        assert_eq!(get("PATH"), ["/bin"]);
        assert_eq!(get("LANG"), ["C.UTF-8"]);
    }

    #[tokio::test]
    async fn oldest_first_regular_files_only() {
        let d = tempfile::tempdir().unwrap();
        let g = d.path().join("t1");
        tokio::fs::create_dir_all(&g).await.unwrap();
        tokio::fs::write(g.join("a.png"), b"1").await.unwrap();
        tokio::time::sleep(Duration::from_millis(20)).await;
        tokio::fs::write(g.join("b.jpg"), b"2").await.unwrap();
        tokio::fs::write(g.join("big.png"), b"0123456789")
            .await
            .unwrap();
        std::os::unix::fs::symlink("/etc/passwd", g.join("link.png")).unwrap();
        let names: Vec<_> = files_oldest_first(d.path(), 4)
            .await
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, ["a.png", "b.jpg"]);
        assert_eq!(
            files_oldest_first(&d.path().join("none"), 4).await,
            Vec::<PathBuf>::new()
        );
    }
}
