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
        use async_dnssd::{Type, register_extended};

        let registration_type = service_registration_type(&lobby.warcraft.version)?;
        let record_data = encode_game_info_record(lobby, local_port)?;
        let pending_registration = register_extended(
            &registration_type,
            local_port,
            local_registration_data(&lobby.name),
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

#[cfg(target_os = "macos")]
fn local_registration_data(service_name: &str) -> async_dnssd::RegisterData<'_> {
    use async_dnssd::{Interface, RegisterData, RegisterFlags};

    RegisterData {
        flags: RegisterFlags::NO_AUTO_RENAME | RegisterFlags::UNIQUE,
        interface: Interface::LocalOnly,
        name: Some(service_name),
        ..Default::default()
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

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use async_dnssd::{Interface, RegisterFlags};

    use super::*;

    #[test]
    fn restricts_the_lobby_advertisement_to_the_local_machine() {
        let registration = local_registration_data("Strajer Test #1");

        assert_eq!(registration.interface, Interface::LocalOnly);
        assert_eq!(registration.name, Some("Strajer Test #1"));
        assert!(registration.flags.contains(RegisterFlags::NO_AUTO_RENAME));
        assert!(registration.flags.contains(RegisterFlags::UNIQUE));
    }
}
