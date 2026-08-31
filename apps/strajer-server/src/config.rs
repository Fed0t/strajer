use std::env;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

const DEFAULT_PORT: u16 = 8_080;
const DEFAULT_MAP_PATH: &str = "maps/DotA_v6_89Q.w3x";
const MINIMUM_JOIN_TOKEN_BYTES: usize = 32;
const MAXIMUM_JOIN_TOKEN_BYTES: usize = 128;

#[derive(Clone, Eq, PartialEq)]
pub struct ServerConfig {
    pub bind_address: SocketAddr,
    join_token: Option<String>,
    map_path: PathBuf,
}

impl ServerConfig {
    pub fn from_environment() -> Result<Self> {
        let bind_address = match env::var("STRAJER_BIND_ADDR") {
            Ok(value) => value
                .parse::<SocketAddr>()
                .with_context(|| format!("invalid STRAJER_BIND_ADDR: {value}"))?,
            Err(env::VarError::NotPresent) => {
                SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), DEFAULT_PORT)
            }
            Err(error) => return Err(error).context("could not read STRAJER_BIND_ADDR"),
        };

        let join_token = read_join_token()?;
        if !bind_address.ip().is_loopback() && join_token.is_none() {
            bail!("STRAJER_JOIN_TOKEN is required when STRAJER_BIND_ADDR is not loopback");
        }

        let map_path = read_map_path()?;

        Ok(Self {
            bind_address,
            join_token,
            map_path,
        })
    }

    pub fn join_token(&self) -> Option<&str> {
        self.join_token.as_deref()
    }

    pub fn map_path(&self) -> &Path {
        &self.map_path
    }
}

fn read_map_path() -> Result<PathBuf> {
    match env::var_os("STRAJER_MAP_PATH") {
        Some(value) if !value.is_empty() => Ok(PathBuf::from(value)),
        Some(_) => bail!("STRAJER_MAP_PATH must not be empty"),
        None => Ok(PathBuf::from(DEFAULT_MAP_PATH)),
    }
}

fn read_join_token() -> Result<Option<String>> {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_join_token_format() {
        assert!(is_valid_join_token(
            "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
        ));
        assert!(is_valid_join_token("abcdefghijklmnopqrstuvwxyz_ABCDE-"));
        assert!(!is_valid_join_token("short"));
        assert!(!is_valid_join_token("0123456789abcdef0123456789abcdef!"));
    }
}
