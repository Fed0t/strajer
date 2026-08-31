use std::collections::HashSet;

use serde::{Deserialize, Serialize};
use thiserror::Error;

pub const CATALOG_SCHEMA_VERSION: u16 = 3;
pub const DEFAULT_WARCRAFT_PRODUCT: &str = "W3XP";
pub const DEFAULT_WARCRAFT_VERSION: &str = "2.0.4.23745";
pub const LOBBY_SESSION_PROTOCOL_VERSION: u16 = 4;
pub const LOBBY_COUNTDOWN_SECONDS: u8 = 60;
pub const LOBBY_COUNTDOWN_STEP_SECONDS: u8 = 10;
pub const MAX_LOBBY_CONTROL_MESSAGE_BYTES: usize = 4_096;
pub const MAX_LOBBY_CHAT_MESSAGE_BYTES: usize = 254;
pub const MAX_LOBBY_PLAYER_NAME_BYTES: usize = 15;
pub const MAX_GAME_NAME_BYTES: usize = 31;
pub const MAX_WARCRAFT_PLAYERS: u8 = 24;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AgentLobbyMessage {
    Join {
        protocol_version: u16,
        player_name: String,
    },
    Ready {
        protocol_version: u16,
    },
    Loaded {
        protocol_version: u16,
    },
    Chat {
        protocol_version: u16,
        message: String,
    },
}

impl AgentLobbyMessage {
    pub fn join(player_name: String) -> Result<Self, LobbySessionValidationError> {
        validate_lobby_player_name(&player_name)?;
        Ok(Self::Join {
            protocol_version: LOBBY_SESSION_PROTOCOL_VERSION,
            player_name,
        })
    }

    pub fn ready() -> Self {
        Self::Ready {
            protocol_version: LOBBY_SESSION_PROTOCOL_VERSION,
        }
    }

    pub fn loaded() -> Self {
        Self::Loaded {
            protocol_version: LOBBY_SESSION_PROTOCOL_VERSION,
        }
    }

    pub fn chat(message: String) -> Result<Self, LobbySessionValidationError> {
        validate_lobby_chat_message(&message)?;
        Ok(Self::Chat {
            protocol_version: LOBBY_SESSION_PROTOCOL_VERSION,
            message,
        })
    }

    pub fn validate(&self) -> Result<(), LobbySessionValidationError> {
        match self {
            Self::Join {
                protocol_version,
                player_name,
            } => {
                validate_lobby_session_protocol_version(*protocol_version)?;
                validate_lobby_player_name(player_name)
            }
            Self::Ready { protocol_version } | Self::Loaded { protocol_version } => {
                validate_lobby_session_protocol_version(*protocol_version)
            }
            Self::Chat {
                protocol_version,
                message,
            } => {
                validate_lobby_session_protocol_version(*protocol_version)?;
                validate_lobby_chat_message(message)
            }
        }
    }

