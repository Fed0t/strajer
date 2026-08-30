use thiserror::Error;

use crate::Frame;

pub const REQ_JOIN_PACKET_ID: u8 = 0x1E;
pub const MAX_PLAYER_NAME_BYTES: usize = 255;
const FIXED_PREFIX_LENGTH: usize = 15;
const MINIMUM_PAYLOAD_LENGTH: usize = FIXED_PREFIX_LENGTH + 1;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReqJoin {
    host_counter: u32,
    entry_key: u32,
    unknown: u8,
    listen_port: u16,
    join_counter: u32,
    player_name: Vec<u8>,
    tail: Vec<u8>,
}

impl ReqJoin {
    pub fn decode(frame: &Frame) -> Result<Self, ReqJoinError> {
        if frame.packet_id() != REQ_JOIN_PACKET_ID {
            return Err(ReqJoinError::UnexpectedPacketId {
                actual: frame.packet_id(),
            });
        }

        let payload = frame.payload();
        if payload.len() < MINIMUM_PAYLOAD_LENGTH {
            return Err(ReqJoinError::PayloadTooShort {
                actual: payload.len(),
                minimum: MINIMUM_PAYLOAD_LENGTH,
            });
        }

        let player_name_and_tail = &payload[FIXED_PREFIX_LENGTH..];
        let player_name_length = player_name_and_tail
            .iter()
            .position(is_nul)
            .ok_or(ReqJoinError::MissingPlayerNameTerminator)?;

        if player_name_length == 0 {
            return Err(ReqJoinError::EmptyPlayerName);
        }

        if player_name_length > MAX_PLAYER_NAME_BYTES {
            return Err(ReqJoinError::PlayerNameTooLong {
                actual: player_name_length,
                maximum: MAX_PLAYER_NAME_BYTES,
            });
        }

        let tail_offset = FIXED_PREFIX_LENGTH + player_name_length + 1;
        Ok(Self {
            host_counter: read_u32_le(payload, 0),
            entry_key: read_u32_le(payload, 4),
            unknown: payload[8],
            listen_port: read_u16_le(payload, 9),
            join_counter: read_u32_le(payload, 11),
            player_name: player_name_and_tail[..player_name_length].to_vec(),
            tail: payload[tail_offset..].to_vec(),
        })
    }

    pub fn host_counter(&self) -> u32 {
        self.host_counter
    }

    pub fn entry_key(&self) -> u32 {
        self.entry_key
    }

    pub fn unknown(&self) -> u8 {
        self.unknown
    }

    pub fn listen_port(&self) -> u16 {
        self.listen_port
    }

    pub fn join_counter(&self) -> u32 {
        self.join_counter
    }

    pub fn player_name_bytes(&self) -> &[u8] {
        &self.player_name
    }

    pub fn tail(&self) -> &[u8] {
        &self.tail
    }
}

#[derive(Debug, Error, Eq, PartialEq)]
pub enum ReqJoinError {
    #[error("expected W3GS_REQJOIN packet 0x1E, got 0x{actual:02X}")]
    UnexpectedPacketId { actual: u8 },
    #[error("REQJOIN payload requires at least {minimum} bytes, got {actual}")]
    PayloadTooShort { actual: usize, minimum: usize },
    #[error("REQJOIN player name is missing its NUL terminator")]
    MissingPlayerNameTerminator,
    #[error("REQJOIN player name must not be empty")]
    EmptyPlayerName,
    #[error("REQJOIN player name contains {actual} bytes; maximum is {maximum}")]
    PlayerNameTooLong { actual: usize, maximum: usize },
}

fn read_u16_le(bytes: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes([bytes[offset], bytes[offset + 1]])
}

fn read_u32_le(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes([
        bytes[offset],
        bytes[offset + 1],
        bytes[offset + 2],
        bytes[offset + 3],
    ])
}

fn is_nul(byte: &u8) -> bool {
    *byte == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_the_confirmed_req_join_prefix_and_preserves_the_tail() {
        let frame = req_join_frame(b"StrajerPlayer");
        let req_join = ReqJoin::decode(&frame).expect("REQJOIN should decode");

        assert_eq!(req_join.host_counter(), 0x1234_5678);
        assert_eq!(req_join.entry_key(), 0x5354_524A);
        assert_eq!(req_join.unknown(), 0);
        assert_eq!(req_join.listen_port(), 6_112);
        assert_eq!(req_join.join_counter(), 0x0102_0304);
        assert_eq!(req_join.player_name_bytes(), b"StrajerPlayer");
        assert_eq!(req_join.tail(), modern_req_join_tail());
    }

    #[test]
    fn preserves_non_utf8_player_name_bytes() {
        let frame = req_join_frame(&[0x53, 0xFF]);
        let req_join = ReqJoin::decode(&frame).expect("raw name bytes should decode");

        assert_eq!(req_join.player_name_bytes(), &[0x53, 0xFF]);
    }

    #[test]
    fn rejects_an_unexpected_packet_id() {
        let frame =
            Frame::new(0x46, vec![0; MINIMUM_PAYLOAD_LENGTH]).expect("test frame should build");

        assert_eq!(
            ReqJoin::decode(&frame),
            Err(ReqJoinError::UnexpectedPacketId { actual: 0x46 })
        );
    }

    #[test]
    fn rejects_a_missing_player_name_terminator() {
        let mut payload = fixed_prefix();
        payload.extend_from_slice(b"Player");
        let frame = Frame::new(REQ_JOIN_PACKET_ID, payload).expect("test frame should build");

        assert_eq!(
            ReqJoin::decode(&frame),
            Err(ReqJoinError::MissingPlayerNameTerminator)
        );
    }

    #[test]
    fn rejects_an_empty_player_name() {
        let mut payload = fixed_prefix();
        payload.push(0);
        let frame = Frame::new(REQ_JOIN_PACKET_ID, payload).expect("test frame should build");

        assert_eq!(ReqJoin::decode(&frame), Err(ReqJoinError::EmptyPlayerName));
    }

    #[test]
    fn rejects_an_unbounded_player_name() {
        let mut payload = fixed_prefix();
        payload.extend(std::iter::repeat_n(b'X', MAX_PLAYER_NAME_BYTES + 1));
        payload.push(0);
        let frame = Frame::new(REQ_JOIN_PACKET_ID, payload).expect("test frame should build");

        assert_eq!(
            ReqJoin::decode(&frame),
            Err(ReqJoinError::PlayerNameTooLong {
                actual: MAX_PLAYER_NAME_BYTES + 1,
                maximum: MAX_PLAYER_NAME_BYTES,
            })
        );
    }

    fn req_join_frame(player_name: &[u8]) -> Frame {
        let mut payload = fixed_prefix();
        payload.extend_from_slice(player_name);
        payload.push(0);
        payload.extend_from_slice(modern_req_join_tail());
        Frame::new(REQ_JOIN_PACKET_ID, payload).expect("REQJOIN fixture should build")
    }

    fn fixed_prefix() -> Vec<u8> {
        let mut payload = Vec::new();
        payload.extend_from_slice(&0x1234_5678_u32.to_le_bytes());
        payload.extend_from_slice(&0x5354_524A_u32.to_le_bytes());
        payload.push(0);
        payload.extend_from_slice(&6_112_u16.to_le_bytes());
        payload.extend_from_slice(&0x0102_0304_u32.to_le_bytes());
        payload
    }

    fn modern_req_join_tail() -> &'static [u8] {
        &[
            2, 0, 0, 2, 0, 0x17, 0xE0, 192, 168, 1, 10, 0, 0, 0, 0, 0, 0, 0, 0,
        ]
    }
}
