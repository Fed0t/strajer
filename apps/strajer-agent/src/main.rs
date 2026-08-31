mod local_map;
mod remote_lobby;

use std::env;
use std::io::{self, Write};
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use local_map::LocalMapMetadata;
use remote_lobby::RemoteLobbySession;
use reqwest::Client;
use serde::Serialize;
use strajer_lan::PublishedLobby;
use strajer_protocol::{LobbyCatalog, LobbyDescriptor, LobbyPlayer, LobbyRoster};
use strajer_w3gs::{
    CHAT_TO_HOST_PACKET_ID, Frame, FrameReader, LEAVE_REQUEST_PACKET_ID, MAP_SIZE_PACKET_ID,
    MapCheck, MapSize, PONG_TO_HOST_PACKET_ID, PROTOBUF_PACKET_ID, ProtobufEnvelope, RACE_HUMAN,
    RACE_NIGHT_ELF, RACE_UNDEAD, ReqJoin, SlotData, SlotInfo, SlotLayout, leave_ack,
    ping_from_host, player_info_frame, player_leave_others_frame, player_profile_frame,
    player_skins_frame,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::task::JoinHandle;
use tokio::time::{Instant, interval_at, timeout};
use tracing::{debug, info, warn};
use tracing_subscriber::EnvFilter;

const DEFAULT_SERVER_URL: &str = "http://127.0.0.1:18080";
const CATALOG_TIMEOUT: Duration = Duration::from_secs(5);
const JOIN_FRAME_TIMEOUT: Duration = Duration::from_secs(3);
const MAX_INITIAL_JOIN_FRAME_BYTES: usize = 4_096;
const MAX_LOBBY_FRAME_BYTES: usize = u16::MAX as usize;
const LOBBY_PING_INTERVAL: Duration = Duration::from_secs(15);
const DOTA_MAP_PATH: &str = "Maps\\Download\\DotA_v6_89Q.w3x";
const DOTA_PLAYER_SLOTS: u8 = 11;
const MINIMUM_JOIN_TOKEN_BYTES: usize = 32;
const MAXIMUM_JOIN_TOKEN_BYTES: usize = 128;
const DOTA_SLOT_TOPOLOGY: [(u8, u8, u8); DOTA_PLAYER_SLOTS as usize] = [
    (0, 1, RACE_NIGHT_ELF),
    (0, 2, RACE_NIGHT_ELF),
    (0, 3, RACE_NIGHT_ELF),
    (0, 4, RACE_NIGHT_ELF),
    (0, 5, RACE_NIGHT_ELF),
    (1, 7, RACE_UNDEAD),
    (1, 8, RACE_UNDEAD),
    (1, 9, RACE_UNDEAD),
    (1, 10, RACE_UNDEAD),
    (1, 11, RACE_UNDEAD),
    (2, 12, RACE_HUMAN),
];

#[derive(Debug, Serialize)]
struct AgentStatusEvent {
    event: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    lobby_count: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    lobby_id: Option<String>,
}

impl AgentStatusEvent {
    fn ready(lobby_count: usize) -> Self {
        Self {
            event: "ready",
            lobby_count: Some(lobby_count),
            lobby_id: None,
        }
    }

    fn join_request_captured(lobby_id: &str) -> Self {
        Self {
            event: "join_request_captured",
            lobby_count: None,
            lobby_id: Some(lobby_id.to_owned()),
        }
    }

    fn lobby_joined(lobby_id: &str) -> Self {
        Self {
            event: "lobby_joined",
            lobby_count: None,
            lobby_id: Some(lobby_id.to_owned()),
        }
    }
}

#[derive(Clone)]
struct LobbySessionConfig {
    server_url: String,
    join_token: Option<String>,
    lobby: LobbyDescriptor,
    local_map: LocalMapMetadata,
    map_check: MapCheck,
}

impl LobbySessionConfig {
    fn new(
        server_url: String,
        join_token: Option<String>,
        lobby: LobbyDescriptor,
        local_map: LocalMapMetadata,
    ) -> Result<Self> {
        let map_sha1 = lobby.map.sha1_bytes()?;
        let map_check = MapCheck::new(
            lobby.map.path.replace('\\', "/"),
            local_map.file_size(),
            local_map.crc32(),
            lobby.map.checksum,
            map_sha1,
        )?;

        Ok(Self {
            server_url,
            join_token,
            lobby,
            local_map,
            map_check,
        })
    }

    fn initial_frames(
        &self,
        assigned_player_id: u8,
        roster: &LobbyRoster,
        peer_address: SocketAddrV4,
    ) -> Result<Vec<Frame>> {
        let slot_info = build_dota_slot_info(&self.lobby, roster)?;
        let mut frames = vec![
            slot_info.join_frame(assigned_player_id, peer_address)?,
            slot_info.frame()?,
        ];

        for player in remote_players(roster, assigned_player_id) {
            frames.push(player_info(player)?);
        }
        for player in remote_players(roster, assigned_player_id) {
            frames.push(player_skins_frame(player.player_id)?);
        }
        for player in &roster.players {
            frames.push(player_profile_frame(player.player_id, player.name.clone())?);
        }
        frames.push(self.map_check.frame()?);
        Ok(frames)
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    initialize_tracing();

    let server_url = server_url()?;
    let join_token = join_token()?;
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
        let (publisher, listener_task) =
            activate_lobby(server_url.clone(), join_token.clone(), lobby).await?;
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
    server_url: String,
    join_token: Option<String>,
    lobby: LobbyDescriptor,
) -> Result<(PublishedLobby, JoinHandle<Result<()>>)> {
    let local_map = LocalMapMetadata::load(&lobby.map)
        .with_context(|| format!("could not prepare map for lobby {}", lobby.id))?;
    let session_config = Arc::new(LobbySessionConfig::new(
        server_url,
        join_token,
        lobby.clone(),
        local_map,
    )?);
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
        map_size = session_config.local_map.file_size(),
        map_crc32 = session_config.local_map.crc32(),
        "published Warcraft LAN lobby"
    );

    let listener_task = tokio::spawn(accept_connections(session_config, listener));
    Ok((publisher, listener_task))
}

async fn accept_connections(config: Arc<LobbySessionConfig>, listener: TcpListener) -> Result<()> {
    let frame_reader = FrameReader::new(MAX_INITIAL_JOIN_FRAME_BYTES)
        .context("invalid initial W3GS frame limit")?;

    loop {
        let (stream, peer_address) = listener
            .accept()
            .await
            .with_context(|| format!("listener failed for lobby {}", config.lobby.id))?;
        let connection_config = Arc::clone(&config);
        tokio::spawn(async move {
            let lobby_id = connection_config.lobby.id.clone();
            if let Err(error) =
                serve_join_connection(connection_config, frame_reader, stream, peer_address).await
            {
                warn!(%lobby_id, %error, "W3GS lobby connection ended with an error");
            }
        });
    }
}

async fn serve_join_connection(
    config: Arc<LobbySessionConfig>,
    frame_reader: FrameReader,
    mut stream: TcpStream,
    peer_address: SocketAddr,
) -> Result<()> {
    let frame = timeout(JOIN_FRAME_TIMEOUT, frame_reader.read_next(&mut stream))
        .await
        .context("Warcraft W3GS join frame timed out")??
        .context("Warcraft closed the connection before W3GS_REQJOIN")?;
    let request = ReqJoin::decode(&frame).context("rejected initial Warcraft W3GS frame")?;
    validate_join_request(&config.lobby, &request)?;
    let peer_address = ipv4_peer_address(peer_address)?;
    let lobby_id = config.lobby.id.as_str();

    info!(
        %lobby_id,
        frame_length = frame.encoded_length(),
        host_counter = request.host_counter(),
        listen_port = request.listen_port(),
        join_counter = request.join_counter(),
        player_name_bytes = request.player_name_bytes().len(),
        tail_bytes = request.tail().len(),
        "accepted valid W3GS_REQJOIN"
    );
    emit_status_event(&AgentStatusEvent::join_request_captured(lobby_id))?;

    let player_name = decode_player_name(&request)?;
    let remote_session = RemoteLobbySession::connect(
        &config.server_url,
        &config.lobby,
        player_name,
        config.join_token.as_deref(),
    )
    .await?;
    let assigned_player_id = remote_session.assigned_player_id();
    let initial_roster = remote_session.initial_roster().clone();
    let initial_frames =
        config.initial_frames(assigned_player_id, &initial_roster, peer_address)?;
    write_frames(&mut stream, &initial_frames).await?;
    info!(
        %lobby_id,
        assigned_player_id,
        player_count = initial_roster.players.len(),
        "sent coordinated W3GS lobby handshake"
    );

    run_lobby_session(
        config,
        stream,
        remote_session,
        assigned_player_id,
        initial_roster,
    )
    .await
}

fn decode_player_name(request: &ReqJoin) -> Result<String> {
    String::from_utf8(request.player_name_bytes().to_vec())
        .context("Warcraft player name is not valid UTF-8")
}

fn validate_join_request(lobby: &LobbyDescriptor, request: &ReqJoin) -> Result<()> {
    if request.host_counter() != lobby.lan_game_id {
        bail!(
            "W3GS_REQJOIN host counter {} does not match lobby {}",
            request.host_counter(),
            lobby.lan_game_id
        );
    }

    if request.entry_key() != lobby.game_secret {
        bail!("W3GS_REQJOIN entry key does not match lobby");
    }

    Ok(())
}

fn ipv4_peer_address(address: SocketAddr) -> Result<SocketAddrV4> {
    match address {
        SocketAddr::V4(address) => Ok(address),
        SocketAddr::V6(_) => bail!("Warcraft W3GS IPv6 connections are not supported"),
    }
}

async fn write_frames(stream: &mut TcpStream, frames: &[Frame]) -> Result<()> {
    for frame in frames {
        write_frame(stream, frame).await?;
    }

    Ok(())
}

async fn write_frame(stream: &mut TcpStream, frame: &Frame) -> Result<()> {
    stream
        .write_all(&frame.to_bytes())
        .await
        .with_context(|| format!("could not send W3GS packet 0x{:02X}", frame.packet_id()))
}

async fn run_lobby_session(
    config: Arc<LobbySessionConfig>,
    mut stream: TcpStream,
    mut remote_session: RemoteLobbySession,
    assigned_player_id: u8,
    mut roster: LobbyRoster,
) -> Result<()> {
    let frame_reader =
        FrameReader::new(MAX_LOBBY_FRAME_BYTES).context("invalid lobby W3GS frame limit")?;
    let session_started_at = Instant::now();
    let mut ping_interval = interval_at(
        session_started_at + LOBBY_PING_INTERVAL,
        LOBBY_PING_INTERVAL,
    );
    let mut map_verified = false;

    loop {
        tokio::select! {
            next_frame = frame_reader.read_next(&mut stream) => {
                let Some(frame) = next_frame.context("could not read W3GS lobby packet")? else {
                    info!(lobby_id = %config.lobby.id, "Warcraft left the local lobby");
                    return Ok(());
                };

                if handle_lobby_frame(&config, &mut stream, frame, &mut map_verified).await? {
                    return Ok(());
                }
            }
            _ = ping_interval.tick() => {
                let elapsed_millis = u32::try_from(session_started_at.elapsed().as_millis())
                    .unwrap_or(u32::MAX);
                let frame = ping_from_host(elapsed_millis)?;
                write_frame(&mut stream, &frame).await?;
            }
            roster_update = remote_session.next_roster() => {
                let Some(next_roster) = roster_update? else {
                    bail!("coordinated lobby connection closed");
                };
                if next_roster.revision <= roster.revision {
                    continue;
                }
                if next_roster.player(assigned_player_id).is_none() {
                    bail!("coordinated lobby roster removed the local player");
                }

                apply_roster_update(
                    &config.lobby,
                    &mut stream,
                    assigned_player_id,
                    &roster,
                    &next_roster,
                )
                .await?;
                info!(
                    lobby_id = %config.lobby.id,
                    roster_revision = next_roster.revision,
                    player_count = next_roster.players.len(),
                    "synchronized W3GS lobby roster"
                );
                roster = next_roster;
            }
        }
    }
}

async fn apply_roster_update(
    lobby: &LobbyDescriptor,
    stream: &mut TcpStream,
    assigned_player_id: u8,
    previous: &LobbyRoster,
    next: &LobbyRoster,
) -> Result<()> {
    for player in remote_players(previous, assigned_player_id) {
        if next.player(player.player_id) != Some(player) {
            write_frame(stream, &player_leave_others_frame(player.player_id)?).await?;
        }
    }

    for player in remote_players(next, assigned_player_id) {
        if previous.player(player.player_id) != Some(player) {
            write_frame(stream, &player_info(player)?).await?;
            write_frame(stream, &player_skins_frame(player.player_id)?).await?;
            write_frame(
                stream,
                &player_profile_frame(player.player_id, player.name.clone())?,
            )
            .await?;
        }
    }

    let slot_info = build_dota_slot_info(lobby, next)?;
    write_frame(stream, &slot_info.frame()?).await
}

fn remote_players(
    roster: &LobbyRoster,
    assigned_player_id: u8,
) -> impl Iterator<Item = &LobbyPlayer> {
    roster
        .players
        .iter()
        .filter(move |player| player.player_id != assigned_player_id)
}

fn player_info(player: &LobbyPlayer) -> Result<Frame> {
    let address = SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 0);
    Ok(player_info_frame(
        player.player_id,
        &player.name,
        address,
        address,
    )?)
}

