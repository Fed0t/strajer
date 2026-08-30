use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use strajer_protocol::{
    CATALOG_SCHEMA_VERSION, DEFAULT_WARCRAFT_PRODUCT, DEFAULT_WARCRAFT_VERSION, LobbyCatalog,
    LobbyDescriptor, MapDescriptor, PlayerCount, ValidationError, WarcraftDescriptor,
};

const DOTA_MAP_PATH: &str = "Maps\\Download\\DotA_v6_89Q.w3x";
const DOTA_MAP_SHA1_HEX: &str = "c771ac8d7dc3665a211c2b1432672d49bfba1bcf";
const DOTA_MAP_CHECKSUM: u32 = 448_311_427;
const DOTA_MAP_WIDTH: u16 = 128;
const DOTA_MAP_HEIGHT: u16 = 128;

#[derive(Clone)]
pub struct AppState {
    catalog: Arc<LobbyCatalog>,
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
                    max: 24,
                },
            }],
        };
        catalog.validate()?;

        Ok(Self {
            catalog: Arc::new(catalog),
        })
    }

    pub fn catalog(&self) -> &LobbyCatalog {
        &self.catalog
    }
}
