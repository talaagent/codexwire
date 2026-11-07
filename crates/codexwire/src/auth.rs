//! Codex's `ChatGPT` login (`<CODEX_HOME>/auth.json`) shared with a session's
//! own `CODEX_HOME`: copied in before the app-server starts, copied back when
//! Codex refreshed its tokens and nobody else changed the real one since.
//! (The same rule a tool that refreshes the login and copies it back follows.)
//!
//! Unix only: the files are made 0600 and the directories 0700.

use std::path::{Path, PathBuf};

/// Copy a refreshed `auth.json` back (pure decision): `orig` what was
/// copied in, `now_real` the real file now, `after` the session's copy.
/// Only when Codex rewrote it, it is a JSON object, and the real one is
/// unchanged since.
pub fn auth_to_copy_back(orig: &[u8], now_real: &[u8], after: &[u8]) -> bool {
    after != orig
        && now_real == orig
        && !after.is_empty()
        && serde_json::from_slice::<serde_json::Value>(after).is_ok_and(|v| v.is_object())
}

/// Whether a run on login `auth` (an `auth.json`) may refresh its tokens
/// at `now` (unix seconds; pure): the access token ends within 10
/// minutes, the last refresh is a week old (Codex refreshes ahead of
/// time), or either can't be read.
///
/// Such a run should go alone ([`refresh_lock`]): `ChatGPT`'s refresh
/// tokens are single-use, and two runs refreshing the same one get it
/// revoked (`refresh_token_reused`) until the user signs in again.
pub fn may_refresh(auth: &[u8], now: i64) -> bool {
    use base64::Engine as _;
    let Ok(v) = serde_json::from_slice::<serde_json::Value>(auth) else {
        return true;
    };
    let exp = v["tokens"]["access_token"]
        .as_str()
        .and_then(|t| t.split('.').nth(1))
        .and_then(|p| {
            base64::engine::general_purpose::URL_SAFE_NO_PAD
                .decode(p.trim_end_matches('='))
                .ok()
        })
        .and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok())
        .and_then(|c| c["exp"].as_i64());
    let last = v["last_refresh"]
        .as_str()
        .and_then(|s| s.parse::<jiff::Timestamp>().ok())
        .map(jiff::Timestamp::as_second);
    match (exp, last) {
        (Some(exp), Some(last)) => exp <= now + 600 || last <= now - 7 * 86_400,
        _ => true,
    }
}

/// This process's lock for runs that [`may_refresh`] the login: hold it
/// (it is async, so across the run) from reading `auth.json` until the
/// refreshed one is copied back, and the next run reads the new tokens.
/// One lock for every login: a process rarely has more than one, and
/// runs that may refresh are rare. It doesn't reach other processes.
pub fn refresh_lock() -> &'static tokio::sync::Mutex<()> {
    static L: std::sync::OnceLock<tokio::sync::Mutex<()>> = std::sync::OnceLock::new();
    L.get_or_init(tokio::sync::Mutex::default)
}

/// What was copied into a session's home, to copy back on close.
///
/// It holds the copied `auth.json` (its tokens), so its `Debug` shows only
/// the length of it.
#[derive(Clone)]
pub struct Copied {
    pub real: PathBuf,
    pub ours: PathBuf,
    orig: Vec<u8>,
}

impl Copied {
    pub(crate) fn new(real: PathBuf, ours: PathBuf, orig: Vec<u8>) -> Self {
        Self { real, ours, orig }
    }

    /// The length of the `auth.json` that was copied in.
    pub fn orig_len(&self) -> usize {
        self.orig.len()
    }
}

impl core::fmt::Debug for Copied {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Copied")
            .field("real", &self.real)
            .field("ours", &self.ours)
            .field(
                "orig",
                &format_args!("[redacted; {} bytes]", self.orig.len()),
            )
            .finish()
    }
}

/// Make `home` (0700) and copy `<from>/auth.json` into it (0600).
pub async fn copy_in(from: &Path, home: &Path) -> std::io::Result<Copied> {
    mkdir_private(home).await?;
    let real = from.join("auth.json");
    let orig = tokio::fs::read(&real).await?;
    let ours = home.join("auth.json");
    write_private_atomic(&ours, &orig).await?;
    Ok(Copied { real, ours, orig })
}

/// Copy the session's `auth.json` back when [`auth_to_copy_back`] says so.
/// Returns whether it did.
///
/// Not atomic against another writer of the real file: one that writes it
/// between this reading it and the rename loses its write. The window is
/// short (a read and a rename); callers that share the real file across
/// processes serialise their copy-backs.
pub async fn copy_back(c: &Copied) -> std::io::Result<bool> {
    let after = match tokio::fs::read(&c.ours).await {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(e) => return Err(e),
    };
    let now_real = tokio::fs::read(&c.real).await?;
    if !auth_to_copy_back(&c.orig, &now_real, &after) {
        return Ok(false);
    }
    write_private_atomic(&c.real, &after).await?;
    Ok(true)
}