async fn handle_lobby_frame(
    config: &LobbySessionConfig,
    stream: &mut TcpStream,
    frame: Frame,
    map_verified: &mut bool,
) -> Result<bool> {
    match frame.packet_id() {
        MAP_SIZE_PACKET_ID => {
            handle_map_size(config, &frame, map_verified)?;
        }
        PROTOBUF_PACKET_ID => {
            let envelope = ProtobufEnvelope::decode(&frame)?;
            if envelope.should_echo_in_lobby() {
                write_frame(stream, &frame).await?;
            } else {
                warn!(
                    lobby_id = %config.lobby.id,
                    message_type = format_args!("0x{:02X}", envelope.message_type()),
                    "ignored unsupported W3GS protobuf lobby message"
                );
            }
        }
        LEAVE_REQUEST_PACKET_ID => {
            let frame = leave_ack()?;
            write_frame(stream, &frame).await?;
            info!(lobby_id = %config.lobby.id, "Warcraft requested to leave the lobby");
            return Ok(true);
        }
        PONG_TO_HOST_PACKET_ID => {
            debug!(lobby_id = %config.lobby.id, "received W3GS lobby pong");
        }
        CHAT_TO_HOST_PACKET_ID => {
            warn!(
                lobby_id = %config.lobby.id,
                "ignored lobby chat or slot change during local join proof"
            );
        }
        packet_id => {
            debug!(
                lobby_id = %config.lobby.id,
                packet_id = format_args!("0x{packet_id:02X}"),
                frame_length = frame.encoded_length(),
                "ignored W3GS lobby packet"
            );
        }
    }

    Ok(false)
}

