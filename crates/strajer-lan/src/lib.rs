mod game_info;
mod publisher;
mod service_type;

pub use game_info::encode_game_info_record;
pub use publisher::PublishedLobby;
pub use service_type::service_registration_type;

use thiserror::Error;

#[derive(Debug, Error)]
pub enum LanError {
    #[error("invalid lobby descriptor: {0}")]
    InvalidLobby(#[from] strajer_protocol::ValidationError),
    #[error("unsupported Warcraft version: {0}")]
    UnsupportedWarcraftVersion(String),
    #[error("Bonjour registration failed: {0}")]
    Bonjour(#[source] std::io::Error),
    #[error("LAN publishing is supported only on macOS")]
    UnsupportedPlatform,
}
