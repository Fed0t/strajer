mod local_map;
mod remote_lobby;

use std::env;
use std::io::{self, Write};
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use local_map::MapCache;
use remote_lobby::RemoteLobbySession;
use reqwest::Client;
use serde::Serialize;
use strajer_lan::PublishedLobby;
use strajer_protocol::{LobbyCatalog, LobbyDescriptor, LobbyPlayer, LobbyRoster};
use strajer_w3gs::{
    CHAT_TO_HOST_PACKET_ID, Frame, FrameReader, LEAVE_REQUEST_PACKET_ID, MAP_PART_DATA_BYTES,
    MAP_PART_NOT_OK_PACKET_ID, MAP_PART_OK_PACKET_ID, MAP_SIZE_PACKET_ID, MapCheck, MapPartAck,
    MapSize, PONG_TO_HOST_PACKET_ID, PROTOBUF_PACKET_ID, ProtobufEnvelope, RACE_HUMAN,
    RACE_NIGHT_ELF, RACE_UNDEAD, ReqJoin, SlotData, SlotInfo, SlotLayout, leave_ack,
    map_part_frame, ping_from_host, player_info_frame, player_leave_others_frame,
    player_profile_frame, player_skins_frame, start_download_frame,
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
const MAP_TRANSFER_INTERVAL: Duration = Duration::from_millis(100);
const MAP_TRANSFER_WINDOW_PARTS: u32 = 100;
const MAP_PREPARATION_TIMEOUT: Duration = Duration::from_secs(5 * 60);
const MAP_TRANSFER_STALL_TIMEOUT: Duration = Duration::from_secs(60);
const MAP_TRANSFER_TOTAL_TIMEOUT: Duration = Duration::from_secs(5 * 60);
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
    map_cache: MapCache,
    map_check: MapCheck,
}

