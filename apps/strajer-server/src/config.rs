use std::env;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};

use anyhow::{Context, Result};

const DEFAULT_PORT: u16 = 8_080;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ServerConfig {
    pub bind_address: SocketAddr,
}

impl ServerConfig {
    pub fn from_environment() -> Result<Self> {
        let bind_address = match env::var("STRAJER_BIND_ADDR") {
            Ok(value) => value
                .parse::<SocketAddr>()
                .with_context(|| format!("invalid STRAJER_BIND_ADDR: {value}"))?,
            Err(env::VarError::NotPresent) => {
                SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), DEFAULT_PORT)
            }
            Err(error) => return Err(error).context("could not read STRAJER_BIND_ADDR"),
        };

        Ok(Self { bind_address })
    }
}
