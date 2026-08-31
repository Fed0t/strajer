use std::time::Duration;

use anyhow::{Context, Result, bail};
use futures_util::{SinkExt, StreamExt};
use strajer_protocol::{
    AgentLobbyMessage, LOBBY_SESSION_PROTOCOL_VERSION, LobbyDescriptor, LobbyRoster,
    MAX_LOBBY_CONTROL_MESSAGE_BYTES, ServerLobbyMessage, validate_lobby_chat_message,
    validate_lobby_countdown_seconds,
};
use tokio::net::TcpStream;
use tokio::time::{Instant, Interval, MissedTickBehavior, interval_at, timeout};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::header::AUTHORIZATION;
use tokio_tungstenite::tungstenite::http::{HeaderValue, Request};
use tokio_tungstenite::tungstenite::protocol::WebSocketConfig;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, connect_async_with_config};
use url::Url;

const REMOTE_CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const REMOTE_JOIN_TIMEOUT: Duration = Duration::from_secs(5);
const REMOTE_HEARTBEAT_INTERVAL: Duration = Duration::from_secs(30);

type LobbyWebSocket = WebSocketStream<MaybeTlsStream<TcpStream>>;

pub(crate) struct RemoteLobbySession {
    socket: LobbyWebSocket,
    assigned_player_id: u8,
    initial_roster: LobbyRoster,
    maximum_players: u8,
    heartbeat_interval: Interval,
    ready_sent: bool,
    loaded_sent: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum RemoteLobbyEvent {
    Roster(LobbyRoster),
    Countdown { remaining_seconds: u8 },
    CountdownCancelled,
    Chat { from_player_id: u8, message: String },
    Notice { message: String },
    Start,
    PlayerLoaded { player_id: u8 },
}

impl RemoteLobbySession {
    pub(crate) async fn connect(
        server_url: &str,
        lobby: &LobbyDescriptor,
        player_name: String,
        join_token: Option<&str>,
    ) -> Result<Self> {
        let endpoint = lobby_session_endpoint(server_url, &lobby.id)?;
        let request = websocket_request(&endpoint, join_token)?;
        let websocket_config = WebSocketConfig::default()
            .read_buffer_size(8_192)
            .write_buffer_size(8_192)
            .max_write_buffer_size(16_384)
            .max_message_size(Some(MAX_LOBBY_CONTROL_MESSAGE_BYTES))
            .max_frame_size(Some(MAX_LOBBY_CONTROL_MESSAGE_BYTES));
        let (mut socket, response) = timeout(
            REMOTE_CONNECT_TIMEOUT,
            connect_async_with_config(request, Some(websocket_config), false),
        )
        .await
        .context("coordinated lobby WebSocket connection timed out")?
        .with_context(|| format!("could not connect coordinated lobby at {endpoint}"))?;
        if response.status().as_u16() != 101 {
            bail!("coordinated lobby upgrade returned {}", response.status());
        }

        let join = AgentLobbyMessage::join(player_name.clone())
            .context("Warcraft player name is not valid for coordinated lobby")?;
        send_agent_message(&mut socket, &join).await?;

        let joined = timeout(REMOTE_JOIN_TIMEOUT, receive_server_message(&mut socket))
            .await
            .context("coordinated lobby join response timed out")??
            .context("coordinated lobby closed before accepting the player")?;
        let (protocol_version, assigned_player_id, roster) = match joined {
            ServerLobbyMessage::Joined {
                protocol_version,
                player_id,
                roster,
            } => (protocol_version, player_id, roster),
            ServerLobbyMessage::Rejected { code } => {
                bail!("coordinated lobby rejected the player: {code:?}")
            }
            ServerLobbyMessage::Roster { .. } => {
                bail!("coordinated lobby sent roster before join acceptance")
            }
            ServerLobbyMessage::Countdown { .. }
            | ServerLobbyMessage::CountdownCancelled
            | ServerLobbyMessage::Chat { .. }
            | ServerLobbyMessage::Notice { .. }
            | ServerLobbyMessage::Start
            | ServerLobbyMessage::PlayerLoaded { .. } => {
                bail!("coordinated lobby started control flow before join acceptance")
            }
        };

        if protocol_version != LOBBY_SESSION_PROTOCOL_VERSION {
            bail!(
                "coordinated lobby protocol {protocol_version} does not match agent protocol {LOBBY_SESSION_PROTOCOL_VERSION}"
            );
        }
        roster
            .validate(lobby.human_player_capacity())
            .context("coordinated lobby returned an invalid roster")?;
        let assigned_player = roster
            .player(assigned_player_id)
            .context("coordinated lobby roster does not contain the assigned player")?;
        if assigned_player.name != player_name {
            bail!("coordinated lobby assigned player name does not match the join request");
        }

        let mut heartbeat_interval = interval_at(
            Instant::now() + REMOTE_HEARTBEAT_INTERVAL,
            REMOTE_HEARTBEAT_INTERVAL,
        );
        heartbeat_interval.set_missed_tick_behavior(MissedTickBehavior::Delay);

        Ok(Self {
            socket,
            assigned_player_id,
            initial_roster: roster,
            maximum_players: lobby.human_player_capacity(),
            heartbeat_interval,
            ready_sent: false,
            loaded_sent: false,
        })
    }

