use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use axum::Json;
use axum::body::Body;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, State};
use axum::http::HeaderMap;
use axum::http::header::{
    AUTHORIZATION, CACHE_CONTROL, CONTENT_LENGTH, CONTENT_TYPE, ETAG, X_CONTENT_TYPE_OPTIONS,
};
use axum::response::Response;
use axum::routing::get;
use axum::{Router, http::StatusCode};
use serde::Serialize;
use strajer_protocol::{
    AgentLobbyMessage, LOBBY_SESSION_PROTOCOL_VERSION, LobbyCatalog, LobbyJoinRejection,
    LobbySessionValidationError, MAX_LOBBY_CONTROL_MESSAGE_BYTES, ServerLobbyMessage,
};
use tokio::sync::broadcast;
use tokio::time::timeout;
use tokio_util::io::ReaderStream;
use tower_http::trace::TraceLayer;
use tracing::{error, info, warn};

use crate::AppState;
use crate::lobby::{LobbyJoinError, LobbyMembership, LobbyPhase, LobbyRoom, LobbyUpdate};

const LOBBY_JOIN_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, Serialize)]
struct HealthResponse {
    status: &'static str,
    service: &'static str,
    version: &'static str,
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/healthz", get(health))
        .route("/readyz", get(readiness))
        .route("/v1/lobbies", get(lobbies))
        .route("/v1/lobbies/{lobby_id}/session", get(lobby_session))
        .route("/v1/maps/{sha1_hex}", get(map_download))
        .with_state(state)
        .layer(TraceLayer::new_for_http())
}

async fn health() -> Json<HealthResponse> {
    Json(HealthResponse {
        status: "ok",
        service: "strajer-server",
        version: env!("CARGO_PKG_VERSION"),
    })
}

async fn readiness(State(state): State<AppState>) -> (StatusCode, Json<HealthResponse>) {
    let status = if state.catalog().validate().is_ok() && state.has_required_map_assets() {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };

    (
        status,
        Json(HealthResponse {
            status: if status == StatusCode::OK {
                "ready"
            } else {
                "not_ready"
            },
            service: "strajer-server",
            version: env!("CARGO_PKG_VERSION"),
        }),
    )
}

async fn lobbies(State(state): State<AppState>) -> Json<LobbyCatalog> {
    Json(state.catalog().clone())
}