impl LobbySessionConfig {
    fn new(server_url: String, join_token: Option<String>, lobby: LobbyDescriptor) -> Result<Self> {
        let map_sha1 = lobby.map.sha1_bytes()?;
        let map_check = MapCheck::new(
            lobby.map.path.replace('\\', "/"),
            lobby.map.file_size,
            lobby.map.file_crc32,
            lobby.map.checksum,
            map_sha1,
        )?;
        let map_cache = MapCache::new(&server_url, join_token.clone(), lobby.map.clone())?;

        Ok(Self {
            server_url,
            join_token,
            lobby,
            map_cache,
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

enum MapTransferState {
    AwaitingStatus,
    Preparing(PreparingMapTransfer),
    Downloading(ActiveMapTransfer),
    Verified,
}

struct PreparingMapTransfer {
    task: JoinHandle<Result<Arc<[u8]>>>,
    from_player_id: u8,
    to_player_id: u8,
    started_at: Instant,
}

struct ActiveMapTransfer {
    data: Arc<[u8]>,
    from_player_id: u8,
    to_player_id: u8,
    next_offset: u32,
    acknowledged_offset: u32,
    started_at: Instant,
    last_progress_at: Instant,
    last_logged_percent: u8,
}

impl ActiveMapTransfer {
    fn new(
        data: Arc<[u8]>,
        from_player_id: u8,
        to_player_id: u8,
        expected_size: u32,
    ) -> Result<Self> {
        if data.len() != usize::try_from(expected_size).context("map size does not fit platform")? {
            bail!(
                "prepared map contains {} bytes, expected {expected_size}",
                data.len()
            );
        }

        let now = Instant::now();
        Ok(Self {
            data,
            from_player_id,
            to_player_id,
            next_offset: 0,
            acknowledged_offset: 0,
            started_at: now,
            last_progress_at: now,
            last_logged_percent: 0,
        })
    }

    fn acknowledge(&mut self, offset: u32) -> Result<bool> {
        let map_size = u32::try_from(self.data.len()).context("map data exceeds 4 GiB")?;
        if offset > map_size {
            bail!("Warcraft acknowledged map offset {offset}, but map size is {map_size}");
        }
        if offset <= self.acknowledged_offset {
            return Ok(false);
        }

        self.acknowledged_offset = offset;
        self.last_progress_at = Instant::now();
        Ok(true)
    }

    fn pending_frames(&mut self) -> Result<Vec<Frame>> {
        let part_size = u32::try_from(MAP_PART_DATA_BYTES).expect("map part size fits u32");
        let window_size = part_size
            .checked_mul(MAP_TRANSFER_WINDOW_PARTS)
            .expect("map transfer window fits u32");
        let window_end = self.acknowledged_offset.saturating_add(window_size);
        let map_size = u32::try_from(self.data.len()).context("map data exceeds 4 GiB")?;
        let mut frames = Vec::with_capacity(MAP_TRANSFER_WINDOW_PARTS as usize);

        while self.next_offset < map_size && self.next_offset < window_end {
            let start =
                usize::try_from(self.next_offset).context("map offset does not fit usize")?;
            let end = start
                .saturating_add(MAP_PART_DATA_BYTES)
                .min(self.data.len());
            frames.push(map_part_frame(
                self.from_player_id,
                self.to_player_id,
                self.next_offset,
                &self.data[start..end],
            )?);
            self.next_offset = u32::try_from(end).context("map offset exceeds 4 GiB")?;
        }

        Ok(frames)
    }

    fn progress_percent(&self) -> u8 {
        let map_size = self.data.len() as u64;
        if map_size == 0 {
            return 0;
        }
        ((u64::from(self.acknowledged_offset) * 100) / map_size) as u8
    }

    fn should_log_progress(&mut self) -> bool {
        let percent = self.progress_percent();
        if percent < self.last_logged_percent.saturating_add(10) && percent != 100 {
            return false;
        }

        self.last_logged_percent = percent;
        true
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
    let session_config = Arc::new(LobbySessionConfig::new(
        server_url,
        join_token,
        lobby.clone(),
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
        map_size = session_config.map_cache.file_size(),
        map_crc32 = session_config.map_cache.file_crc32(),
        "published Warcraft LAN lobby"
    );

    let prefetch_config = Arc::clone(&session_config);
    tokio::spawn(async move {
        if let Err(error) = prefetch_config.map_cache.load().await {
            warn!(
                lobby_id = %prefetch_config.lobby.id,
                %error,
                "map prefetch failed; the agent will retry if Warcraft requests the map"
            );
        }
    });

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
    let mut map_transfer_interval = interval_at(
        session_started_at + MAP_TRANSFER_INTERVAL,
        MAP_TRANSFER_INTERVAL,
    );
    let map_host_player_id = roster
        .players
        .iter()
        .map(|player| player.player_id)
        .min()
        .unwrap_or(assigned_player_id);
    let mut map_transfer = MapTransferState::AwaitingStatus;

    loop {
        tokio::select! {
            next_frame = frame_reader.read_next(&mut stream) => {
                let Some(frame) = next_frame.context("could not read W3GS lobby packet")? else {
                    info!(lobby_id = %config.lobby.id, "Warcraft left the local lobby");
                    return Ok(());
                };

                if handle_lobby_frame(
                    &config,
                    &mut stream,
                    frame,
                    &mut map_transfer,
                    map_host_player_id,
                    assigned_player_id,
                )
                .await?
                {
                    return Ok(());
                }
            }
            _ = ping_interval.tick() => {
                let elapsed_millis = u32::try_from(session_started_at.elapsed().as_millis())
                    .unwrap_or(u32::MAX);
                let frame = ping_from_host(elapsed_millis)?;
                write_frame(&mut stream, &frame).await?;
            }
            _ = map_transfer_interval.tick() => {
                advance_map_transfer(&config, &mut stream, &mut map_transfer).await?;
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
    map_transfer: &mut MapTransferState,
    map_host_player_id: u8,
    assigned_player_id: u8,
) -> Result<bool> {
    match frame.packet_id() {
        MAP_SIZE_PACKET_ID => {
            handle_map_size(
                config,
                stream,
                &frame,
                map_transfer,
                map_host_player_id,
                assigned_player_id,
            )
            .await?;
        }
        MAP_PART_OK_PACKET_ID => match MapPartAck::decode(&frame) {
            Ok(acknowledgement) => {
                debug!(
                    lobby_id = %config.lobby.id,
                    sender_player_id = acknowledgement.sender_player_id(),
                    receiver_player_id = acknowledgement.receiver_player_id(),
                    map_offset = acknowledgement.map_size(),
                    "received W3GS map part acknowledgement"
                );
            }
            Err(error) => {
                warn!(
                    lobby_id = %config.lobby.id,
                    frame_length = frame.encoded_length(),
                    %error,
                    "ignored an unrecognized W3GS map part acknowledgement"
                );
            }
        },
        MAP_PART_NOT_OK_PACKET_ID => {
            bail!("Warcraft rejected a W3GS map part");
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

async fn handle_map_size(
    config: &LobbySessionConfig,
    stream: &mut TcpStream,
    frame: &Frame,
    map_transfer: &mut MapTransferState,
    map_host_player_id: u8,
    assigned_player_id: u8,
) -> Result<()> {
    let map_size = MapSize::decode(frame)?;
    let expected_size = config.map_cache.file_size();
    if map_size.has_map() && map_size.map_size() == expected_size {
        if let MapTransferState::Preparing(preparing) = map_transfer {
            preparing.task.abort();
        }
        let was_verified = matches!(map_transfer, MapTransferState::Verified);
        *map_transfer = MapTransferState::Verified;
        if !was_verified {
            info!(
                lobby_id = %config.lobby.id,
                map_size = map_size.map_size(),
                "Warcraft entered the local lobby with the verified map"
            );
            emit_status_event(&AgentStatusEvent::lobby_joined(&config.lobby.id))?;
        }
        return Ok(());
    }

    if map_size.map_size() > expected_size {
        bail!(
            "Warcraft reported map offset {}, but expected map size is {expected_size}",
            map_size.map_size()
        );
    }
    if !map_size.has_map() && !map_size.continues_download() {
        bail!(
            "Warcraft reported unsupported W3GS map size flag {}",
            map_size.size_flag()
        );
    }

    match map_transfer {
        MapTransferState::AwaitingStatus => {
            let map_cache = config.map_cache.clone();
            let task = tokio::spawn(async move { map_cache.load().await });
            write_frame(stream, &start_download_frame(map_host_player_id)?).await?;
            *map_transfer = MapTransferState::Preparing(PreparingMapTransfer {
                task,
                from_player_id: map_host_player_id,
                to_player_id: assigned_player_id,
                started_at: Instant::now(),
            });
            info!(
                lobby_id = %config.lobby.id,
                reported_map_size = map_size.map_size(),
                "Warcraft requested the map; preparing verified transfer data"
            );
        }
        MapTransferState::Preparing(_) => {}
        MapTransferState::Downloading(transfer) => {
            if map_size.continues_download()
                && transfer.acknowledge(map_size.map_size())?
                && transfer.should_log_progress()
            {
                info!(
                    lobby_id = %config.lobby.id,
                    progress_percent = transfer.progress_percent(),
                    acknowledged_bytes = map_size.map_size(),
                    "Warcraft map transfer progressed"
                );
            }
        }
        MapTransferState::Verified => {
            bail!("Warcraft reported an inconsistent map state after verification");
        }
    }

    Ok(())
}

async fn advance_map_transfer(
    config: &LobbySessionConfig,
    stream: &mut TcpStream,
    state: &mut MapTransferState,
) -> Result<()> {
    let preparation_finished = match state {
        MapTransferState::Preparing(preparing) => {
            if preparing.started_at.elapsed() > MAP_PREPARATION_TIMEOUT {
                bail!("preparing the Warcraft map transfer timed out");
            }
            preparing.task.is_finished()
        }
        _ => false,
    };

    if preparation_finished {
        let previous = std::mem::replace(state, MapTransferState::AwaitingStatus);
        let MapTransferState::Preparing(preparing) = previous else {
            unreachable!("map transfer state changed while completing preparation");
        };
        let data = preparing
            .task
            .await
            .context("map preparation task stopped unexpectedly")??;
        *state = MapTransferState::Downloading(ActiveMapTransfer::new(
            data,
            preparing.from_player_id,
            preparing.to_player_id,
            config.map_cache.file_size(),
        )?);
        info!(
            lobby_id = %config.lobby.id,
            map_size = config.map_cache.file_size(),
            "started local W3GS map transfer"
        );
    }

    let MapTransferState::Downloading(transfer) = state else {
        return Ok(());
    };
    if transfer.started_at.elapsed() > MAP_TRANSFER_TOTAL_TIMEOUT {
        bail!("Warcraft map transfer exceeded the maximum duration");
    }
    if transfer.last_progress_at.elapsed() > MAP_TRANSFER_STALL_TIMEOUT {
        bail!("Warcraft map transfer stalled waiting for acknowledgement");
    }

    let frames = transfer.pending_frames()?;
    write_frames(stream, &frames).await?;
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
    use std::fs;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    use axum::Router;
    use axum::body::Body;
    use axum::routing::get;
    use strajer_protocol::{
        DEFAULT_WARCRAFT_PRODUCT, DEFAULT_WARCRAFT_VERSION, MapDescriptor, PlayerCount,
        WarcraftDescriptor,
    };

    use super::*;

    static MAP_FLOW_TEST_SEQUENCE: AtomicU64 = AtomicU64::new(1);

    #[tokio::test]
    async fn downloads_and_transfers_a_missing_map_without_a_warcraft_copy() {
        let http_listener = TcpListener::bind(("127.0.0.1", 0))
            .await
            .expect("map fixture server should bind");
        let http_address = http_listener
            .local_addr()
            .expect("map fixture address should be available");
        let map_sha1 = "f7c3bc1d808e04732adf679965ccc34ca7ae3441";
        let application = Router::new().route(
            "/v1/maps/f7c3bc1d808e04732adf679965ccc34ca7ae3441",
            get(test_map_asset),
        );
        let http_task = tokio::spawn(async move {
            axum::serve(http_listener, application)
                .await
                .expect("map fixture server should run");
        });

        let test_directory = map_flow_test_directory();
        let map = MapDescriptor {
            path: DOTA_MAP_PATH.to_owned(),
            file_size: 9,
            file_crc32: 0xCBF4_3926,
            sha1_hex: map_sha1.to_owned(),
            checksum: 448_311_427,
            width: 128,
            height: 128,
        };
        let map_cache = MapCache::for_test(
            map.clone(),
            test_directory.join("maps").join(format!("{map_sha1}.w3x")),
            &format!("http://{http_address}/v1/maps/{map_sha1}"),
        )
        .expect("test map cache should build");
        let map_check = MapCheck::new(
            map.path.replace('\\', "/"),
            map.file_size,
            map.file_crc32,
            map.checksum,
            map.sha1_bytes().expect("test SHA-1 should decode"),
        )
        .expect("test map check should build");
        let config = LobbySessionConfig {
            server_url: format!("http://{http_address}"),
            join_token: None,
            lobby: LobbyDescriptor {
                id: "synthetic-1".to_owned(),
                revision: 1,
                lan_game_id: 1,
                game_secret: 0x5354_524A,
                name: "Strajer Test #1".to_owned(),
                created_at_unix_seconds: 1,
                warcraft: WarcraftDescriptor {
                    version: DEFAULT_WARCRAFT_VERSION.to_owned(),
                    product: DEFAULT_WARCRAFT_PRODUCT.to_owned(),
                },
                map,
                players: PlayerCount {
                    current: 1,
                    max: DOTA_PLAYER_SLOTS,
                },
            },
            map_cache,
            map_check,
        };

        let tcp_listener = TcpListener::bind(("127.0.0.1", 0))
            .await
            .expect("test W3GS listener should bind");
        let tcp_address = tcp_listener
            .local_addr()
            .expect("test W3GS address should be available");
        let client_task = tokio::spawn(async move {
            TcpStream::connect(tcp_address)
                .await
                .expect("test W3GS client should connect")
        });
        let (mut server_stream, _) = tcp_listener
            .accept()
            .await
            .expect("test W3GS server should accept");
        let mut client_stream = client_task.await.expect("client task should finish");
        let mut transfer = MapTransferState::AwaitingStatus;

        handle_map_size(
            &config,
            &mut server_stream,
            &test_map_size_frame(1, 0),
            &mut transfer,
            1,
            2,
        )
        .await
        .expect("missing map should start a transfer");
        let reader = FrameReader::new(4_096).expect("test frame reader should build");
        let start = reader
            .read_next(&mut client_stream)
            .await
            .expect("start download should read")
            .expect("start download should exist");
        assert_eq!(start.packet_id(), strajer_w3gs::START_DOWNLOAD_PACKET_ID);

        let mut transfer_started = false;
        for _ in 0..200 {
            advance_map_transfer(&config, &mut server_stream, &mut transfer)
                .await
                .expect("map preparation should advance");
            if matches!(transfer, MapTransferState::Downloading(_)) {
                transfer_started = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        assert!(transfer_started, "map preparation should complete");

        let part = timeout(Duration::from_secs(1), reader.read_next(&mut client_stream))
            .await
            .expect("map part should arrive")
            .expect("map part should decode")
            .expect("map part should exist");
        assert_eq!(part.packet_id(), strajer_w3gs::MAP_PART_PACKET_ID);
        assert_eq!(&part.payload()[14..], b"123456789");

        handle_map_size(
            &config,
            &mut server_stream,
            &test_map_size_frame(3, 9),
            &mut transfer,
            1,
            2,
        )
        .await
        .expect("map progress should apply");
        handle_map_size(
            &config,
            &mut server_stream,
            &test_map_size_frame(1, 9),
            &mut transfer,
            1,
            2,
        )
        .await
        .expect("map verification should finish");
        assert!(matches!(transfer, MapTransferState::Verified));
        assert_eq!(
            fs::read(test_directory.join("maps").join(format!("{map_sha1}.w3x")))
                .expect("cached map should be readable"),
            b"123456789"
        );

        http_task.abort();
        fs::remove_dir_all(test_directory).expect("test directory should be removed");
    }

    #[test]
    fn limits_map_parts_to_a_hundred_packet_sliding_window() {
        let map_size = MAP_PART_DATA_BYTES * 101;
        let data: Arc<[u8]> = Arc::from(vec![0xA5; map_size]);
        let mut transfer = ActiveMapTransfer::new(
            data,
            1,
            2,
            u32::try_from(map_size).expect("test map size should fit u32"),
        )
        .expect("map transfer should initialize");

        let first_window = transfer
            .pending_frames()
            .expect("first window should build");
        assert_eq!(first_window.len(), 100);
        assert!(
            transfer
                .pending_frames()
                .expect("blocked window should build")
                .is_empty()
        );

        transfer
            .acknowledge(u32::try_from(MAP_PART_DATA_BYTES).expect("part size should fit u32"))
            .expect("acknowledgement should apply");
        let second_window = transfer.pending_frames().expect("next window should build");
        assert_eq!(second_window.len(), 1);
    }

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

    async fn test_map_asset() -> Body {
        Body::from(&b"123456789"[..])
    }

    fn test_map_size_frame(size_flag: u8, map_size: u32) -> Frame {
        let mut payload = Vec::with_capacity(9);
        payload.extend_from_slice(&1_u32.to_le_bytes());
        payload.push(size_flag);
        payload.extend_from_slice(&map_size.to_le_bytes());
        Frame::new(MAP_SIZE_PACKET_ID, payload).expect("test map size frame should build")
    }

    fn map_flow_test_directory() -> PathBuf {
        let sequence = MAP_FLOW_TEST_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!(
            "strajer-map-flow-{}-{sequence}",
            std::process::id()
        ))
    }
}
