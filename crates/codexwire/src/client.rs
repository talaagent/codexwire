//! The app-server client: writes requests, matches responses by id, and
//! hands the server's notifications and requests to the caller in order.
//! Generic over the transport (the child's stdio, or an in-process duplex
//! in tests).

use alloc::collections::VecDeque;
use alloc::sync::Arc;
use core::sync::atomic::{AtomicI64, Ordering};
use std::collections::HashMap;
use std::path::Path;
use std::process::Stdio;

use serde_json::Value;
use tokio::io::{AsyncBufReadExt as _, AsyncRead, AsyncWrite, AsyncWriteExt as _, BufReader};
use tokio::sync::{Mutex, mpsc, oneshot};

use crate::protocol::Request;
use crate::rpc::{self, Incoming, RequestId, RpcError};

/// Why a request failed.
#[derive(Debug, Clone, PartialEq)]
pub enum ClientError {
    /// The server answered with an error.
    Rpc(RpcError),
    /// The connection closed before the answer.
    Closed,
    /// Writing failed.
    Io(String),
    /// The params didn't serialise as JSON.
    Encode(String),
    /// The answer didn't decode as the method's response type.
    Decode(String),
}

impl core::fmt::Display for ClientError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Rpc(e) => write!(f, "app-server error: {e}"),
            Self::Closed => f.write_str("app-server connection closed"),
            Self::Io(e) => write!(f, "app-server write: {e}"),
            Self::Encode(e) => write!(f, "app-server request params: {e}"),
            Self::Decode(e) => write!(f, "app-server answer: {e}"),
        }
    }
}

impl core::error::Error for ClientError {}

/// What the server sends us, in order.
#[derive(Debug, Clone, PartialEq)]
pub enum ServerMessage {
    /// Answer it with [`Client::respond`] or [`Client::respond_error`].
    Request {
        id: RequestId,
        method: String,
        params: Value,
    },
    Notification {
        method: String,
        params: Value,
    },
    /// A line that didn't parse, or wasn't UTF-8: why (logged and skipped
    /// by most callers). The connection goes on.
    Malformed(String),
}

/// The calls waiting for an answer, and whether the reader has stopped
/// (then no call waits: it would wait forever).
#[derive(Default)]
struct PendingState {
    closed: bool,
    waiting: HashMap<i64, oneshot::Sender<Result<Value, RpcError>>>,
}

type Pending = Arc<std::sync::Mutex<PendingState>>;

/// Removes a call's entry when its future is done or dropped (cancelled by
/// a `select!` or a timeout), so abandoned calls don't pile up.
struct Waiting<'a> {
    pending: &'a Pending,
    id: i64,
}

impl Drop for Waiting<'_> {
    fn drop(&mut self) {
        lock(self.pending).waiting.remove(&self.id);
    }
}

/// The request side; cheap to share (`Arc`).
pub struct Client {
    writer: Mutex<Option<Box<dyn AsyncWrite + Send + Unpin>>>,
    next_id: AtomicI64,
    pending: Pending,
}

/// The server's notifications and requests; `None` once it closed.
pub type Messages = mpsc::UnboundedReceiver<ServerMessage>;

