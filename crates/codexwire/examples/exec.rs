//! Run a prompt on an existing ChatGPT login, in a temporary private home.
#![expect(
    clippy::disallowed_methods,
    reason = "the executable entry point starts its Tokio runtime"
)]

use std::{io, path::PathBuf};

use codexwire::exec::{Exec, Input};

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn core::error::Error>> {
    let codex_home = std::env::var_os("CODEX_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".codex")))
        .ok_or_else(|| io::Error::other("set CODEX_HOME or HOME to find the existing login"))?;
    let temporary_root = tempfile::tempdir()?;
    let run = Exec::new("codex", codex_home, temporary_root.path())
        .run(&Input {
            prompt: std::env::args()
                .nth(1)
                .unwrap_or_else(|| "Say hello.".into()),
            images: Vec::new(),
        })
        .await?;
    if let Some(error) = run.failure() {
        return Err(io::Error::other(error.to_string()).into());
    }
    if run.code != Some(0) {
        return Err(io::Error::other(format!("codex exited with {:?}", run.code)).into());
    }
    if let Err(error) = &run.copied_back {
        return Err(io::Error::other(format!("saving refreshed login: {error}")).into());
    }
    println!("{}", String::from_utf8_lossy(&run.last_message));
    Ok(())
}