/// `mkdir -p -m 0700`: only the directories it creates get 0700; an
/// existing `p` keeps its mode.
pub async fn mkdir_private(p: &Path) -> std::io::Result<()> {
    let mut b = tokio::fs::DirBuilder::new();
    b.recursive(true).mode(0o700);
    b.create(p).await
}

/// Write `data` to `p` (0600) through a temp file and a rename.
pub(crate) async fn write_private_atomic(p: &Path, data: &[u8]) -> std::io::Result<()> {
    let name = p
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.subsec_nanos());
    let tmp = p.with_file_name(format!(".{name}.codexwire-{}-{nanos}", std::process::id()));
    let r = async {
        let mut f = tokio::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&tmp)
            .await?;
        tokio::io::AsyncWriteExt::write_all(&mut f, data).await?;
        f.sync_all().await?;
        tokio::fs::rename(&tmp, p).await
    }
    .await;
    if r.is_err() {
        let _ = tokio::fs::remove_file(&tmp).await;
    }
    r
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copy_back_decisions() {
        let orig = br#"{"tokens":1}"#;
        let new = br#"{"tokens":2}"#;
        assert!(
            auth_to_copy_back(orig, orig, new),
            "refreshed, real unchanged"
        );
        assert!(!auth_to_copy_back(orig, orig, orig), "not refreshed");
        assert!(
            !auth_to_copy_back(orig, br#"{"tokens":3}"#, new),
            "real changed meanwhile"
        );
        assert!(!auth_to_copy_back(orig, orig, b""), "emptied");
        assert!(!auth_to_copy_back(orig, orig, b"not json"), "garbage");
        assert!(!auth_to_copy_back(orig, orig, b"[1]"), "not an object");
    }

    #[test]
    fn may_refresh_only_near_expiry() {
        use base64::Engine as _;
        let now = 1_800_000_000;
        let auth = |exp: i64, last: &str| {
            let claims = base64::engine::general_purpose::URL_SAFE_NO_PAD
                .encode(format!(r#"{{"exp":{exp}}}"#));
            format!(r#"{{"last_refresh":"{last}","tokens":{{"access_token":"h.{claims}.s"}}}}"#)
        };
        let recent = "2027-01-14T00:00:00Z"; // a day before `now`
        assert!(!may_refresh(auth(now + 86_400, recent).as_bytes(), now));
        assert!(may_refresh(auth(now + 60, recent).as_bytes(), now));
        assert!(may_refresh(auth(now - 60, recent).as_bytes(), now));
        assert!(may_refresh(
            auth(now + 86_400, "2027-01-01T00:00:00Z").as_bytes(),
            now
        ));
        // Equivalent UTC offsets and fractional seconds keep the same refresh decision.
        assert!(!may_refresh(
            auth(now + 86_400, "2027-01-14T08:00:00.123+08:00").as_bytes(),
            now
        ));
        assert!(may_refresh(
            auth(now + 86_400, "not-a-timestamp").as_bytes(),
            now
        ));
        assert!(may_refresh(b"{}", now));
        assert!(may_refresh(b"nope", now));
    }

    #[test]
    fn debug_redacts_the_tokens() {
        let c = Copied {
            real: PathBuf::from("/r/auth.json"),
            ours: PathBuf::from("/s/auth.json"),
            orig: br#"{"tokens":{"access_token":"eyJsecret","refresh_token":"rt-secret"}}"#
                .to_vec(),
        };
        let d = format!("{c:?}");
        assert!(!d.contains("secret") && !d.contains("eyJ"), "{d}");
        let n = c.orig_len();
        assert!(d.contains(&format!("[redacted; {n} bytes]")), "{d}");
        assert!(d.contains("/r/auth.json"), "{d}");
    }

    #[tokio::test]
    async fn copies_in_and_back() {
        use std::os::unix::fs::PermissionsExt as _;
        let d = tempfile::tempdir().unwrap();
        let real_home = d.path().join("real");
        tokio::fs::create_dir(&real_home).await.unwrap();
        tokio::fs::write(real_home.join("auth.json"), b"{\"a\":1}")
            .await
            .unwrap();
        let home = d.path().join("session/home");
        let c = copy_in(&real_home, &home).await.unwrap();
        let mode = tokio::fs::metadata(&c.ours)
            .await
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
        assert!(!copy_back(&c).await.unwrap(), "untouched: nothing to copy");
        tokio::fs::write(&c.ours, b"{\"a\":2}").await.unwrap();
        assert!(copy_back(&c).await.unwrap());
        assert_eq!(tokio::fs::read(&c.real).await.unwrap(), b"{\"a\":2}");
        // The real one changed since: leave it.
        let c2 = copy_in(&real_home, &home).await.unwrap();
        tokio::fs::write(&c2.real, b"{\"other\":1}").await.unwrap();
        tokio::fs::write(&c2.ours, b"{\"a\":3}").await.unwrap();
        assert!(!copy_back(&c2).await.unwrap());
        assert_eq!(tokio::fs::read(&c2.real).await.unwrap(), b"{\"other\":1}");
    }
}
