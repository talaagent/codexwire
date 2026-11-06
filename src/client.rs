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
    /// The answer didn't decode as the method's response type.
    Decode(String),
}

impl core::fmt::Display for ClientError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Rpc(e) => write!(f, "app-server error: {e}"),
            Self::Closed => f.write_str("app-server connection closed"),
            Self::Io(e) => write!(f, "app-server write: {e}"),
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
    /// A line that didn't parse (logged and skipped by most callers).
    Malformed(String),
}

type Pending = Arc<std::sync::Mutex<HashMap<i64, oneshot::Sender<Result<Value, RpcError>>>>>;

/// The request side; cheap to share (`Arc`).
pub struct Client {
    writer: Mutex<Box<dyn AsyncWrite + Send + Unpin>>,
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
    pub fn new<R, W>(reader: R, writer: W) -> (Arc<Self>, Messages)
    where
        R: AsyncRead + Send + Unpin + 'static,
        W: AsyncWrite + Send + Unpin + 'static,
    {
        let pending: Pending = Arc::default();
        let (tx, rx) = mpsc::unbounded_channel();
        let p = Arc::clone(&pending);
        tokio::spawn(async move {
            let mut lines = BufReader::new(reader).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                let msg = match rpc::parse_line(&line) {
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
            // Closed: fail whatever is still waiting (dropping the senders).
            lock(&p).clear();
        });
        let c = Self {
            writer: Mutex::new(Box::new(writer)),
            next_id: AtomicI64::new(1),
            pending,
        };
        (Arc::new(c), rx)
    }

    async fn write(&self, line: &str) -> Result<(), ClientError> {
        let mut w = self.writer.lock().await;
        w.write_all(line.as_bytes())
            .await
            .map_err(|e| ClientError::Io(e.to_string()))?;
        w.flush().await.map_err(|e| ClientError::Io(e.to_string()))
    }

    /// Send a raw request and await its result.
    pub async fn call(&self, method: &str, params: &Value) -> Result<Value, ClientError> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        lock(&self.pending).insert(id, tx);
        if let Err(e) = self
            .write(&rpc::request(&Value::from(id), method, params))
            .await
        {
            lock(&self.pending).remove(&id);
            return Err(e);
        }
        match rx.await {
            Ok(Ok(v)) => Ok(v),
            Ok(Err(e)) => Err(ClientError::Rpc(e)),
            Err(_) => Err(ClientError::Closed),
        }
    }

    /// A typed request (its method and response from the generated
    /// protocol).
    pub async fn request<R: Request + Sync>(&self, params: &R) -> Result<R::Response, ClientError> {
        let p = serde_json::to_value(params).map_err(|e| ClientError::Decode(e.to_string()))?;
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

    /// Close our side (the app-server exits on EOF).
    pub async fn shutdown(&self) {
        let _ = self.writer.lock().await.shutdown().await;
    }
}

fn answer(p: &Pending, id: &RequestId, r: Result<Value, RpcError>) {
    let Some(id) = id.as_i64() else { return };
    if let Some(tx) = lock(p).remove(&id) {
        let _ = tx.send(r);
    }
}

/// The last lines the child wrote on stderr (for error messages).
#[derive(Clone, Default)]
pub struct StderrTail(Arc<std::sync::Mutex<VecDeque<String>>>);

impl StderrTail {
    const KEEP: usize = 20;

    pub(crate) fn push(&self, line: String) {
        let mut q = lock(&self.0);
        if q.len() == Self::KEEP {
            q.pop_front();
        }
        q.push_back(line);
    }

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
    tokio::spawn(async move {
        let mut lines = BufReader::new(stderr).lines();
        while let Ok(Some(l)) = lines.next_line().await {
            t.push(l);
        }
    });
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
}
