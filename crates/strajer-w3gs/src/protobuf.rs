use prost::Message;
use thiserror::Error;

use crate::{Frame, FrameError};

pub const PROTOBUF_PACKET_ID: u8 = 0x59;
pub const PLAYER_PROFILE_MESSAGE_TYPE: u8 = 0x03;
pub const PLAYER_SKINS_MESSAGE_TYPE: u8 = 0x04;
pub const PLAYER_UNKNOWN_5_MESSAGE_TYPE: u8 = 0x05;
const ENVELOPE_HEADER_LENGTH: usize = 5;
const MAX_BATTLE_TAG_BYTES: usize = 255;

#[derive(Clone, PartialEq, Message)]
pub struct PlayerProfileMessage {
    #[prost(uint32, tag = "1")]
    pub player_id: u32,
    #[prost(string, tag = "2")]
    pub battle_tag: String,
    #[prost(string, tag = "3")]
    pub clan: String,
    #[prost(string, tag = "4")]
    pub portrait: String,
    #[prost(enumeration = "PlayerProfileRealm", tag = "5")]
    pub realm: i32,
    #[prost(string, tag = "6")]
    pub unknown_1: String,
}

#[derive(Clone, PartialEq, Message)]
pub struct PlayerSkinsMessage {
    #[prost(uint32, tag = "1")]
    pub player_id: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, prost::Enumeration)]
#[repr(i32)]
pub enum PlayerProfileRealm {
    Offline = 0,
    Americas = 10,
    Europe = 20,
    Asia = 30,
}

pub fn player_profile_frame(player_id: u8, battle_tag: String) -> Result<Frame, ProtobufError> {
    if player_id == 0 {
        return Err(ProtobufError::InvalidPlayerId);
    }

    if battle_tag.is_empty() || battle_tag.len() > MAX_BATTLE_TAG_BYTES {
        return Err(ProtobufError::InvalidBattleTagLength {
            actual: battle_tag.len(),
            maximum: MAX_BATTLE_TAG_BYTES,
        });
    }

    let message = PlayerProfileMessage {
        player_id: u32::from(player_id),
        battle_tag,
        clan: String::new(),
        portrait: "p042".to_owned(),
        realm: PlayerProfileRealm::Offline as i32,
        unknown_1: String::new(),
    };
    encode_envelope(PLAYER_PROFILE_MESSAGE_TYPE, message.encode_to_vec())
}

pub fn player_skins_frame(player_id: u8) -> Result<Frame, ProtobufError> {
    if player_id == 0 {
        return Err(ProtobufError::InvalidPlayerId);
    }

    let message = PlayerSkinsMessage {
        player_id: u32::from(player_id),
    };
    encode_envelope(PLAYER_SKINS_MESSAGE_TYPE, message.encode_to_vec())
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProtobufEnvelope {
    message_type: u8,
    data: Vec<u8>,
}

impl ProtobufEnvelope {
    pub fn decode(frame: &Frame) -> Result<Self, ProtobufError> {
        if frame.packet_id() != PROTOBUF_PACKET_ID {
            return Err(ProtobufError::UnexpectedPacketId {
                actual: frame.packet_id(),
            });
        }

        let payload = frame.payload();
        if payload.len() < ENVELOPE_HEADER_LENGTH {
            return Err(ProtobufError::PayloadTooShort {
                actual: payload.len(),
                minimum: ENVELOPE_HEADER_LENGTH,
            });
        }

        let declared_length = u32::from_le_bytes([payload[1], payload[2], payload[3], payload[4]]);
        let actual_length = payload.len() - ENVELOPE_HEADER_LENGTH;
        if usize::try_from(declared_length).ok() != Some(actual_length) {
            return Err(ProtobufError::DataLengthMismatch {
                declared: declared_length,
                actual: actual_length,
            });
        }

        Ok(Self {
            message_type: payload[0],
            data: payload[ENVELOPE_HEADER_LENGTH..].to_vec(),
        })
    }

    pub fn message_type(&self) -> u8 {
        self.message_type
    }

    pub fn data(&self) -> &[u8] {
        &self.data
    }

    pub fn should_echo_in_lobby(&self) -> bool {
        matches!(
            self.message_type,
            PLAYER_PROFILE_MESSAGE_TYPE | PLAYER_SKINS_MESSAGE_TYPE | PLAYER_UNKNOWN_5_MESSAGE_TYPE
        )
    }
}

fn encode_envelope(message_type: u8, data: Vec<u8>) -> Result<Frame, ProtobufError> {
    let data_length = u32::try_from(data.len()).map_err(|_| ProtobufError::DataTooLarge)?;
    let mut payload = Vec::with_capacity(ENVELOPE_HEADER_LENGTH + data.len());
    payload.push(message_type);
    payload.extend_from_slice(&data_length.to_le_bytes());
    payload.extend_from_slice(&data);
    Ok(Frame::new(PROTOBUF_PACKET_ID, payload)?)
}

#[derive(Debug, Error)]
pub enum ProtobufError {
    #[error("W3GS protobuf player id must not be zero")]
    InvalidPlayerId,
    #[error("W3GS battle tag contains {actual} bytes; expected 1 to {maximum}")]
    InvalidBattleTagLength { actual: usize, maximum: usize },
    #[error("expected W3GS protobuf packet 0x59, got 0x{actual:02X}")]
    UnexpectedPacketId { actual: u8 },
    #[error("W3GS protobuf payload requires at least {minimum} bytes, got {actual}")]
    PayloadTooShort { actual: usize, minimum: usize },
    #[error("W3GS protobuf payload declares {declared} data bytes, got {actual}")]
    DataLengthMismatch { declared: u32, actual: usize },
    #[error("W3GS protobuf message is too large")]
    DataTooLarge,
    #[error(transparent)]
    Frame(#[from] FrameError),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_a_reforged_player_profile() {
        let frame =
            player_profile_frame(1, "StrajerPlayer".to_owned()).expect("profile should encode");
        let envelope = ProtobufEnvelope::decode(&frame).expect("envelope should decode");
        let profile =
            PlayerProfileMessage::decode(envelope.data()).expect("profile protobuf should decode");

        assert_eq!(frame.packet_id(), PROTOBUF_PACKET_ID);
        assert_eq!(envelope.message_type(), PLAYER_PROFILE_MESSAGE_TYPE);
        assert!(envelope.should_echo_in_lobby());
        assert_eq!(profile.player_id, 1);
        assert_eq!(profile.battle_tag, "StrajerPlayer");
        assert_eq!(profile.portrait, "p042");
        assert_eq!(profile.realm, PlayerProfileRealm::Offline as i32);
    }

    #[test]
    fn encodes_empty_reforged_player_skins() {
        let frame = player_skins_frame(2).expect("skins should encode");
        let envelope = ProtobufEnvelope::decode(&frame).expect("envelope should decode");
        let skins =
            PlayerSkinsMessage::decode(envelope.data()).expect("skins protobuf should decode");

        assert_eq!(envelope.message_type(), PLAYER_SKINS_MESSAGE_TYPE);
        assert_eq!(skins.player_id, 2);
    }

    #[test]
    fn rejects_a_mismatched_envelope_length() {
        let frame =
            Frame::new(PROTOBUF_PACKET_ID, vec![3, 2, 0, 0, 0, 1]).expect("frame should build");
        let error = ProtobufEnvelope::decode(&frame).expect_err("length must be checked");

        assert!(matches!(
            error,
            ProtobufError::DataLengthMismatch {
                declared: 2,
                actual: 1
            }
        ));
    }
}
