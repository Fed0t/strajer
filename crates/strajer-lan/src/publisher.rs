use strajer_protocol::LobbyDescriptor;

use crate::LanError;

#[cfg(target_os = "macos")]
use crate::{encode_game_info_record, service_registration_type};

#[cfg(target_os = "macos")]
const GAME_INFO_RECORD_TYPE: u16 = 66;
#[cfg(target_os = "macos")]
const GAME_INFO_TTL_SECONDS: u32 = 4_500;

#[cfg(target_os = "macos")]
pub struct PublishedLobby {
    _registration: async_dnssd::Registration,
    _record: async_dnssd::Record,
}

#[cfg(target_os = "macos")]
impl PublishedLobby {
    pub async fn publish(lobby: &LobbyDescriptor, local_port: u16) -> Result<Self, LanError> {
        use async_dnssd::{RegisterData, RegisterFlags, Type, register_extended};

        let registration_type = service_registration_type(&lobby.warcraft.version)?;
        let record_data = encode_game_info_record(lobby, local_port)?;
        let pending_registration = register_extended(
            &registration_type,
            local_port,
            RegisterData {
                flags: RegisterFlags::NO_AUTO_RENAME | RegisterFlags::UNIQUE,
                name: Some(&lobby.name),
                ..Default::default()
            },
        )
        .map_err(LanError::Bonjour)?;

        let record = pending_registration
            .add_record(
                Type(GAME_INFO_RECORD_TYPE),
                &record_data,
                GAME_INFO_TTL_SECONDS,
            )
            .map_err(LanError::Bonjour)?;
        let (registration, _) = pending_registration.await.map_err(LanError::Bonjour)?;

        Ok(Self {
            _registration: registration,
            _record: record,
        })
    }
}

#[cfg(not(target_os = "macos"))]
pub struct PublishedLobby;

#[cfg(not(target_os = "macos"))]
impl PublishedLobby {
    pub async fn publish(_lobby: &LobbyDescriptor, _local_port: u16) -> Result<Self, LanError> {
        Err(LanError::UnsupportedPlatform)
    }
}
