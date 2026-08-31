use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use strajer_protocol::{
    CATALOG_SCHEMA_VERSION, DEFAULT_WARCRAFT_PRODUCT, DEFAULT_WARCRAFT_VERSION, LobbyCatalog,
    LobbyDescriptor, MapDescriptor, PlayerCount, ValidationError, WarcraftDescriptor,
};
use subtle::ConstantTimeEq;

use crate::lobby::{LobbyRegistry, LobbyRoom};

const DOTA_MAP_PATH: &str = "Maps\\Download\\DotA_v6_89Q.w3x";
const DOTA_MAP_SHA1_HEX: &str = "c771ac8d7dc3665a211c2b1432672d49bfba1bcf";
const DOTA_MAP_CHECKSUM: u32 = 448_311_427;
const DOTA_MAP_WIDTH: u16 = 128;
const DOTA_MAP_HEIGHT: u16 = 128;
const DOTA_PLAYER_SLOTS: u8 = 11;

#[derive(Clone)]
pub struct AppState {
    catalog: Arc<LobbyCatalog>,
    lobby_registry: LobbyRegistry,
    join_token: Arc<Option<String>>,
}

impl AppState {
    pub fn synthetic() -> Result<Self, ValidationError> {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default();
        Self::synthetic_at(now.as_millis() as u64, now.as_secs())
    }

    pub fn synthetic_at(
        generated_at_unix_ms: u64,
        created_at_unix_seconds: u64,
    ) -> Result<Self, ValidationError> {
        let catalog = LobbyCatalog {
            schema_version: CATALOG_SCHEMA_VERSION,
            generated_at_unix_ms,
            lobbies: vec![LobbyDescriptor {
                id: "synthetic-1".to_owned(),
                revision: 1,
                lan_game_id: 1,
                game_secret: 0x5354_524A,
                name: "Strajer Test #1".to_owned(),
                created_at_unix_seconds,
                warcraft: WarcraftDescriptor {
                    version: DEFAULT_WARCRAFT_VERSION.to_owned(),
                    product: DEFAULT_WARCRAFT_PRODUCT.to_owned(),
                },
                map: MapDescriptor {
                    path: DOTA_MAP_PATH.to_owned(),
                    sha1_hex: DOTA_MAP_SHA1_HEX.to_owned(),
                    checksum: DOTA_MAP_CHECKSUM,
                    width: DOTA_MAP_WIDTH,
                    height: DOTA_MAP_HEIGHT,
                },
                players: PlayerCount {
                    current: 1,
                    max: DOTA_PLAYER_SLOTS,
                },
            }],
        };
        catalog.validate()?;

        let lobby_registry = LobbyRegistry::from_catalog(&catalog);
        Ok(Self {
            catalog: Arc::new(catalog),
            lobby_registry,
            join_token: Arc::new(None),
        })
    }

    pub fn catalog(&self) -> &LobbyCatalog {
        &self.catalog
    }

    pub fn with_join_token(mut self, join_token: Option<String>) -> Self {
        self.join_token = Arc::new(join_token);
        self
    }

    pub(crate) fn authorizes_lobby_session(&self, authorization: Option<&str>) -> bool {
        let Some(expected_token) = self.join_token.as_deref() else {
            return true;
        };
        let Some(provided_token) = authorization.and_then(parse_bearer_token) else {
            return false;
        };

        provided_token
            .as_bytes()
            .ct_eq(expected_token.as_bytes())
            .into()
    }

    pub(crate) fn lobby_room(&self, lobby_id: &str) -> Option<Arc<LobbyRoom>> {
        self.lobby_registry.room(lobby_id)
    }
}

fn parse_bearer_token(authorization: &str) -> Option<&str> {
    authorization.strip_prefix("Bearer ")
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_JOIN_TOKEN: &str =
        "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    #[test]
    fn protects_lobby_sessions_with_the_configured_bearer_token() {
        let protected_state = AppState::synthetic_at(2_000, 2)
            .expect("state should be valid")
            .with_join_token(Some(TEST_JOIN_TOKEN.to_owned()));

        assert!(!protected_state.authorizes_lobby_session(None));
        assert!(!protected_state.authorizes_lobby_session(Some("Basic invalid")));
        assert!(!protected_state.authorizes_lobby_session(Some("Bearer invalid")));
        assert!(
            protected_state.authorizes_lobby_session(Some(&format!("Bearer {TEST_JOIN_TOKEN}")))
        );
    }

    #[test]
    fn permits_local_unprotected_lobby_sessions() {
        let local_state = AppState::synthetic_at(2_000, 2).expect("state should be valid");

        assert!(local_state.authorizes_lobby_session(None));
    }
}
