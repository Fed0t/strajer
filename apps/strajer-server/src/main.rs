use std::env;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use strajer_server::{AppState, ServerConfig, router};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::time::timeout;
use tracing::info;
use tracing_subscriber::EnvFilter;

const DEFAULT_HEALTHCHECK_PORT: u16 = 8_080;
const HEALTHCHECK_TIMEOUT: Duration = Duration::from_secs(3);
const HTTP_OK_STATUS_PREFIX: &[u8] = b"HTTP/1.1 200";

#[tokio::main]
async fn main() -> Result<()> {
    initialize_tracing();

    match env::args().nth(1).as_deref() {
        Some("healthcheck") => run_healthcheck().await,
        Some(argument) => bail!("unsupported argument: {argument}"),
        None => run_server().await,
    }
}

async fn run_server() -> Result<()> {
    let config = ServerConfig::from_environment()?;
    let state = AppState::synthetic()
        .context("could not initialize lobby catalog")?
        .with_join_token(config.join_token().map(str::to_owned));
    let listener = TcpListener::bind(config.bind_address)
        .await
        .with_context(|| format!("could not bind {}", config.bind_address))?;

    info!(address = %config.bind_address, "strajer server listening");
    axum::serve(listener, router(state))
        .with_graceful_shutdown(shutdown_signal())
        .await
        .context("HTTP server failed")
}

async fn run_healthcheck() -> Result<()> {
    let address = healthcheck_address()?;
    timeout(HEALTHCHECK_TIMEOUT, check_ready_endpoint(address))
        .await
        .context("healthcheck timed out")??;
    Ok(())
}

fn healthcheck_address() -> Result<SocketAddr> {
    match env::var("STRAJER_HEALTHCHECK_ADDR") {
        Ok(value) => value
            .parse::<SocketAddr>()
            .with_context(|| format!("invalid STRAJER_HEALTHCHECK_ADDR: {value}")),
        Err(env::VarError::NotPresent) => Ok(SocketAddr::new(
            IpAddr::V4(Ipv4Addr::LOCALHOST),
            DEFAULT_HEALTHCHECK_PORT,
        )),
        Err(error) => Err(error).context("could not read STRAJER_HEALTHCHECK_ADDR"),
    }
}

async fn check_ready_endpoint(address: SocketAddr) -> Result<()> {
    let mut stream = TcpStream::connect(address)
        .await
        .with_context(|| format!("could not connect to {address}"))?;
    stream
        .write_all(b"GET /readyz HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .await
        .context("could not write healthcheck request")?;

    let mut response = [0_u8; 64];
    let mut bytes_read = 0_usize;
    while bytes_read < HTTP_OK_STATUS_PREFIX.len() {
        let read_count = stream
            .read(&mut response[bytes_read..])
            .await
            .context("could not read healthcheck response")?;
        if read_count == 0 {
            bail!("readiness endpoint closed the connection before returning a status");
        }
        bytes_read += read_count;
    }

    if !response[..bytes_read].starts_with(HTTP_OK_STATUS_PREFIX) {
        bail!("readiness endpoint did not return HTTP 200");
    }

    Ok(())
}

async fn shutdown_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};

        let mut terminate = match signal(SignalKind::terminate()) {
            Ok(signal) => signal,
            Err(error) => {
                tracing::error!(%error, "SIGTERM handler could not be installed");
                wait_for_ctrl_c().await;
                return;
            }
        };
        tokio::select! {
            _ = wait_for_ctrl_c() => {}
            _ = terminate.recv() => {}
        }
    }

    #[cfg(not(unix))]
    {
        wait_for_ctrl_c().await;
    }

    info!("shutdown signal received");
}

async fn wait_for_ctrl_c() {
    if let Err(error) = tokio::signal::ctrl_c().await {
        tracing::error!(%error, "Ctrl-C handler failed");
    }
}

fn initialize_tracing() {
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("strajer_server=info,tower_http=info"));
    tracing_subscriber::fmt().with_env_filter(filter).init();
}