    pub fn join_player_name(&self) -> Option<&str> {
        match self {
            Self::Join { player_name, .. } => Some(player_name),
            Self::Ready { .. } | Self::Loaded { .. } | Self::Chat { .. } => None,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerLobbyMessage {
    Joined {
        protocol_version: u16,
        player_id: u8,
        roster: LobbyRoster,
    },
    Roster {
        roster: LobbyRoster,
    },
    Countdown {
        remaining_seconds: u8,
    },
    CountdownCancelled,
    Chat {
        from_player_id: u8,
        message: String,
    },
    Notice {
        message: String,
    },
    Start,
    PlayerLoaded {
        player_id: u8,
    },
    Rejected {
        code: LobbyJoinRejection,
    },
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LobbyJoinRejection {
    InvalidRequest,
    UnsupportedProtocol,
    InvalidPlayerName,
    LobbyFull,
    LobbyStarted,
    DuplicatePlayerName,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct LobbyRoster {
    pub revision: u64,
    pub players: Vec<LobbyPlayer>,
}

impl LobbyRoster {
    pub fn validate(&self, maximum_players: u8) -> Result<(), LobbySessionValidationError> {
        if self.revision == 0 {
            return Err(LobbySessionValidationError::InvalidRosterRevision);
        }

        if maximum_players == 0
            || maximum_players > MAX_WARCRAFT_PLAYERS
            || self.players.len() > usize::from(maximum_players)
        {
            return Err(LobbySessionValidationError::InvalidRosterPlayerCount {
                actual: self.players.len(),
                maximum: maximum_players,
            });
        }

        let mut player_ids = HashSet::with_capacity(self.players.len());
        let mut slot_indices = HashSet::with_capacity(self.players.len());
        for player in &self.players {
            player.validate(maximum_players)?;
            if !player_ids.insert(player.player_id) {
                return Err(LobbySessionValidationError::DuplicateRosterPlayerId(
                    player.player_id,
                ));
            }
            if !slot_indices.insert(player.slot_index) {
                return Err(LobbySessionValidationError::DuplicateRosterSlotIndex(
                    player.slot_index,
                ));
            }
        }

        Ok(())
    }

    pub fn player(&self, player_id: u8) -> Option<&LobbyPlayer> {
        self.players
            .iter()
            .find(|player| player.player_id == player_id)
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct LobbyPlayer {
    pub player_id: u8,
    pub slot_index: u8,
    pub name: String,
}

impl LobbyPlayer {
    fn validate(&self, maximum_players: u8) -> Result<(), LobbySessionValidationError> {
        if self.player_id == 0 || self.player_id > maximum_players {
            return Err(LobbySessionValidationError::InvalidRosterPlayerId(
                self.player_id,
            ));
        }

        if self.slot_index >= maximum_players {
            return Err(LobbySessionValidationError::InvalidRosterSlotIndex(
                self.slot_index,
            ));
        }

        validate_lobby_player_name(&self.name)
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct LobbyCatalog {
    pub schema_version: u16,
    pub generated_at_unix_ms: u64,
    pub lobbies: Vec<LobbyDescriptor>,
}

impl LobbyCatalog {
    pub fn validate(&self) -> Result<(), ValidationError> {
        if self.schema_version != CATALOG_SCHEMA_VERSION {
            return Err(ValidationError::UnsupportedSchemaVersion {
                actual: self.schema_version,
                expected: CATALOG_SCHEMA_VERSION,
            });
        }

        let mut lobby_ids = HashSet::with_capacity(self.lobbies.len());
        let mut game_ids = HashSet::with_capacity(self.lobbies.len());

        for lobby in &self.lobbies {
            lobby.validate()?;

            if !lobby_ids.insert(lobby.id.as_str()) {
                return Err(ValidationError::DuplicateLobbyId(lobby.id.clone()));
            }

            if !game_ids.insert(lobby.lan_game_id) {
                return Err(ValidationError::DuplicateLanGameId(lobby.lan_game_id));
            }
        }

        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct LobbyDescriptor {
    pub id: String,
    pub revision: u64,
    pub lan_game_id: u32,
    pub game_secret: u32,
    pub name: String,
    pub created_at_unix_seconds: u64,
    pub warcraft: WarcraftDescriptor,
    pub map: MapDescriptor,
    pub players: PlayerCount,
    pub virtual_host: LobbyPlayer,
}

impl LobbyDescriptor {
    pub fn validate(&self) -> Result<(), ValidationError> {
        validate_lobby_id(&self.id)?;
        validate_game_name(&self.name)?;

        if self.lan_game_id == 0 || self.lan_game_id > i32::MAX as u32 {
            return Err(ValidationError::InvalidLanGameId(self.lan_game_id));
        }

        if self.revision == 0 || self.revision > i32::MAX as u64 {
            return Err(ValidationError::InvalidRevision(self.revision));
        }

        self.warcraft.validate()?;
        self.map.validate()?;
        self.players.validate()?;

        if self.players.current == 0
            || self.players.max < 2
            || self.virtual_host.player_id != self.players.max
            || self.virtual_host.slot_index != self.players.max - 1
            || validate_lobby_player_name(&self.virtual_host.name).is_err()
        {
            return Err(ValidationError::InvalidVirtualHost);
        }

        Ok(())
    }

    pub fn human_player_capacity(&self) -> u8 {
        self.players.max.saturating_sub(1)
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct WarcraftDescriptor {
    pub version: String,
    pub product: String,
}

impl WarcraftDescriptor {
    pub fn validate(&self) -> Result<(), ValidationError> {
        if self.product != DEFAULT_WARCRAFT_PRODUCT {
            return Err(ValidationError::UnsupportedWarcraftProduct(
                self.product.clone(),
            ));
        }

        validate_warcraft_version(&self.version)
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct MapDescriptor {
    pub path: String,
    pub file_size: u32,
    pub file_crc32: u32,
    pub sha1_hex: String,
    pub checksum: u32,
    pub width: u16,
    pub height: u16,
}

impl MapDescriptor {
    pub fn validate(&self) -> Result<(), ValidationError> {
        if self.path.is_empty() || self.path.contains('\0') {
            return Err(ValidationError::InvalidMapPath);
        }

        if self.file_size == 0 {
            return Err(ValidationError::InvalidMapFileSize);
        }

        self.sha1_bytes()?;
        Ok(())
    }

    pub fn sha1_bytes(&self) -> Result<[u8; 20], ValidationError> {
        let mut bytes = [0_u8; 20];
        hex::decode_to_slice(&self.sha1_hex, &mut bytes)
            .map_err(|_| ValidationError::InvalidMapSha1)?;
        Ok(bytes)
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct PlayerCount {
    pub current: u8,
    pub max: u8,
}

impl PlayerCount {
    pub fn validate(&self) -> Result<(), ValidationError> {
        if self.max == 0 || self.max > MAX_WARCRAFT_PLAYERS || self.current > self.max {
            return Err(ValidationError::InvalidPlayerCount {
                current: self.current,
                max: self.max,
            });
        }

        Ok(())
    }
}

#[derive(Debug, Error, Eq, PartialEq)]
pub enum ValidationError {
    #[error("unsupported catalog schema version {actual}; expected {expected}")]
    UnsupportedSchemaVersion { actual: u16, expected: u16 },
    #[error("lobby id must contain 1 to 64 printable ASCII bytes")]
    InvalidLobbyId,
    #[error("duplicate lobby id: {0}")]
    DuplicateLobbyId(String),
    #[error("LAN game id must be in the range 1..={}", i32::MAX)]
    InvalidLanGameId(u32),
    #[error("duplicate LAN game id: {0}")]
    DuplicateLanGameId(u32),
    #[error("lobby revision must be in the range 1..={}", i32::MAX)]
    InvalidRevision(u64),
    #[error("game name must contain 1 to {MAX_GAME_NAME_BYTES} UTF-8 bytes and no NUL byte")]
    InvalidGameName,
    #[error("unsupported Warcraft product: {0}")]
    UnsupportedWarcraftProduct(String),
    #[error("Warcraft version must contain exactly four numeric components")]
    InvalidWarcraftVersion,
    #[error("map path must not be empty or contain a NUL byte")]
    InvalidMapPath,
    #[error("map file size must not be zero")]
    InvalidMapFileSize,
    #[error("map SHA-1 must contain exactly 40 hexadecimal characters")]
    InvalidMapSha1,
    #[error("invalid player count: {current}/{max}")]
    InvalidPlayerCount { current: u8, max: u8 },
    #[error(
        "virtual host must occupy the final player id and slot in a lobby with at least two slots"
    )]
    InvalidVirtualHost,
}

#[derive(Debug, Error, Eq, PartialEq)]
pub enum LobbySessionValidationError {
    #[error("unsupported lobby session protocol {actual}; expected {expected}")]
    UnsupportedProtocolVersion { actual: u16, expected: u16 },
    #[error(
        "lobby player name must contain 1 to {MAX_LOBBY_PLAYER_NAME_BYTES} bytes and no NUL byte"
    )]
    InvalidPlayerName,
    #[error("lobby roster revision must not be zero")]
    InvalidRosterRevision,
    #[error("lobby roster contains {actual} players; maximum is {maximum}")]
    InvalidRosterPlayerCount { actual: usize, maximum: u8 },
    #[error("lobby roster player id {0} is outside the configured range")]
    InvalidRosterPlayerId(u8),
    #[error("lobby roster slot index {0} is outside the configured range")]
    InvalidRosterSlotIndex(u8),
    #[error("lobby roster contains duplicate player id {0}")]
    DuplicateRosterPlayerId(u8),
    #[error("lobby roster contains duplicate slot index {0}")]
    DuplicateRosterSlotIndex(u8),
    #[error("invalid lobby countdown value: {0} seconds")]
    InvalidCountdownSeconds(u8),
    #[error(
        "lobby chat message must contain 1 to {MAX_LOBBY_CHAT_MESSAGE_BYTES} UTF-8 bytes and no control characters"
    )]
    InvalidChatMessage,
}

pub fn validate_lobby_countdown_seconds(
    remaining_seconds: u8,
) -> Result<(), LobbySessionValidationError> {
    if remaining_seconds == 0
        || remaining_seconds > LOBBY_COUNTDOWN_SECONDS
        || !remaining_seconds.is_multiple_of(LOBBY_COUNTDOWN_STEP_SECONDS)
    {
        return Err(LobbySessionValidationError::InvalidCountdownSeconds(
            remaining_seconds,
        ));
    }

    Ok(())
}

pub fn validate_lobby_chat_message(message: &str) -> Result<(), LobbySessionValidationError> {
    if message.is_empty()
        || message.len() > MAX_LOBBY_CHAT_MESSAGE_BYTES
        || message.contains('\0')
        || message.chars().any(char::is_control)
    {
        return Err(LobbySessionValidationError::InvalidChatMessage);
    }

    Ok(())
}

fn validate_lobby_session_protocol_version(
    protocol_version: u16,
) -> Result<(), LobbySessionValidationError> {
    if protocol_version != LOBBY_SESSION_PROTOCOL_VERSION {
        return Err(LobbySessionValidationError::UnsupportedProtocolVersion {
            actual: protocol_version,
            expected: LOBBY_SESSION_PROTOCOL_VERSION,
        });
    }

    Ok(())
}

fn validate_lobby_player_name(name: &str) -> Result<(), LobbySessionValidationError> {
    if name.is_empty()
        || name.len() > MAX_LOBBY_PLAYER_NAME_BYTES
        || name.contains('\0')
        || name.chars().any(char::is_control)
    {
        return Err(LobbySessionValidationError::InvalidPlayerName);
    }

    Ok(())
}

fn validate_lobby_id(id: &str) -> Result<(), ValidationError> {
    if id.is_empty() || id.len() > 64 || !id.bytes().all(is_printable_ascii) {
        return Err(ValidationError::InvalidLobbyId);
    }

    Ok(())
}

fn is_printable_ascii(byte: u8) -> bool {
    byte.is_ascii_graphic()
}

fn validate_game_name(name: &str) -> Result<(), ValidationError> {
    if name.is_empty() || name.len() > MAX_GAME_NAME_BYTES || name.contains('\0') {
        return Err(ValidationError::InvalidGameName);
    }

    Ok(())
}

fn validate_warcraft_version(version: &str) -> Result<(), ValidationError> {
    let mut component_count = 0_u8;

    for component in version.split('.') {
        component_count += 1;
        if component.is_empty() || !component.bytes().all(is_ascii_digit) {
            return Err(ValidationError::InvalidWarcraftVersion);
        }
    }

    if component_count != 4 {
        return Err(ValidationError::InvalidWarcraftVersion);
    }

    Ok(())
}

fn is_ascii_digit(byte: u8) -> bool {
    byte.is_ascii_digit()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_a_catalog() {
        let catalog = valid_catalog();
        assert_eq!(catalog.validate(), Ok(()));
        assert_eq!(catalog.lobbies[0].map.sha1_bytes(), Ok([0_u8; 20]));
    }

    #[test]
    fn rejects_duplicate_ids() {
        let mut catalog = valid_catalog();
        catalog.lobbies.push(catalog.lobbies[0].clone());

        assert_eq!(
            catalog.validate(),
            Err(ValidationError::DuplicateLobbyId("synthetic-1".to_owned()))
        );
    }

    #[test]
    fn rejects_invalid_versions() {
        let mut catalog = valid_catalog();
        catalog.lobbies[0].warcraft.version = "2.0.4".to_owned();

        assert_eq!(
            catalog.validate(),
            Err(ValidationError::InvalidWarcraftVersion)
        );
    }

    #[test]
    fn rejects_a_zero_length_map_asset() {
        let mut catalog = valid_catalog();
        catalog.lobbies[0].map.file_size = 0;

        assert_eq!(catalog.validate(), Err(ValidationError::InvalidMapFileSize));
    }

    #[test]
    fn rejects_game_names_larger_than_the_wire_limit() {
        let mut catalog = valid_catalog();
        catalog.lobbies[0].name = "x".repeat(MAX_GAME_NAME_BYTES + 1);

        assert_eq!(catalog.validate(), Err(ValidationError::InvalidGameName));
    }

    #[test]
    fn validates_a_two_player_lobby_roster() {
        let roster = LobbyRoster {
            revision: 3,
            players: vec![
                LobbyPlayer {
                    player_id: 1,
                    slot_index: 0,
                    name: "Host#1234".to_owned(),
                },
                LobbyPlayer {
                    player_id: 2,
                    slot_index: 1,
                    name: "Friend#5678".to_owned(),
                },
            ],
        };

        assert_eq!(roster.validate(11), Ok(()));
        assert_eq!(
            roster.player(2).map(|player| player.name.as_str()),
            Some("Friend#5678")
        );
    }

    #[test]
    fn rejects_duplicate_roster_slots() {
        let roster = LobbyRoster {
            revision: 2,
            players: vec![
                LobbyPlayer {
                    player_id: 1,
                    slot_index: 0,
                    name: "One".to_owned(),
                },
                LobbyPlayer {
                    player_id: 2,
                    slot_index: 0,
                    name: "Two".to_owned(),
                },
            ],
        };

        assert_eq!(
            roster.validate(11),
            Err(LobbySessionValidationError::DuplicateRosterSlotIndex(0))
        );
    }

    #[test]
    fn validates_the_join_control_message() {
        let message =
            AgentLobbyMessage::join("Player#1234".to_owned()).expect("player name should be valid");

        assert_eq!(message.validate(), Ok(()));
        assert_eq!(message.join_player_name(), Some("Player#1234"));
        assert_eq!(
            serde_json::to_string(&message).expect("join should serialize"),
            r#"{"type":"join","protocol_version":4,"player_name":"Player#1234"}"#
        );
    }

    #[test]
    fn validates_ready_and_countdown_control_messages() {
        let ready = AgentLobbyMessage::ready();
        let loaded = AgentLobbyMessage::loaded();
        let chat = AgentLobbyMessage::chat("hello lobby".to_owned())
            .expect("chat message should be valid");

        assert_eq!(ready.validate(), Ok(()));
        assert_eq!(ready.join_player_name(), None);
        assert_eq!(loaded.validate(), Ok(()));
        assert_eq!(loaded.join_player_name(), None);
        assert_eq!(chat.validate(), Ok(()));
        assert_eq!(validate_lobby_countdown_seconds(60), Ok(()));
        assert_eq!(validate_lobby_countdown_seconds(10), Ok(()));
        assert_eq!(
            validate_lobby_countdown_seconds(55),
            Err(LobbySessionValidationError::InvalidCountdownSeconds(55))
        );
        assert_eq!(
            serde_json::to_string(&ready).expect("ready should serialize"),
            r#"{"type":"ready","protocol_version":4}"#
        );
        assert_eq!(
            serde_json::to_string(&loaded).expect("loaded should serialize"),
            r#"{"type":"loaded","protocol_version":4}"#
        );
        assert_eq!(
            serde_json::to_string(&chat).expect("chat should serialize"),
            r#"{"type":"chat","protocol_version":4,"message":"hello lobby"}"#
        );
        assert_eq!(
            serde_json::to_string(&ServerLobbyMessage::PlayerLoaded { player_id: 2 })
                .expect("player-loaded should serialize"),
            r#"{"type":"player_loaded","player_id":2}"#
        );
        assert!(matches!(
            AgentLobbyMessage::chat("bad\nchat".to_owned()),
            Err(LobbySessionValidationError::InvalidChatMessage)
        ));
    }

    fn valid_catalog() -> LobbyCatalog {
        LobbyCatalog {
            schema_version: CATALOG_SCHEMA_VERSION,
            generated_at_unix_ms: 1_000,
            lobbies: vec![LobbyDescriptor {
                id: "synthetic-1".to_owned(),
                revision: 1,
                lan_game_id: 1,
                game_secret: 42,
                name: "Strajer Test #1".to_owned(),
                created_at_unix_seconds: 1,
                warcraft: WarcraftDescriptor {
                    version: DEFAULT_WARCRAFT_VERSION.to_owned(),
                    product: DEFAULT_WARCRAFT_PRODUCT.to_owned(),
                },
                map: MapDescriptor {
                    path: "Maps\\Strajer\\Synthetic.w3x".to_owned(),
                    file_size: 1,
                    file_crc32: 0,
                    sha1_hex: "00".repeat(20),
                    checksum: u32::MAX,
                    width: 0,
                    height: 0,
                },
                players: PlayerCount {
                    current: 1,
                    max: MAX_WARCRAFT_PLAYERS,
                },
                virtual_host: LobbyPlayer {
                    player_id: MAX_WARCRAFT_PLAYERS,
                    slot_index: MAX_WARCRAFT_PLAYERS - 1,
                    name: "Strajer".to_owned(),
                },
            }],
        }
    }
}