fn handle_map_size(
    config: &LobbySessionConfig,
    frame: &Frame,
    map_verified: &mut bool,
) -> Result<()> {
    let map_size = MapSize::decode(frame)?;
    if !map_size.has_map() {
        bail!("Warcraft reported that the required map is missing");
    }

    if map_size.map_size() != config.local_map.file_size() {
        bail!(
            "Warcraft reported map size {}, expected {}",
            map_size.map_size(),
            config.local_map.file_size()
        );
    }

    if !*map_verified {
        *map_verified = true;
        info!(
            lobby_id = %config.lobby.id,
            map_size = map_size.map_size(),
            "Warcraft entered the local lobby with the verified map"
        );
        emit_status_event(&AgentStatusEvent::lobby_joined(&config.lobby.id))?;
    }

    Ok(())
}

fn build_dota_slot_info(lobby: &LobbyDescriptor, roster: &LobbyRoster) -> Result<SlotInfo> {
    if lobby.map.path != DOTA_MAP_PATH {
        bail!("no W3GS slot topology is configured for {}", lobby.map.path);
    }

    if lobby.players.max != DOTA_PLAYER_SLOTS {
        bail!(
            "DotA coordinated lobby requires {DOTA_PLAYER_SLOTS} slots, got {}",
            lobby.players.max,
        );
    }
    roster
        .validate(DOTA_PLAYER_SLOTS)
        .context("cannot build W3GS slots from invalid roster")?;

    let mut slots = Vec::with_capacity(DOTA_SLOT_TOPOLOGY.len());
    for (index, &(team, color, race)) in DOTA_SLOT_TOPOLOGY.iter().enumerate() {
        if let Some(player) = roster
            .players
            .iter()
            .find(|player| usize::from(player.slot_index) == index)
        {
            slots.push(SlotData::occupied_human(
                player.player_id,
                team,
                color,
                race,
            ));
        } else {
            slots.push(SlotData::open(team, color, race));
        }
    }

    Ok(SlotInfo::new(
        slots,
        lobby_random_seed(lobby),
        SlotLayout::CustomForcesFixedPlayerSettings,
        DOTA_PLAYER_SLOTS,
    )?)
}

