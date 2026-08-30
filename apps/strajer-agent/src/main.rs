use std::env;
use std::io::{self, Write};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use reqwest::Client;
use serde::Serialize;
use strajer_lan::PublishedLobby;
use strajer_protocol::{LobbyCatalog, LobbyDescriptor};
use tokio::io::AsyncReadExt;
use tokio::net::{TcpListener, TcpStream};
use tokio::task::JoinHandle;
use tokio::time::timeout;
use tracing::{info, warn};
use tracing_subscriber::EnvFilter;

const DEFAULT_SERVER_URL: &str = "http://127.0.0.1:18080";
const CATALOG_TIMEOUT: Duration = Duration::from_secs(5);
const JOIN_PROBE_TIMEOUT: Duration = Duration::from_secs(3);
const MAX_JOIN_PROBE_BYTES: usize = 4_096;

#[derive(Debug, Serialize)]
struct AgentStatusEvent {
    event: &'static str,
    lobby_count: usize,
}

impl AgentStatusEvent {
    fn ready(lobby_count: usize) -> Self {
        Self {
            event: "ready",
            lobby_count,
        }
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    initialize_tracing();

    let server_url = server_url()?;
    let catalog = fetch_catalog(&server_url).await?;
    catalog
        .validate()
        .context("server returned an invalid lobby catalog")?;

    if catalog.lobbies.is_empty() {
        bail!("server returned an empty lobby catalog");
    }

    let mut published_lobbies = Vec::with_capacity(catalog.lobbies.len());
    let mut listener_tasks = Vec::with_capacity(catalog.lobbies.len());

    for lobby in catalog.lobbies {
        let (publisher, listener_task) = activate_lobby(lobby).await?;
        published_lobbies.push(publisher);
        listener_tasks.push(listener_task);
    }

    info!(
        lobby_count = published_lobbies.len(),
        "lobby catalog is published; open Warcraft III > Local Area Network"
    );
    emit_status_event(&AgentStatusEvent::ready(published_lobbies.len()))?;
    wait_for_shutdown().await?;

    for task in listener_tasks {
        task.abort();
    }
    drop(published_lobbies);
    info!("local lobby advertisements removed");

    Ok(())
}

async fn fetch_catalog(server_url: &str) -> Result<LobbyCatalog> {
    let endpoint = format!("{}/v1/lobbies", server_url.trim_end_matches('/'));
    let client = Client::builder()
        .timeout(CATALOG_TIMEOUT)
        .user_agent(concat!("strajer-agent/", env!("CARGO_PKG_VERSION")))
        .build()
        .context("could not build HTTP client")?;
    let response = client
        .get(&endpoint)
        .send()
        .await
        .with_context(|| format!("could not fetch {endpoint}"))?
        .error_for_status()
        .with_context(|| format!("catalog endpoint returned an error: {endpoint}"))?;

    response
        .json::<LobbyCatalog>()
        .await
        .context("could not decode lobby catalog")
}

async fn activate_lobby(
    lobby: LobbyDescriptor,
) -> Result<(PublishedLobby, JoinHandle<Result<()>>)> {
    let listener = TcpListener::bind(("0.0.0.0", 0))
        .await
        .with_context(|| format!("could not bind local listener for lobby {}", lobby.id))?;
    let local_address = listener
        .local_addr()
        .with_context(|| format!("could not inspect local listener for lobby {}", lobby.id))?;
    let publisher = PublishedLobby::publish(&lobby, local_address.port())
        .await
        .with_context(|| format!("could not publish lobby {}", lobby.id))?;

    info!(
        lobby_id = %lobby.id,
        game_name = %lobby.name,
        local_port = local_address.port(),
        "published Warcraft LAN lobby"
    );

    let listener_task = tokio::spawn(accept_connections(lobby.id, listener));
    Ok((publisher, listener_task))
}

async fn accept_connections(lobby_id: String, listener: TcpListener) -> Result<()> {
    loop {
        let (stream, peer_address) = listener
            .accept()
            .await
            .with_context(|| format!("listener failed for lobby {lobby_id}"))?;
        let connection_lobby_id = lobby_id.clone();
        tokio::spawn(probe_join_connection(
            connection_lobby_id,
            peer_address.to_string(),
            stream,
        ));
    }
}

async fn probe_join_connection(lobby_id: String, peer_address: String, mut stream: TcpStream) {
    let mut buffer = vec![0_u8; MAX_JOIN_PROBE_BYTES];
    let read_result = timeout(JOIN_PROBE_TIMEOUT, stream.read(&mut buffer)).await;

    match read_result {
        Ok(Ok(bytes_read)) if bytes_read > 0 => {
            info!(
                %lobby_id,
                %peer_address,
                bytes_read,
                packet_prefix = %hex::encode(&buffer[..bytes_read.min(64)]),
                "captured initial Warcraft join packet"
            );
            warn!(%lobby_id, "W3GS join forwarding is not implemented in milestone M0");
        }
        Ok(Ok(_)) => {
            info!(%lobby_id, %peer_address, "Warcraft closed the join connection");
        }
        Ok(Err(error)) => {
            warn!(%lobby_id, %peer_address, %error, "could not read Warcraft join packet");
        }
        Err(_) => {
            warn!(%lobby_id, %peer_address, "Warcraft join probe timed out");
        }
    }
}

async fn wait_for_shutdown() -> Result<()> {
    tokio::select! {
        result = tokio::signal::ctrl_c() => {
            result.context("could not install Ctrl-C handler")?;
        }
        result = wait_for_parent_disconnect() => {
            result?;
        }
    }

    Ok(())
}

async fn wait_for_parent_disconnect() -> Result<()> {
    let mut stdin = tokio::io::stdin();
    let mut buffer = [0_u8; 1];

    loop {
        let bytes_read = stdin
            .read(&mut buffer)
            .await
            .context("could not monitor parent process")?;
        if bytes_read == 0 {
            return Ok(());
        }
    }
}

fn server_url() -> Result<String> {
    match env::var("STRAJER_SERVER_URL") {
        Ok(value) if !value.trim().is_empty() => Ok(value),
        Ok(_) => bail!("STRAJER_SERVER_URL must not be empty"),
        Err(env::VarError::NotPresent) => Ok(DEFAULT_SERVER_URL.to_owned()),
        Err(error) => Err(error).context("could not read STRAJER_SERVER_URL"),
    }
}

fn initialize_tracing() {
    let filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("strajer_agent=info"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_ansi(false)
        .with_writer(io::stderr)
        .init();
}

fn emit_status_event(event: &AgentStatusEvent) -> Result<()> {
    let stdout = io::stdout();
    let mut writer = stdout.lock();
    write_status_event(&mut writer, event).context("could not emit agent status")
}

fn write_status_event<W: Write>(writer: &mut W, event: &AgentStatusEvent) -> Result<()> {
    serde_json::to_writer(&mut *writer, event).context("could not encode agent status")?;
    writeln!(writer).context("could not terminate agent status line")?;
    writer.flush().context("could not flush agent status")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_machine_readable_ready_status() {
        let mut output = Vec::new();
        write_status_event(&mut output, &AgentStatusEvent::ready(3))
            .expect("status should serialize");

        assert_eq!(
            String::from_utf8(output).expect("status should be UTF-8"),
            "{\"event\":\"ready\",\"lobby_count\":3}\n"
        );
    }
}
