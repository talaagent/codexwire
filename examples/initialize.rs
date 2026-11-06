//! Start `codex app-server`, initialize, and print what it sends.

// `#[tokio::main]` expands to `block_on`: allowed in a main.
#![allow(clippy::disallowed_methods)]

use codexwire::client::{ServerMessage, spawn_app_server};
use codexwire::protocol::{ClientInfo, InitializeParams};

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn core::error::Error>> {
    let mut server = spawn_app_server("codex", &[], &[], std::path::Path::new("."))?;
    let init = server
        .client
        .request(&InitializeParams {
            capabilities: None,
            client_info: ClientInfo {
                name: "example".into(),
                title: None,
                version: "0.1.0".into(),
            },
        })
        .await?;
    println!("codex: {}", init.user_agent);
    server.client.notify("initialized", None).await?;
    // thread/start, turn/start (protocol::{ThreadStartParams, TurnStartParams}),
    // then read what the server sends:
    while let Some(msg) = server.messages.recv().await {
        if let ServerMessage::Notification { method, .. } = msg {
            println!("{method}");
        }
    }
    Ok(())
}
