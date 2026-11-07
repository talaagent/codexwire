//! Initialize an app-server and shut it down under a timeout.
#![expect(
    clippy::disallowed_methods,
    reason = "the executable entry point starts its Tokio runtime"
)]

use codexwire::client::spawn_app_server;
use codexwire::protocol::{ClientInfo, InitializeParams};
use core::time::Duration;

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn core::error::Error>> {
    let mut server = spawn_app_server("codex", &[], &[], ".".as_ref())?;
    let params = InitializeParams {
        capabilities: None,
        client_info: ClientInfo {
            name: "example".into(),
            title: None,
            version: "0.1.0".into(),
        },
    };
    let init =
        tokio::time::timeout(Duration::from_secs(10), server.client.request(&params)).await??;
    println!("codex: {}", init.user_agent);
    server.client.notify("initialized", None).await?;
    server.client.shutdown().await;
    let status = tokio::time::timeout(Duration::from_secs(10), server.child.wait()).await??;
    if !status.success() {
        return Err(std::io::Error::other(format!("app-server exited with {status}")).into());
    }
    Ok(())
}
