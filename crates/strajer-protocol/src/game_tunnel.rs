use thiserror::Error;

use crate::LOBBY_SESSION_PROTOCOL_VERSION;

const GAME_TUNNEL_MAGIC: [u8; 4] = *b"STRJ";
const GAME_TUNNEL_HEADER_BYTES: usize = 16;
const AGENT_W3GS_FRAME_KIND: u8 = 0x01;
const SERVER_W3GS_FRAME_KIND: u8 = 0x81;
const RESERVED_FLAGS: u8 = 0;

pub const MAX_TUNNELED_W3GS_FRAME_BYTES: usize = 1_460;
pub const MAX_GAME_TUNNEL_MESSAGE_BYTES: usize =
    GAME_TUNNEL_HEADER_BYTES + MAX_TUNNELED_W3GS_FRAME_BYTES;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AgentGameMessage {
    W3gsFrame { sequence: u64, frame: Vec<u8> },
}

impl AgentGameMessage {
    pub fn w3gs_frame(sequence: u64, frame: Vec<u8>) -> Result<Self, GameTunnelError> {
        validate_sequence(sequence)?;
        validate_w3gs_frame(&frame)?;
        Ok(Self::W3gsFrame { sequence, frame })
    }

    pub fn sequence(&self) -> u64 {
        match self {
            Self::W3gsFrame { sequence, .. } => *sequence,
        }
    }

    pub fn frame(&self) -> &[u8] {
        match self {
            Self::W3gsFrame { frame, .. } => frame,
        }
    }