async fn map_download(
    Path(sha1_hex): Path<String>,
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Response, StatusCode> {
    let authorization = headers
        .get(AUTHORIZATION)
        .and_then(|value| value.to_str().ok());
    if !state.authorizes_lobby_session(authorization) {
        return Err(StatusCode::UNAUTHORIZED);
    }

    let asset = state.map_asset(&sha1_hex).ok_or(StatusCode::NOT_FOUND)?;
    let file = tokio::fs::File::open(asset.path()).await.map_err(|open_error| {
        error!(map_sha1 = %asset.sha1_hex(), %open_error, "configured map asset could not be opened");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    let body = Body::from_stream(ReaderStream::new(file));

    Response::builder()
        .status(StatusCode::OK)
        .header(CONTENT_TYPE, "application/octet-stream")
        .header(CONTENT_LENGTH, asset.file_size().to_string())
        .header(ETAG, format!("\"{}\"", asset.sha1_hex()))
        .header(CACHE_CONTROL, "private, max-age=31536000, immutable")
        .header(X_CONTENT_TYPE_OPTIONS, "nosniff")
        .body(body)
        .map_err(|response_error| {
            error!(%response_error, "could not build map download response");
            StatusCode::INTERNAL_SERVER_ERROR
        })
}

async fn lobby_session(
    Path(lobby_id): Path<String>,
    State(state): State<AppState>,
    headers: HeaderMap,
    websocket: WebSocketUpgrade,
) -> Result<Response, StatusCode> {
    let authorization = headers
        .get(AUTHORIZATION)
        .and_then(|value| value.to_str().ok());
    if !state.authorizes_lobby_session(authorization) {
        return Err(StatusCode::UNAUTHORIZED);
    }

    let room = state.lobby_room(&lobby_id).ok_or(StatusCode::NOT_FOUND)?;

    Ok(websocket
        .max_message_size(MAX_LOBBY_CONTROL_MESSAGE_BYTES)
        .max_frame_size(MAX_LOBBY_CONTROL_MESSAGE_BYTES)
        .on_upgrade(move |socket| serve_lobby_socket(socket, room, lobby_id)))
}

async fn serve_lobby_socket(mut socket: WebSocket, room: Arc<LobbyRoom>, lobby_id: String) {
    let join_message = match receive_join_message(&mut socket).await {
        Ok(message) => message,
        Err(rejection) => {
            let _ = send_server_message(
                &mut socket,
                &ServerLobbyMessage::Rejected { code: rejection },
            )
            .await;
            return;
        }
    };

    let player_name = match join_message.join_player_name() {
        Some(player_name) => player_name.to_owned(),
        None => {
            let _ = send_server_message(
                &mut socket,
                &ServerLobbyMessage::Rejected {
                    code: LobbyJoinRejection::InvalidRequest,
                },
            )
            .await;
            return;
        }
    };

    let membership = match room.join(player_name).await {
        Ok(membership) => membership,
        Err(error) => {
            let rejection = map_join_error(&error);
            let _ = send_server_message(
                &mut socket,
                &ServerLobbyMessage::Rejected { code: rejection },
            )
            .await;
            return;
        }
    };

    let player_id = membership.player_id;
    let session_id = membership.session_id;
    info!(%lobby_id, player_id, "player joined coordinated lobby");

    if let Err(error) = run_connected_lobby_socket(&mut socket, &room, membership).await {
        warn!(%lobby_id, player_id, %error, "coordinated lobby socket ended with an error");
    }

    room.leave(player_id, session_id).await;
    info!(%lobby_id, player_id, "player left coordinated lobby");
}

async fn receive_join_message(
    socket: &mut WebSocket,
) -> Result<AgentLobbyMessage, LobbyJoinRejection> {
    let message = timeout(LOBBY_JOIN_TIMEOUT, socket.recv())
        .await
        .map_err(|_| LobbyJoinRejection::InvalidRequest)?
        .ok_or(LobbyJoinRejection::InvalidRequest)?
        .map_err(|_| LobbyJoinRejection::InvalidRequest)?;

    let Message::Text(text) = message else {
        return Err(LobbyJoinRejection::InvalidRequest);
    };
    if text.len() > MAX_LOBBY_CONTROL_MESSAGE_BYTES {
        return Err(LobbyJoinRejection::InvalidRequest);
    }

    let message = serde_json::from_str::<AgentLobbyMessage>(&text)
        .map_err(|_| LobbyJoinRejection::InvalidRequest)?;
    message.validate().map_err(map_validation_error)?;
    Ok(message)
}

async fn run_connected_lobby_socket(
    socket: &mut WebSocket,
    room: &Arc<LobbyRoom>,
    mut membership: LobbyMembership,
) -> Result<()> {
    send_server_message(
        socket,
        &ServerLobbyMessage::Joined {
            protocol_version: LOBBY_SESSION_PROTOCOL_VERSION,
            player_id: membership.player_id,
            roster: membership.roster.clone(),
        },
    )
    .await?;

    loop {
        tokio::select! {
            received = socket.recv() => {
                match received {
                    Some(Ok(Message::Ping(payload))) => {
                        socket.send(Message::Pong(payload)).await.context("could not send WebSocket pong")?;
                    }
                    Some(Ok(Message::Pong(_))) => {}
                    Some(Ok(Message::Close(_))) | None => return Ok(()),
                    Some(Ok(Message::Text(text))) => {
                        handle_active_agent_message(
                            room,
                            membership.player_id,
                            membership.session_id,
                            &text,
                        )
                        .await?;
                    }
                    Some(Ok(Message::Binary(_))) => {
                        bail!("unexpected binary client message after lobby join");
                    }
                    Some(Err(error)) => return Err(error).context("could not read lobby WebSocket"),
                }
            }
            update = membership.updates.recv() => {
                let message = match update {
                    Ok(LobbyUpdate::Roster(roster)) => ServerLobbyMessage::Roster { roster },
                    Ok(LobbyUpdate::Countdown { remaining_seconds }) => {
                        ServerLobbyMessage::Countdown { remaining_seconds }
                    }
                    Ok(LobbyUpdate::CountdownCancelled) => {
                        ServerLobbyMessage::CountdownCancelled
                    }
                    Ok(LobbyUpdate::Start) => ServerLobbyMessage::Start,
                    Err(broadcast::error::RecvError::Lagged(skipped_updates)) => {
                        warn!(
                            player_id = membership.player_id,
                            skipped_updates,
                            "lobby client lagged behind control updates; sending authoritative snapshot"
                        );
                        send_lobby_control_snapshot(socket, room).await?;
                        continue;
                    }
                    Err(broadcast::error::RecvError::Closed) => return Ok(()),
                };
                send_server_message(socket, &message).await?;
            }
        }
    }
}

async fn send_lobby_control_snapshot(socket: &mut WebSocket, room: &LobbyRoom) -> Result<()> {
    let snapshot = room.control_snapshot().await;
    send_server_message(
        socket,
        &ServerLobbyMessage::Roster {
            roster: snapshot.roster,
        },
    )
    .await?;

    match snapshot.phase {
        LobbyPhase::Waiting => Ok(()),
        LobbyPhase::Countdown { remaining_seconds } => {
            send_server_message(socket, &ServerLobbyMessage::Countdown { remaining_seconds }).await
        }
        LobbyPhase::Started => send_server_message(socket, &ServerLobbyMessage::Start).await,
    }
}

async fn handle_active_agent_message(
    room: &Arc<LobbyRoom>,
    player_id: u8,
    session_id: u64,
    text: &str,
) -> Result<()> {
    if text.len() > MAX_LOBBY_CONTROL_MESSAGE_BYTES {
        bail!("lobby client message exceeds configured limit");
    }

    let message = serde_json::from_str::<AgentLobbyMessage>(text)
        .context("could not decode active lobby client message")?;
    message
        .validate()
        .context("active lobby client message failed validation")?;

    match message {
        AgentLobbyMessage::Ready { .. } => room
            .mark_ready(player_id, session_id)
            .await
            .context("could not mark lobby player ready"),
        AgentLobbyMessage::Join { .. } => bail!("duplicate lobby join message"),
    }
}

async fn send_server_message(socket: &mut WebSocket, message: &ServerLobbyMessage) -> Result<()> {
    let text = serde_json::to_string(message).context("could not encode lobby control message")?;
    if text.len() > MAX_LOBBY_CONTROL_MESSAGE_BYTES {
        bail!("encoded lobby control message exceeds configured limit");
    }

    socket
        .send(Message::Text(text.into()))
        .await
        .context("could not send lobby control message")
}

fn map_validation_error(error: LobbySessionValidationError) -> LobbyJoinRejection {
    match error {
        LobbySessionValidationError::UnsupportedProtocolVersion { .. } => {
            LobbyJoinRejection::UnsupportedProtocol
        }
        LobbySessionValidationError::InvalidPlayerName => LobbyJoinRejection::InvalidPlayerName,
        _ => LobbyJoinRejection::InvalidRequest,
    }
}

fn map_join_error(error: &LobbyJoinError) -> LobbyJoinRejection {
    match error {
        LobbyJoinError::Full => LobbyJoinRejection::LobbyFull,
        LobbyJoinError::Started => LobbyJoinRejection::LobbyStarted,
        LobbyJoinError::DuplicatePlayerName => LobbyJoinRejection::DuplicatePlayerName,
        LobbyJoinError::SessionIdExhausted => LobbyJoinRejection::InvalidRequest,
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    use axum::body::{Body, to_bytes};
    use axum::http::{Request, StatusCode};
    use futures_util::{SinkExt, StreamExt};
    use strajer_protocol::{
        AgentLobbyMessage, CATALOG_SCHEMA_VERSION, LobbyCatalog, ServerLobbyMessage,
    };
    use tokio::net::{TcpListener, TcpStream};
    use tokio::task::JoinHandle;
    use tokio_tungstenite::tungstenite::Message as ClientMessage;
    use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, connect_async};
    use tower::ServiceExt;

    use super::*;

    const TEST_JOIN_TOKEN: &str =
        "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
    static TEST_FILE_SEQUENCE: AtomicU64 = AtomicU64::new(1);

    #[tokio::test]
    async fn serves_a_valid_synthetic_catalog() {
        let state = AppState::synthetic_at(2_000, 2).expect("state should be valid");
        let response = router(state)
            .oneshot(
                Request::builder()
                    .uri("/v1/lobbies")
                    .body(Body::empty())
                    .expect("request should build"),
            )
            .await
            .expect("router should answer");

        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), 64 * 1_024)
            .await
            .expect("body should be readable");
        let catalog: LobbyCatalog =
            serde_json::from_slice(&body).expect("catalog should deserialize");

        assert_eq!(catalog.schema_version, CATALOG_SCHEMA_VERSION);
        assert_eq!(catalog.generated_at_unix_ms, 2_000);
        assert_eq!(catalog.lobbies.len(), 1);
        assert_eq!(catalog.lobbies[0].name, "Strajer Test #1");
        assert_eq!(
            catalog.lobbies[0].map.path,
            "Maps\\Download\\DotA_v6_89Q.w3x"
        );
        assert_eq!(
            catalog.lobbies[0].map.sha1_hex,
            "c771ac8d7dc3665a211c2b1432672d49bfba1bcf"
        );
        assert_eq!(catalog.lobbies[0].map.file_size, 35_053_979);
        assert_eq!(catalog.lobbies[0].map.file_crc32, 2_194_498_669);
        assert_eq!(catalog.lobbies[0].map.checksum, 448_311_427);
        assert_eq!(catalog.lobbies[0].map.width, 128);
        assert_eq!(catalog.lobbies[0].map.height, 128);
        assert_eq!(catalog.lobbies[0].players.current, 1);
        assert_eq!(catalog.lobbies[0].players.max, 11);
        assert_eq!(catalog.lobbies[0].human_player_capacity(), 10);
        assert_eq!(catalog.lobbies[0].virtual_host.player_id, 11);
        assert_eq!(catalog.lobbies[0].virtual_host.slot_index, 10);
        assert_eq!(catalog.lobbies[0].virtual_host.name, "Strajer");
        assert_eq!(catalog.validate(), Ok(()));
    }

    #[tokio::test]
    async fn reports_not_ready_without_a_registered_map_asset() {
        let state = AppState::synthetic_at(2_000, 2).expect("state should be valid");
        let response = router(state)
            .oneshot(
                Request::builder()
                    .uri("/readyz")
                    .body(Body::empty())
                    .expect("request should build"),
            )
            .await
            .expect("router should answer");

        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    #[tokio::test]
    async fn serves_the_authenticated_map_asset_with_integrity_headers() {
        let map_path = test_map_path();
        fs::write(&map_path, b"123456789").expect("test map should write");
        let mut catalog = AppState::synthetic_at(2_000, 2)
            .expect("state should be valid")
            .catalog()
            .clone();
        catalog.lobbies[0].map.file_size = 9;
        catalog.lobbies[0].map.file_crc32 = 0xCBF4_3926;
        catalog.lobbies[0].map.sha1_hex = "f7c3bc1d808e04732adf679965ccc34ca7ae3441".to_owned();
        let state = AppState::from_catalog(catalog)
            .expect("test catalog should be valid")
            .with_map_file(&map_path)
            .expect("test map should register")
            .with_join_token(Some(TEST_JOIN_TOKEN.to_owned()));
        let application = router(state);

        let ready = application
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/readyz")
                    .body(Body::empty())
                    .expect("request should build"),
            )
            .await
            .expect("router should answer");
        assert_eq!(ready.status(), StatusCode::OK);

        let unauthorized = application
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/v1/maps/f7c3bc1d808e04732adf679965ccc34ca7ae3441")
                    .body(Body::empty())
                    .expect("request should build"),
            )
            .await
            .expect("router should answer");
        assert_eq!(unauthorized.status(), StatusCode::UNAUTHORIZED);

        let response = application
            .oneshot(
                Request::builder()
                    .uri("/v1/maps/f7c3bc1d808e04732adf679965ccc34ca7ae3441")
                    .header(AUTHORIZATION, format!("Bearer {TEST_JOIN_TOKEN}"))
                    .body(Body::empty())
                    .expect("request should build"),
            )
            .await
            .expect("router should answer");

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()[CONTENT_TYPE], "application/octet-stream");
        assert_eq!(response.headers()[CONTENT_LENGTH], "9");
        assert_eq!(
            response.headers()[ETAG],
            "\"f7c3bc1d808e04732adf679965ccc34ca7ae3441\""
        );
        let body = to_bytes(response.into_body(), 16)
            .await
            .expect("map body should be readable");
        assert_eq!(&body[..], b"123456789");
        fs::remove_file(map_path).expect("test map should be removed");
    }

    #[tokio::test]
    async fn synchronizes_two_clients_and_releases_the_departed_slot() {
        let (endpoint, server_task) = start_websocket_server().await;
        let mut first = connect_lobby_client(&endpoint, "First#1000").await;
        let first_join = receive_joined(&mut first).await;
        assert_eq!(first_join.0, 1);
        assert_eq!(first_join.1.players.len(), 1);

        let mut second = connect_lobby_client(&endpoint, "Second#2000").await;
        let second_join = receive_joined(&mut second).await;
        assert_eq!(second_join.0, 2);
        assert_eq!(second_join.1.players.len(), 2);

        let first_two_player_roster = receive_roster_with_count(&mut first, 2).await;
        assert_eq!(first_two_player_roster, second_join.1);

        second
            .send(ClientMessage::Close(None))
            .await
            .expect("second client should close");
        drop(second);
        let first_after_leave = receive_roster_with_count(&mut first, 1).await;
        assert_eq!(first_after_leave.players[0].player_id, 1);

        server_task.abort();
    }

    #[tokio::test]
    async fn accepts_ready_and_publishes_the_initial_countdown() {
        let mut catalog = AppState::synthetic_at(2_000, 2)
            .expect("state should be valid")
            .catalog()
            .clone();
        catalog.lobbies[0].players.max = 2;
        catalog.lobbies[0].virtual_host.player_id = 2;
        catalog.lobbies[0].virtual_host.slot_index = 1;
        let state = AppState::from_catalog(catalog).expect("one-player state should be valid");
        let (endpoint, server_task) = start_websocket_server_with_state(state).await;
        let mut player = connect_lobby_client(&endpoint, "Ready#1000").await;
        receive_joined(&mut player).await;

        let ready =
            serde_json::to_string(&AgentLobbyMessage::ready()).expect("ready should serialize");
        player
            .send(ClientMessage::Text(ready.into()))
            .await
            .expect("ready should send");

        let countdown = timeout(Duration::from_secs(1), async {
            loop {
                if let ServerLobbyMessage::Countdown { remaining_seconds } =
                    receive_control_message(&mut player).await
                {
                    return remaining_seconds;
                }
            }
        })
        .await
        .expect("initial countdown should arrive");
        assert_eq!(countdown, 60);
        server_task.abort();
    }

    type TestLobbySocket = WebSocketStream<MaybeTlsStream<TcpStream>>;

    async fn start_websocket_server() -> (String, JoinHandle<()>) {
        let state = AppState::synthetic_at(2_000, 2).expect("state should be valid");
        start_websocket_server_with_state(state).await
    }

    async fn start_websocket_server_with_state(state: AppState) -> (String, JoinHandle<()>) {
        let listener = TcpListener::bind(("127.0.0.1", 0))
            .await
            .expect("test listener should bind");
        let address = listener
            .local_addr()
            .expect("test listener address should be available");
        let task = tokio::spawn(async move {
            axum::serve(listener, router(state))
                .await
                .expect("test server should run");
        });
        (
            format!("ws://{address}/v1/lobbies/synthetic-1/session"),
            task,
        )
    }

    async fn connect_lobby_client(endpoint: &str, player_name: &str) -> TestLobbySocket {
        let (mut socket, _) = connect_async(endpoint)
            .await
            .expect("test WebSocket should connect");
        let join = AgentLobbyMessage::join(player_name.to_owned())
            .expect("test player name should be valid");
        let text = serde_json::to_string(&join).expect("join should serialize");
        socket
            .send(ClientMessage::Text(text.into()))
            .await
            .expect("join should send");
        socket
    }

    async fn receive_joined(socket: &mut TestLobbySocket) -> (u8, strajer_protocol::LobbyRoster) {
        match receive_control_message(socket).await {
            ServerLobbyMessage::Joined {
                protocol_version,
                player_id,
                roster,
            } => {
                assert_eq!(protocol_version, LOBBY_SESSION_PROTOCOL_VERSION);
                (player_id, roster)
            }
            message => panic!("expected joined message, got {message:?}"),
        }
    }

    async fn receive_roster_with_count(
        socket: &mut TestLobbySocket,
        expected_count: usize,
    ) -> strajer_protocol::LobbyRoster {
        timeout(Duration::from_secs(2), async {
            loop {
                if let ServerLobbyMessage::Roster { roster } = receive_control_message(socket).await
                    && roster.players.len() == expected_count
                {
                    return roster;
                }
            }
        })
        .await
        .expect("expected roster update should arrive")
    }

    async fn receive_control_message(socket: &mut TestLobbySocket) -> ServerLobbyMessage {
        let message = socket
            .next()
            .await
            .expect("WebSocket should remain open")
            .expect("WebSocket message should be valid");
        let ClientMessage::Text(text) = message else {
            panic!("expected text control message");
        };
        serde_json::from_str(&text).expect("control message should decode")
    }

    fn test_map_path() -> PathBuf {
        let sequence = TEST_FILE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!(
            "strajer-http-map-{}-{sequence}.w3x",
            std::process::id()
        ))
    }
}