fn lock<T>(m: &std::sync::Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

impl Client {
    /// Start reading `reader` (a task) and writing to `writer`.
    ///
    /// The reader runs until `reader` ends (EOF or an I/O error); a line
    /// that isn't JSON-RPC or isn't UTF-8 comes out as
    /// [`ServerMessage::Malformed`] and the reading goes on.
    ///
    /// Neither the line length nor the [`Messages`] channel is bounded: a
    /// caller that stops reading `Messages`, or a server that writes one
    /// huge line, grows memory.
    ///
    /// # Panics
    ///
    /// Outside a tokio runtime (it spawns the reader with `tokio::spawn`).
    pub fn new<R, W>(reader: R, writer: W) -> (Arc<Self>, Messages)
    where
        R: AsyncRead + Send + Unpin + 'static,
        W: AsyncWrite + Send + Unpin + 'static,
    {
        let pending: Pending = Arc::default();
        let (tx, rx) = mpsc::unbounded_channel();
        tokio::spawn(read_server(reader, Arc::clone(&pending), tx));
        let c = Self {
            writer: Mutex::new(Some(Box::new(writer))),
            next_id: AtomicI64::new(1),
            pending,
        };
        (Arc::new(c), rx)
    }

    async fn write(&self, line: &str) -> Result<(), ClientError> {
        let mut writer = self.writer.lock().await;
        let w = writer.as_mut().ok_or(ClientError::Closed)?;
        w.write_all(line.as_bytes())
            .await
            .map_err(|e| ClientError::Io(e.to_string()))?;
        w.flush().await.map_err(|e| ClientError::Io(e.to_string()))
    }

    /// Send a raw request and await its result.
    ///
    /// [`ClientError::Closed`] at once once the server's output has ended.
    /// There is no timeout of its own: wrap it (or [`Client::request`]) in
    /// `tokio::time::timeout`. Dropping the future (a timeout, a `select!`)
    /// forgets the request; a late answer to it is ignored.
    pub async fn call(&self, method: &str, params: &Value) -> Result<Value, ClientError> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        {
            let mut p = lock(&self.pending);
            if p.closed {
                return Err(ClientError::Closed);
            }
            p.waiting.insert(id, tx);
        }
        let _waiting = Waiting {
            pending: &self.pending,
            id,
        };
        self.write(&rpc::request(&Value::from(id), method, params))
            .await?;
        match rx.await {
            Ok(Ok(v)) => Ok(v),
            Ok(Err(e)) => Err(ClientError::Rpc(e)),
            Err(_) => Err(ClientError::Closed),
        }
    }

    /// A typed request (its method and response from the generated
    /// protocol).
    pub async fn request<R: Request + Sync>(&self, params: &R) -> Result<R::Response, ClientError> {
        let p = serde_json::to_value(params).map_err(|e| ClientError::Encode(e.to_string()))?;
        let v = self.call(R::METHOD, &p).await?;
        serde_json::from_value(v).map_err(|e| ClientError::Decode(format!("{}: {e}", R::METHOD)))
    }

    pub async fn notify(&self, method: &str, params: Option<&Value>) -> Result<(), ClientError> {
        self.write(&rpc::notification(method, params)).await
    }

    pub async fn respond(&self, id: &RequestId, result: &Value) -> Result<(), ClientError> {
        self.write(&rpc::response(id, result)).await
    }

    pub async fn respond_error(
        &self,
        id: &RequestId,
        code: i64,
        message: &str,
    ) -> Result<(), ClientError> {
        self.write(&rpc::error_response(id, code, message)).await
    }

    /// Close our side (the app-server exits on EOF). Later writes fail
    /// with [`ClientError::Closed`]. Calling this more than once is harmless.
    pub async fn shutdown(&self) {
        // AsyncWrite::shutdown alone need not close a child pipe. Drop the
        // owned writer as well so the process can observe EOF on stdin.
        let writer = self.writer.lock().await.take();
        if let Some(mut writer) = writer {
            let _ = writer.shutdown().await;
        }
    }
}