    pub(crate) fn assigned_player_id(&self) -> u8 {
        self.assigned_player_id
    }

    pub(crate) fn initial_roster(&self) -> &LobbyRoster {
        &self.initial_roster
    }

    pub(crate) async fn mark_ready(&mut self) -> Result<()> {
        if self.ready_sent {
            return Ok(());
        }

        send_agent_message(&mut self.socket, &AgentLobbyMessage::ready()).await?;
        self.ready_sent = true;
        Ok(())
    }

    pub(crate) async fn send_chat(&mut self, message: String) -> Result<()> {
        let message =
            AgentLobbyMessage::chat(message).context("Warcraft lobby chat message is not valid")?;
        send_agent_message(&mut self.socket, &message).await
    }

    pub(crate) async fn mark_loaded(&mut self) -> Result<()> {
        if self.loaded_sent {
            return Ok(());
        }

        send_agent_message(&mut self.socket, &AgentLobbyMessage::loaded()).await?;
        self.loaded_sent = true;
        Ok(())
    }

    pub(crate) async fn next_event(&mut self) -> Result<Option<RemoteLobbyEvent>> {
        loop {
            tokio::select! {
                received = self.socket.next() => {
                    let Some(message) = received else {
                        return Ok(None);
                    };
                    let message = message.context(
                        "could not read coordinated lobby WebSocket"
                    )?;
                    let Some(message) = handle_server_websocket_message(
                        &mut self.socket,
                        message,
                    )
                    .await? else {
                        continue;
                    };

                    return self.validate_active_server_message(message);
                }
                _ = self.heartbeat_interval.tick() => {
                    self.socket
                        .send(Message::Ping(Vec::new().into()))
                        .await
                        .context("could not send coordinated lobby heartbeat")?;
                }
            }
        }
    }