fn lobby_random_seed(lobby: &LobbyDescriptor) -> u32 {
    lobby.game_secret ^ lobby.lan_game_id ^ lobby.created_at_unix_seconds as u32
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

fn join_token() -> Result<Option<String>> {
    match env::var("STRAJER_JOIN_TOKEN") {
        Ok(value) if is_valid_join_token(&value) => Ok(Some(value)),
        Ok(_) => bail!(
            "STRAJER_JOIN_TOKEN must contain {MINIMUM_JOIN_TOKEN_BYTES} to {MAXIMUM_JOIN_TOKEN_BYTES} ASCII letters, digits, underscores or hyphens"
        ),
        Err(env::VarError::NotPresent) => Ok(None),
        Err(error) => Err(error).context("could not read STRAJER_JOIN_TOKEN"),
    }
}

fn is_valid_join_token(value: &str) -> bool {
    (MINIMUM_JOIN_TOKEN_BYTES..=MAXIMUM_JOIN_TOKEN_BYTES).contains(&value.len())
        && value.bytes().all(is_join_token_byte)
}

fn is_join_token_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-')
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

    #[test]
    fn writes_a_non_sensitive_join_capture_status() {
        let mut output = Vec::new();
        write_status_event(
            &mut output,
            &AgentStatusEvent::join_request_captured("synthetic-1"),
        )
        .expect("status should serialize");

        assert_eq!(
            String::from_utf8(output).expect("status should be UTF-8"),
            "{\"event\":\"join_request_captured\",\"lobby_id\":\"synthetic-1\"}\n"
        );
    }
}