/// The reader task: route answers to their calls, hand the rest to `tx`.
async fn read_server<R: AsyncRead + Unpin>(
    reader: R,
    p: Pending,
    tx: mpsc::UnboundedSender<ServerMessage>,
) {
    let mut reader = BufReader::new(reader);
    let mut buf = Vec::new();
    loop {
        buf.clear();
        match reader.read_until(b'\n', &mut buf).await {
            Ok(0) | Err(_) => break,
            Ok(_) => {}
        }
        let line = match core::str::from_utf8(&buf) {
            Ok(l) => l,
            Err(e) => {
                let _ = tx.send(ServerMessage::Malformed(format!(
                    "JSON-RPC line is not UTF-8: {e}"
                )));
                continue;
            }
        };
        let msg = match rpc::parse_line(line) {
            Ok(None) => continue,
            Ok(Some(m)) => m,
            Err(e) => {
                let _ = tx.send(ServerMessage::Malformed(e));
                continue;
            }
        };
        match msg {
            Incoming::Response { id, result } => answer(&p, &id, Ok(result)),
            Incoming::Error { id, error } => answer(&p, &id, Err(error)),
            Incoming::Request { id, method, params } => {
                let _ = tx.send(ServerMessage::Request { id, method, params });
            }
            Incoming::Notification { method, params } => {
                let _ = tx.send(ServerMessage::Notification { method, params });
            }
        }
    }
    // Closed: fail whatever is still waiting (dropping the senders), and
    // every later call.
    let mut p = lock(&p);
    p.closed = true;
    p.waiting.clear();
}

fn answer(p: &Pending, id: &RequestId, r: Result<Value, RpcError>) {
    let Some(id) = id.as_i64() else { return };
    if let Some(tx) = lock(p).waiting.remove(&id) {
        let _ = tx.send(r);
    }
}

/// The last lines the child wrote on stderr (for error messages).
#[derive(Clone, Default)]
pub struct StderrTail(Arc<std::sync::Mutex<VecDeque<String>>>);

impl StderrTail {
    const KEEP: usize = 20;

    /// Keep `line`, dropping the oldest past the last 20 (for a child this
    /// crate didn't spawn; `spawn_app_server` fills its own).
    pub fn push(&self, line: String) {
        let mut q = lock(&self.0);
        if q.len() == Self::KEEP {
            q.pop_front();
        }
        q.push_back(line);
    }

    /// Keep each line of `reader` until it ends (EOF or an I/O error).
    /// Lines that aren't UTF-8 are kept lossily (`U+FFFD`), so the reading
    /// never stops early and the child never writes into a closed pipe.
    pub async fn read_from<R: AsyncRead + Unpin>(&self, reader: R) {
        let mut reader = BufReader::new(reader);
        let mut buf = Vec::new();
        loop {
            buf.clear();
            match reader.read_until(b'\n', &mut buf).await {
                Ok(0) | Err(_) => break,
                Ok(_) => {}
            }
            let line = String::from_utf8_lossy(&buf);
            self.push(line.trim_end_matches(['\n', '\r']).to_owned());
        }
    }

    /// The kept lines, oldest first, one a line.
    pub fn text(&self) -> String {
        lock(&self.0)
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>()
            .join("\n")
    }
}

/// `text` with secret-looking words (`sk-…`, a JWT, a long token, the
/// value after `Bearer` or of a `key=`/`token:`-style pair) as
/// `[redacted]`, cut to `max` characters.
pub fn redact(text: &str, max: usize) -> String {
    let mut out = String::with_capacity(text.len().min(max));
    for (i, line) in text.lines().enumerate() {
        if i > 0 {
            out.push('\n');
        }
        let mut after_bearer = false;
        for (j, w) in line.split(' ').enumerate() {
            if j > 0 {
                out.push(' ');
            }
            if after_bearer || secretish(w) {
                out.push_str("[redacted]");
            } else if let Some((k, _)) = secret_pair(w) {
                out.push_str(k);
                out.push_str("[redacted]");
            } else {
                out.push_str(w);
            }
            after_bearer = w.eq_ignore_ascii_case("bearer") || w.eq_ignore_ascii_case("bearer:");
        }
    }
    if out.chars().count() > max {
        let mut cut: String = out.chars().take(max.saturating_sub(1)).collect();
        cut.push('…');
        cut
    } else {
        out
    }
}