    fn validate_active_server_message(
        &self,
        message: ServerLobbyMessage,
    ) -> Result<Option<RemoteLobbyEvent>> {
        match message {
            ServerLobbyMessage::Roster { roster } => {
                roster
                    .validate(self.maximum_players)
                    .context("coordinated lobby returned an invalid roster update")?;
                Ok(Some(RemoteLobbyEvent::Roster(roster)))
            }
            ServerLobbyMessage::Countdown { remaining_seconds } => {
                validate_lobby_countdown_seconds(remaining_seconds)
                    .context("coordinated lobby returned an invalid countdown")?;
                Ok(Some(RemoteLobbyEvent::Countdown { remaining_seconds }))
            }
            ServerLobbyMessage::CountdownCancelled => {
                Ok(Some(RemoteLobbyEvent::CountdownCancelled))
            }
            ServerLobbyMessage::Chat {
                from_player_id,
                message,
            } => {
                if from_player_id == 0 || from_player_id > self.maximum_players {
                    bail!("coordinated lobby returned an invalid chat sender");
                }
                validate_lobby_chat_message(&message)
                    .context("coordinated lobby returned an invalid chat message")?;
                Ok(Some(RemoteLobbyEvent::Chat {
                    from_player_id,
                    message,
                }))
            }
            ServerLobbyMessage::Notice { message } => {
                validate_lobby_chat_message(&message)
                    .context("coordinated lobby returned an invalid notice")?;
                Ok(Some(RemoteLobbyEvent::Notice { message }))
            }
            ServerLobbyMessage::Start => Ok(Some(RemoteLobbyEvent::Start)),
            ServerLobbyMessage::PlayerLoaded { player_id } => {
                if player_id == 0
                    || player_id > self.maximum_players
                    || player_id == self.assigned_player_id
                {
                    bail!("coordinated lobby returned an invalid loaded player");
                }
                Ok(Some(RemoteLobbyEvent::PlayerLoaded { player_id }))
            }
            ServerLobbyMessage::Rejected { code } => {
                bail!("coordinated lobby rejected the active session: {code:?}")
            }
            ServerLobbyMessage::Joined { .. } => {
                bail!("coordinated lobby sent duplicate join acceptance")
            }
        }
    }
}

fn websocket_request(endpoint: &Url, join_token: Option<&str>) -> Result<Request<()>> {
    let mut request = endpoint
        .as_str()
        .into_client_request()
        .context("could not create coordinated lobby WebSocket request")?;
    if let Some(join_token) = join_token {
        let authorization = HeaderValue::from_str(&format!("Bearer {join_token}"))
            .context("STRAJER_JOIN_TOKEN cannot be used in an HTTP header")?;
        request.headers_mut().insert(AUTHORIZATION, authorization);
    }

    Ok(request)
}

async fn send_agent_message(
    socket: &mut LobbyWebSocket,
    message: &AgentLobbyMessage,
) -> Result<()> {
    let text = serde_json::to_string(message).context("could not encode agent lobby message")?;
    if text.len() > MAX_LOBBY_CONTROL_MESSAGE_BYTES {
        bail!("encoded agent lobby message exceeds configured limit");
    }

    socket
        .send(Message::Text(text.into()))
        .await
        .context("could not send agent lobby message")
}

async fn receive_server_message(socket: &mut LobbyWebSocket) -> Result<Option<ServerLobbyMessage>> {
    loop {
        let Some(message) = socket.next().await else {
            return Ok(None);
        };
        let message = message.context("could not read coordinated lobby WebSocket")?;
        if let Some(message) = handle_server_websocket_message(socket, message).await? {
            return Ok(Some(message));
        }
    }
}

async fn handle_server_websocket_message(
    socket: &mut LobbyWebSocket,
    message: Message,
) -> Result<Option<ServerLobbyMessage>> {
    match message {
        Message::Text(text) => {
            if text.len() > MAX_LOBBY_CONTROL_MESSAGE_BYTES {
                bail!("coordinated lobby message exceeds configured limit");
            }
            let message = serde_json::from_str::<ServerLobbyMessage>(&text)
                .context("could not decode coordinated lobby message")?;
            Ok(Some(message))
        }
        Message::Ping(payload) => {
            socket
                .send(Message::Pong(payload))
                .await
                .context("could not send coordinated lobby pong")?;
            Ok(None)
        }
        Message::Pong(_) => Ok(None),
        Message::Close(_) => Ok(None),
        Message::Binary(_) | Message::Frame(_) => {
            bail!("coordinated lobby sent an unsupported WebSocket message")
        }
    }
}

fn lobby_session_endpoint(server_url: &str, lobby_id: &str) -> Result<Url> {
    let mut endpoint = Url::parse(server_url).context("STRAJER_SERVER_URL is not a valid URL")?;
    let websocket_scheme = match endpoint.scheme() {
        "http" => "ws",
        "https" => "wss",
        scheme => bail!("unsupported STRAJER_SERVER_URL scheme: {scheme}"),
    };
    endpoint
        .set_scheme(websocket_scheme)
        .map_err(|_| anyhow::anyhow!("could not set coordinated lobby URL scheme"))?;
    endpoint.set_query(None);
    endpoint.set_fragment(None);

    let mut path_segments = endpoint
        .path_segments_mut()
        .map_err(|_| anyhow::anyhow!("STRAJER_SERVER_URL cannot be used as a URL base"))?;
    path_segments.pop_if_empty();
    path_segments.extend(["v1", "lobbies", lobby_id, "session"]);
    drop(path_segments);

    Ok(endpoint)
}

#[cfg(test)]
mod tests {
    use strajer_server::{AppState, router};
    use tokio::net::TcpListener;

    use super::*;

    const TEST_JOIN_TOKEN: &str =
        "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    #[test]
    fn builds_secure_lobby_endpoint_and_escapes_the_lobby_id() {
        let endpoint = lobby_session_endpoint("https://strajer.example.com/", "lobby/one")
            .expect("endpoint should build");

        assert_eq!(
            endpoint.as_str(),
            "wss://strajer.example.com/v1/lobbies/lobby%2Fone/session"
        );
    }