    pub fn encode(&self) -> Vec<u8> {
        encode_message(AGENT_W3GS_FRAME_KIND, self.sequence(), self.frame())
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, GameTunnelError> {
        let decoded = decode_message(bytes, AGENT_W3GS_FRAME_KIND)?;
        Self::w3gs_frame(decoded.sequence, decoded.frame.to_vec())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ServerGameMessage {
    W3gsFrame { sequence: u64, frame: Vec<u8> },
}

impl ServerGameMessage {
    pub fn w3gs_frame(sequence: u64, frame: Vec<u8>) -> Result<Self, GameTunnelError> {
        validate_sequence(sequence)?;
        validate_w3gs_frame(&frame)?;
        Ok(Self::W3gsFrame { sequence, frame })
    }

    pub fn sequence(&self) -> u64 {
        match self {
            Self::W3gsFrame { sequence, .. } => *sequence,
        }
    }

    pub fn frame(&self) -> &[u8] {
        match self {
            Self::W3gsFrame { frame, .. } => frame,
        }
    }

    pub fn encode(&self) -> Vec<u8> {
        encode_message(SERVER_W3GS_FRAME_KIND, self.sequence(), self.frame())
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, GameTunnelError> {
        let decoded = decode_message(bytes, SERVER_W3GS_FRAME_KIND)?;
        Self::w3gs_frame(decoded.sequence, decoded.frame.to_vec())
    }
}

#[derive(Debug, Error, Eq, PartialEq)]
pub enum GameTunnelError {
    #[error("game tunnel message contains {actual} bytes; expected {minimum} to {maximum}")]
    InvalidMessageLength {
        actual: usize,
        minimum: usize,
        maximum: usize,
    },
    #[error("game tunnel message has an invalid magic value")]
    InvalidMagic,
    #[error("game tunnel message kind 0x{actual:02X} is invalid for this direction")]
    InvalidKind { actual: u8 },
    #[error("game tunnel flags must be zero, got 0x{actual:02X}")]
    InvalidFlags { actual: u8 },
    #[error("game tunnel protocol {actual} does not match {expected}")]
    UnsupportedProtocolVersion { actual: u16, expected: u16 },
    #[error("game tunnel sequence must be greater than zero")]
    InvalidSequence,
    #[error("tunneled W3GS frame contains {actual} bytes; expected 1 to {maximum}")]
    InvalidW3gsFrameLength { actual: usize, maximum: usize },
}

struct DecodedMessage<'a> {
    sequence: u64,
    frame: &'a [u8],
}

fn encode_message(kind: u8, sequence: u64, frame: &[u8]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(GAME_TUNNEL_HEADER_BYTES + frame.len());
    bytes.extend_from_slice(&GAME_TUNNEL_MAGIC);
    bytes.push(kind);
    bytes.push(RESERVED_FLAGS);
    bytes.extend_from_slice(&LOBBY_SESSION_PROTOCOL_VERSION.to_le_bytes());
    bytes.extend_from_slice(&sequence.to_le_bytes());
    bytes.extend_from_slice(frame);
    bytes
}

fn decode_message(bytes: &[u8], expected_kind: u8) -> Result<DecodedMessage<'_>, GameTunnelError> {
    if bytes.len() <= GAME_TUNNEL_HEADER_BYTES || bytes.len() > MAX_GAME_TUNNEL_MESSAGE_BYTES {
        return Err(GameTunnelError::InvalidMessageLength {
            actual: bytes.len(),
            minimum: GAME_TUNNEL_HEADER_BYTES + 1,
            maximum: MAX_GAME_TUNNEL_MESSAGE_BYTES,
        });
    }
    if bytes[..4] != GAME_TUNNEL_MAGIC {
        return Err(GameTunnelError::InvalidMagic);
    }
    if bytes[4] != expected_kind {
        return Err(GameTunnelError::InvalidKind { actual: bytes[4] });
    }
    if bytes[5] != RESERVED_FLAGS {
        return Err(GameTunnelError::InvalidFlags { actual: bytes[5] });
    }

    let protocol_version = u16::from_le_bytes([bytes[6], bytes[7]]);
    if protocol_version != LOBBY_SESSION_PROTOCOL_VERSION {
        return Err(GameTunnelError::UnsupportedProtocolVersion {
            actual: protocol_version,
            expected: LOBBY_SESSION_PROTOCOL_VERSION,
        });
    }

    let sequence = u64::from_le_bytes(
        bytes[8..16]
            .try_into()
            .expect("game tunnel header length was validated"),
    );
    validate_sequence(sequence)?;
    let frame = &bytes[GAME_TUNNEL_HEADER_BYTES..];
    validate_w3gs_frame(frame)?;
    Ok(DecodedMessage { sequence, frame })
}

fn validate_sequence(sequence: u64) -> Result<(), GameTunnelError> {
    if sequence == 0 {
        return Err(GameTunnelError::InvalidSequence);
    }
    Ok(())
}

fn validate_w3gs_frame(frame: &[u8]) -> Result<(), GameTunnelError> {
    if frame.is_empty() || frame.len() > MAX_TUNNELED_W3GS_FRAME_BYTES {
        return Err(GameTunnelError::InvalidW3gsFrameLength {
            actual: frame.len(),
            maximum: MAX_TUNNELED_W3GS_FRAME_BYTES,
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_directional_w3gs_messages() {
        let w3gs_frame = vec![0xF7, 0x27, 0x09, 0x00, 1, 2, 3, 4, 5];
        let agent =
            AgentGameMessage::w3gs_frame(7, w3gs_frame.clone()).expect("agent frame should build");
        let encoded_agent = agent.encode();

        assert_eq!(&encoded_agent[..4], b"STRJ");
        assert_eq!(encoded_agent[4], AGENT_W3GS_FRAME_KIND);
        assert_eq!(AgentGameMessage::decode(&encoded_agent), Ok(agent));

        let server =
            ServerGameMessage::w3gs_frame(9, w3gs_frame).expect("server frame should build");
        let encoded_server = server.encode();
        assert_eq!(encoded_server[4], SERVER_W3GS_FRAME_KIND);
        assert_eq!(ServerGameMessage::decode(&encoded_server), Ok(server));
    }

    #[test]
    fn rejects_wrong_direction_and_zero_sequence() {
        let frame = vec![0xF7, 0x0C, 0x06, 0x00, 100, 0];
        let server =
            ServerGameMessage::w3gs_frame(1, frame.clone()).expect("server frame should build");

        assert!(matches!(
            AgentGameMessage::decode(&server.encode()),
            Err(GameTunnelError::InvalidKind { .. })
        ));
        assert_eq!(
            AgentGameMessage::w3gs_frame(0, frame),
            Err(GameTunnelError::InvalidSequence)
        );
    }

    #[test]
    fn rejects_oversized_w3gs_frames() {
        assert!(matches!(
            ServerGameMessage::w3gs_frame(1, vec![0; MAX_TUNNELED_W3GS_FRAME_BYTES + 1]),
            Err(GameTunnelError::InvalidW3gsFrameLength { .. })
        ));
    }
}