/// A word that looks like a credential on its own.
fn secretish(w: &str) -> bool {
    let w = w.trim_matches(|c: char| "\"'`,;()[]{}<>".contains(c));
    let tokenish = w.len() >= 24
        && w.chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_.=+/".contains(c))
        && w.chars().any(|c| c.is_ascii_digit())
        && !w.contains("//");
    w.starts_with("sk-") || w.starts_with("eyJ") || tokenish
}

/// `token=…`, `"api_key":"…"`, `password: …` (the part up to and
/// including the separator, and the value).
fn secret_pair(w: &str) -> Option<(&str, &str)> {
    let i = w.find(['=', ':'])?;
    let key = w[..i]
        .trim_matches(|c: char| "\"'`{,".contains(c))
        .to_ascii_lowercase();
    let secret = [
        "token", "secret", "password", "passwd", "api_key", "apikey", "auth", "key",
    ]
    .iter()
    .any(|s| key.ends_with(s))
        && !key.ends_with("tokens");
    let value = &w[i + 1..];
    (secret && !value.is_empty()).then(|| (&w[..=i], value))
}

/// The parent's environment variables a `codex app-server` gets (besides
/// the backend's own, like `CODEX_HOME`): it starts from an empty one, so
/// no secret of the parent's reaches the session.
///
/// No proxy variables (`HTTPS_PROXY`, `NO_PROXY`, ...): a proxy URL can
/// carry credentials. A caller that needs them passes them in `env`.
pub const ENV_ALLOW: &[&str] = &[
    "PATH",
    "HOME",
    "USER",
    "LOGNAME",
    "LANG",
    "LC_ALL",
    "TZ",
    "TMPDIR",
    "SSL_CERT_FILE",
    "NIX_SSL_CERT_FILE",
];

/// [`ENV_ALLOW`]'s values from `from` (the parent's environment), then
/// `extra` (which wins).
pub fn child_env(
    from: impl IntoIterator<Item = (String, String)>,
    extra: &[(String, String)],
) -> Vec<(String, String)> {
    let mut env: Vec<(String, String)> = from
        .into_iter()
        .filter(|(k, _)| ENV_ALLOW.contains(&k.as_str()))
        .filter(|(k, _)| !extra.iter().any(|(e, _)| e == k))
        .collect();
    env.extend(extra.iter().cloned());
    env
}

/// A running `codex app-server`.
pub struct AppServer {
    pub client: Arc<Client>,
    pub messages: Messages,
    pub child: tokio::process::Child,
    pub stderr: StderrTail,
}