    #[test]
    fn preserves_a_reverse_proxy_base_path() {
        let endpoint = lobby_session_endpoint("http://127.0.0.1:18080/strajer", "synthetic-1")
            .expect("endpoint should build");

        assert_eq!(
            endpoint.as_str(),
            "ws://127.0.0.1:18080/strajer/v1/lobbies/synthetic-1/session"
        );
    }

    #[test]
    fn adds_the_join_token_to_the_websocket_upgrade() {
        let endpoint = Url::parse("wss://strajer.example.com/v1/lobbies/test/session")
            .expect("endpoint should parse");
        let request =
            websocket_request(&endpoint, Some(TEST_JOIN_TOKEN)).expect("request should build");

        assert_eq!(
            request
                .headers()
                .get(AUTHORIZATION)
                .expect("authorization should be present"),
            &format!("Bearer {TEST_JOIN_TOKEN}")
        );
    }

    #[tokio::test]
    async fn agent_sessions_observe_the_same_two_player_roster() {
        let state = AppState::synthetic_at(2_000, 2)
            .expect("state should be valid")
            .with_join_token(Some(TEST_JOIN_TOKEN.to_owned()));
        let lobby = state.catalog().lobbies[0].clone();
        let listener = TcpListener::bind(("127.0.0.1", 0))
            .await
            .expect("test listener should bind");
        let address = listener
            .local_addr()
            .expect("test listener address should be available");
        let server_task = tokio::spawn(async move {
            axum::serve(listener, router(state))
                .await
                .expect("test server should run");
        });
        let server_url = format!("http://{address}");

        let mut first = RemoteLobbySession::connect(
            &server_url,
            &lobby,
            "First#1000".to_owned(),
            Some(TEST_JOIN_TOKEN),
        )
        .await
        .expect("first session should join");
        let mut second = RemoteLobbySession::connect(
            &server_url,
            &lobby,
            "Second#2000".to_owned(),
            Some(TEST_JOIN_TOKEN),
        )
        .await
        .expect("second session should join");

        assert_eq!(first.assigned_player_id(), 1);
        assert_eq!(second.assigned_player_id(), 2);
        assert_eq!(second.initial_roster().players.len(), 2);
        let synchronized = receive_roster_with_count(&mut first, 2).await;
        assert_eq!(synchronized, *second.initial_roster());

        second
            .send_chat("hello first player".to_owned())
            .await
            .expect("chat should send");
        assert_eq!(
            receive_chat(&mut first).await,
            (2, "hello first player".to_owned())
        );

        first.mark_ready().await.expect("first should become ready");
        second
            .mark_ready()
            .await
            .expect("second should become ready");
        first
            .send_chat("!start".to_owned())
            .await
            .expect("manual start command should send");
        assert_eq!(receive_countdown(&mut first).await, 60);
        assert_eq!(receive_countdown(&mut second).await, 60);

        drop(second);
        let after_leave = receive_roster_with_count(&mut first, 1).await;
        assert_eq!(after_leave.players[0].player_id, 1);
        server_task.abort();
    }

    async fn receive_roster_with_count(
        session: &mut RemoteLobbySession,
        expected_count: usize,
    ) -> LobbyRoster {
        timeout(Duration::from_secs(2), async {
            loop {
                let event = session
                    .next_event()
                    .await
                    .expect("lobby event should be valid")
                    .expect("session should remain connected");
                if let RemoteLobbyEvent::Roster(roster) = event
                    && roster.players.len() == expected_count
                {
                    return roster;
                }
            }
        })
        .await
        .expect("expected roster should arrive")
    }

    async fn receive_chat(session: &mut RemoteLobbySession) -> (u8, String) {
        timeout(Duration::from_secs(2), async {
            loop {
                let event = session
                    .next_event()
                    .await
                    .expect("lobby event should be valid")
                    .expect("session should remain connected");
                if let RemoteLobbyEvent::Chat {
                    from_player_id,
                    message,
                } = event
                {
                    return (from_player_id, message);
                }
            }
        })
        .await
        .expect("expected chat should arrive")
    }

    async fn receive_countdown(session: &mut RemoteLobbySession) -> u8 {
        timeout(Duration::from_secs(2), async {
            loop {
                let event = session
                    .next_event()
                    .await
                    .expect("lobby event should be valid")
                    .expect("session should remain connected");
                if let RemoteLobbyEvent::Countdown { remaining_seconds } = event {
                    return remaining_seconds;
                }
            }
        })
        .await
        .expect("expected countdown should arrive")
    }
}
