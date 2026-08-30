use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use strajer_protocol::{
    CATALOG_SCHEMA_VERSION, DEFAULT_WARCRAFT_PRODUCT, DEFAULT_WARCRAFT_VERSION, LobbyCatalog,
    LobbyDescriptor, MapDescriptor, PlayerCount, ValidationError, WarcraftDescriptor,
};

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
                    path: "Maps\\Strajer\\Synthetic.w3x".to_owned(),
                    sha1_hex: "00".repeat(20),
                    checksum: u32::MAX,
                    width: 0,
                    height: 0,
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