/// Spawn `<command> app-server <args>` in `cwd` with only `env` and the
/// parent's [`ENV_ALLOW`] variables (killed when dropped).
///
/// For a graceful stop, [`Client::shutdown`] (the server exits on EOF),
/// then await `child.wait()` under a timeout before dropping it.
///
/// # Panics
///
/// Outside a tokio runtime, like [`Client::new`].
pub fn spawn_app_server(
    command: &str,
    args: &[String],
    env: &[(String, String)],
    cwd: &Path,
) -> std::io::Result<AppServer> {
    let mut cmd = tokio::process::Command::new(command);
    cmd.arg("app-server")
        .args(args)
        .current_dir(cwd)
        .env_clear()
        .envs(child_env(std::env::vars(), env))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let mut child = cmd.spawn()?;
    let (Some(stdin), Some(stdout), Some(stderr)) =
        (child.stdin.take(), child.stdout.take(), child.stderr.take())
    else {
        return Err(std::io::Error::other("app-server stdio not piped"));
    };
    let tail = StderrTail::default();
    let t = tail.clone();
    tokio::spawn(async move { t.read_from(stderr).await });
    let (client, messages) = Client::new(stdout, stdin);
    Ok(AppServer {
        client,
        messages,
        child,
        stderr: tail,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn redacts_and_caps_stderr() {
        let t = "error: auth failed Bearer abc.def token=hunter2 key sk-proj-123\n\
                 jwt eyJhbGciOi x 0123456789abcdefABCDEF0123 max_tokens=5 at /home/alice/x.rs:3";
        assert_eq!(
            redact(t, 1000),
            "error: auth failed Bearer [redacted] token=[redacted] key [redacted]\n\
             jwt [redacted] x [redacted] max_tokens=5 at /home/alice/x.rs:3"
        );
        assert_eq!(redact("\"api_key\":\"zz\"", 100), "\"api_key\":[redacted]");
        let long = "a ".repeat(2000);
        let r = redact(&long, 50);
        assert_eq!(r.chars().count(), 50);
        assert!(r.ends_with('…'));
    }

    #[test]
    fn the_child_env_is_an_allowlist() {
        let parent = [
            ("PATH", "/bin"),
            ("HOME", "/home/alice"),
            ("ANTHROPIC_API_KEY", "sk-x"),
            ("DAEMON_TOKEN", "t"),
            ("LC_CTYPE", "must-not-be-inherited"),
            ("CODEX_HOME", "/parent/codex"),
        ]
        .map(|(k, v)| (k.to_owned(), v.to_owned()));
        let extra = [("CODEX_HOME".to_owned(), "/s/codex".to_owned())];
        let env = child_env(parent, &extra);
        assert_eq!(
            env,
            [
                ("PATH", "/bin"),
                ("HOME", "/home/alice"),
                ("CODEX_HOME", "/s/codex")
            ]
            .map(|(k, v)| (k.to_owned(), v.to_owned()))
        );
    }

    #[tokio::test]
    async fn matches_responses_and_forwards_the_rest() {
        let (ours, theirs) = tokio::io::duplex(4096);
        let (rd, wr) = tokio::io::split(ours);
        let (client, mut msgs) = Client::new(rd, wr);
        let (sr, mut sw) = tokio::io::split(theirs);
        let mut sr = BufReader::new(sr).lines();
        let c = Arc::clone(&client);
        let call = tokio::spawn(async move { c.call("thread/start", &json!({"cwd":"/w"})).await });
        let line = sr.next_line().await.unwrap().unwrap();
        let v: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(v["method"], "thread/start");
        let id = v["id"].clone();
        // A notification and a server request before the answer.
        sw.write_all(b"{\"method\":\"thread/started\",\"params\":{}}\n{\"id\":0,\"method\":\"item/fileChange/requestApproval\",\"params\":{}}\n")
            .await
            .unwrap();
        sw.write_all(rpc::response(&id, &json!({"ok":true})).as_bytes())
            .await
            .unwrap();
        assert_eq!(call.await.unwrap().unwrap(), json!({"ok":true}));
        assert!(
            matches!(msgs.recv().await, Some(ServerMessage::Notification { method, .. }) if method == "thread/started")
        );
        assert!(
            matches!(msgs.recv().await, Some(ServerMessage::Request { method, .. }) if method == "item/fileChange/requestApproval")
        );
        client
            .respond(&json!(0), &json!({"decision":"accept"}))
            .await
            .unwrap();
        assert_eq!(
            sr.next_line().await.unwrap().unwrap(),
            r#"{"id":0,"result":{"decision":"accept"}}"#
        );
        // An error answer, then the server goes away mid-request.
        let c = Arc::clone(&client);
        let call = tokio::spawn(async move { c.call("turn/start", &json!({})).await });
        let v: Value = serde_json::from_str(&sr.next_line().await.unwrap().unwrap()).unwrap();
        sw.write_all(rpc::error_response(&v["id"], -1, "bad").as_bytes())
            .await
            .unwrap();
        assert!(matches!(call.await.unwrap(), Err(ClientError::Rpc(e)) if e.message == "bad"));
        let c = Arc::clone(&client);
        let call = tokio::spawn(async move { c.call("turn/start", &json!({})).await });
        let _ = sr.next_line().await.unwrap();
        drop(sw);
        drop(sr);
        assert_eq!(call.await.unwrap(), Err(ClientError::Closed));
        assert_eq!(msgs.recv().await, None);
    }

    /// What the server reads: our lines.
    type ServerIn = tokio::io::Lines<BufReader<tokio::io::DuplexStream>>;

    /// A client over two pipes: the server's output (`out`) and its input
    /// (`inp`), so either can close alone.
    fn piped() -> (Arc<Client>, Messages, tokio::io::DuplexStream, ServerIn) {
        let (out_ours, out_theirs) = tokio::io::duplex(4096);
        let (in_ours, in_theirs) = tokio::io::duplex(4096);
        let (client, msgs) = Client::new(out_ours, in_ours);
        (client, msgs, out_theirs, BufReader::new(in_theirs).lines())
    }

    async fn within<F: Future>(f: F) -> F::Output {
        tokio::time::timeout(core::time::Duration::from_secs(5), f)
            .await
            .expect("hung")
    }

    #[tokio::test]
    async fn a_call_after_the_server_output_ended_fails_at_once() {
        let (client, mut msgs, out, _inp) = piped();
        // The server's stdout closes; its stdin (ours to write) stays open.
        drop(out);
        assert_eq!(within(msgs.recv()).await, None);
        assert_eq!(
            within(client.call("thread/start", &json!({}))).await,
            Err(ClientError::Closed)
        );
        assert!(lock(&client.pending).waiting.is_empty());
    }

    #[tokio::test]
    async fn a_line_that_is_not_utf8_is_malformed_and_reading_goes_on() {
        let (client, mut msgs, mut out, mut inp) = piped();
        let c = Arc::clone(&client);
        let call = tokio::spawn(async move { c.call("turn/start", &json!({})).await });
        let v: Value = serde_json::from_str(&inp.next_line().await.unwrap().unwrap()).unwrap();
        out.write_all(
            b"\xff\xfe not utf-8\n{\"method\":\"turn/started\",\"params\":{}}\nnot json\n",
        )
        .await
        .unwrap();
        out.write_all(rpc::response(&v["id"], &json!(1)).as_bytes())
            .await
            .unwrap();
        assert!(
            matches!(within(msgs.recv()).await, Some(ServerMessage::Malformed(e)) if e.contains("UTF-8"))
        );
        assert!(
            matches!(within(msgs.recv()).await, Some(ServerMessage::Notification { method, .. }) if method == "turn/started")
        );
        assert!(matches!(
            within(msgs.recv()).await,
            Some(ServerMessage::Malformed(_))
        ));
        assert_eq!(within(call).await.unwrap(), Ok(json!(1)));
    }

    #[tokio::test]
    async fn a_cancelled_call_forgets_its_request() {
        let (client, mut msgs, mut out, mut inp) = piped();
        let r = tokio::time::timeout(
            core::time::Duration::from_millis(50),
            client.call("turn/start", &json!({})),
        )
        .await;
        assert!(r.is_err(), "no answer: times out");
        assert!(lock(&client.pending).waiting.is_empty());
        // A late answer to it is ignored; the connection goes on.
        let v: Value = serde_json::from_str(&inp.next_line().await.unwrap().unwrap()).unwrap();
        out.write_all(rpc::response(&v["id"], &json!(1)).as_bytes())
            .await
            .unwrap();
        out.write_all(b"{\"method\":\"x\"}\n").await.unwrap();
        assert!(
            matches!(within(msgs.recv()).await, Some(ServerMessage::Notification { method, .. }) if method == "x")
        );
    }

    #[tokio::test]
    async fn stderr_lines_that_are_not_utf8_are_kept_lossily() {
        let tail = StderrTail::default();
        tail.read_from(&b"one\n\xff two\r\nthree"[..]).await;
        assert_eq!(tail.text(), "one\n\u{fffd} two\nthree");
        for i in 0..30 {
            tail.push(i.to_string());
        }
        assert_eq!(tail.text().lines().count(), 20);
        assert!(tail.text().starts_with("10\n"));
    }
}
