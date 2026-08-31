use std::net::SocketAddrV4;

use thiserror::Error;

use crate::net::encode_ipv4_socket_address;
use crate::{Frame, FrameError};

pub const PLAYER_INFO_PACKET_ID: u8 = 0x06;
pub const PLAYER_LEAVE_OTHERS_PACKET_ID: u8 = 0x07;
pub const GAME_LOADED_OTHERS_PACKET_ID: u8 = 0x08;
pub const GAME_OVER_PACKET_ID: u8 = 0x14;
pub const GAME_LOADED_SELF_PACKET_ID: u8 = 0x23;
pub const MAX_CLASSIC_PLAYER_NAME_BYTES: usize = 15;
const PLAYER_JOIN_COUNTER: u32 = 1;
const PLAYER_LEAVE_LOBBY_CODE: u32 = 13;

pub fn player_info_frame(
    player_id: u8,
    player_name: &str,
    external_address: SocketAddrV4,
    internal_address: SocketAddrV4,
) -> Result<Frame, PlayerFrameError> {
    validate_player(player_id, player_name)?;

    let mut payload = Vec::with_capacity(41 + player_name.len());
    payload.extend_from_slice(&PLAYER_JOIN_COUNTER.to_le_bytes());
    payload.push(player_id);
    payload.extend_from_slice(player_name.as_bytes());
    payload.push(0);
    payload.extend_from_slice(&[2, 0, 0]);
    encode_ipv4_socket_address(&mut payload, external_address);
    encode_ipv4_socket_address(&mut payload, internal_address);
    Ok(Frame::new(PLAYER_INFO_PACKET_ID, payload)?)
}

pub fn player_leave_others_frame(player_id: u8) -> Result<Frame, PlayerFrameError> {
    player_leave_others_frame_with_reason(player_id, PLAYER_LEAVE_LOBBY_CODE)
}

pub fn player_leave_others_frame_with_reason(
    player_id: u8,
    reason: u32,
) -> Result<Frame, PlayerFrameError> {
    if player_id == 0 {
        return Err(PlayerFrameError::InvalidPlayerId);
    }

    let mut payload = Vec::with_capacity(5);
    payload.push(player_id);
    payload.extend_from_slice(&reason.to_le_bytes());
    Ok(Frame::new(PLAYER_LEAVE_OTHERS_PACKET_ID, payload)?)
}

pub fn leave_request_reason(frame: &Frame) -> Result<u32, PlayerFrameError> {
    if frame.packet_id() != crate::LEAVE_REQUEST_PACKET_ID {
        return Err(PlayerFrameError::UnexpectedPacketId {
            actual: frame.packet_id(),
        });
    }
    if frame.payload().len() != 4 {
        return Err(PlayerFrameError::InvalidLeaveRequestPayload {
            actual: frame.payload().len(),
        });
    }
    Ok(u32::from_le_bytes(
        frame
            .payload()
            .try_into()
            .expect("leave request payload length was validated"),
    ))
}

pub fn game_over_frame(player_id: u8) -> Result<Frame, PlayerFrameError> {
    if player_id == 0 {
        return Err(PlayerFrameError::InvalidPlayerId);
    }
    Ok(Frame::new(GAME_OVER_PACKET_ID, vec![player_id])?)
}

pub fn game_loaded_others_frame(player_id: u8) -> Result<Frame, PlayerFrameError> {
    if player_id == 0 {
        return Err(PlayerFrameError::InvalidPlayerId);
    }

    Ok(Frame::new(GAME_LOADED_OTHERS_PACKET_ID, vec![player_id])?)
}

pub fn validate_game_loaded_self(frame: &Frame) -> Result<(), PlayerFrameError> {
    if frame.packet_id() != GAME_LOADED_SELF_PACKET_ID {
        return Err(PlayerFrameError::UnexpectedPacketId {
            actual: frame.packet_id(),
        });
    }
    if !frame.payload().is_empty() {
        return Err(PlayerFrameError::InvalidGameLoadedSelfPayload);
    }

    Ok(())
}

fn validate_player(player_id: u8, player_name: &str) -> Result<(), PlayerFrameError> {
    if player_id == 0 {
        return Err(PlayerFrameError::InvalidPlayerId);
    }

    if player_name.is_empty()
        || player_name.len() > MAX_CLASSIC_PLAYER_NAME_BYTES
        || player_name.contains('\0')
    {
        return Err(PlayerFrameError::InvalidPlayerNameLength {
            actual: player_name.len(),
            maximum: MAX_CLASSIC_PLAYER_NAME_BYTES,
        });
    }

    Ok(())
}

#[derive(Debug, Error)]
pub enum PlayerFrameError {
    #[error("expected W3GS_GAMELOADED_SELF packet, got 0x{actual:02X}")]
    UnexpectedPacketId { actual: u8 },
    #[error("W3GS_GAMELOADED_SELF payload must be empty")]
    InvalidGameLoadedSelfPayload,
    #[error("W3GS_LEAVE_REQUEST payload contains {actual} bytes; expected 4")]
    InvalidLeaveRequestPayload { actual: usize },
    #[error("W3GS player id must not be zero")]
    InvalidPlayerId,
    #[error("W3GS player name contains {actual} bytes; expected 1 to {maximum} without NUL")]
    InvalidPlayerNameLength { actual: usize, maximum: usize },
    #[error(transparent)]
    Frame(#[from] FrameError),
}

#[cfg(test)]
mod tests {
    use std::net::Ipv4Addr;

    use super::*;

    #[test]
    fn encodes_player_info_in_the_classic_w3gs_layout() {
        let address = SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0);
        let frame = player_info_frame(2, "Friend#1234", address, address)
            .expect("player info should encode");

        assert_eq!(frame.packet_id(), PLAYER_INFO_PACKET_ID);
        assert_eq!(&frame.payload()[..5], &[1, 0, 0, 0, 2]);
        assert_eq!(&frame.payload()[5..17], b"Friend#1234\0");
        assert_eq!(&frame.payload()[17..20], &[2, 0, 0]);
        assert_eq!(frame.payload().len(), 52);
    }

    #[test]
    fn encodes_a_lobby_leave_notification() {
        let frame = player_leave_others_frame(2).expect("leave should encode");

        assert_eq!(frame.packet_id(), PLAYER_LEAVE_OTHERS_PACKET_ID);
        assert_eq!(frame.payload(), &[2, 13, 0, 0, 0]);
    }

    #[test]
    fn decodes_leave_reason_and_encodes_game_over() {
        let leave = Frame::new(
            crate::LEAVE_REQUEST_PACKET_ID,
            0x07_u32.to_le_bytes().to_vec(),
        )
        .expect("leave request should build");

        assert_eq!(
            leave_request_reason(&leave).expect("leave reason should decode"),
            0x07
        );
        assert_eq!(
            player_leave_others_frame_with_reason(2, 0x07)
                .expect("leave notification should build")
                .payload(),
            &[2, 7, 0, 0, 0]
        );
        assert_eq!(
            game_over_frame(11)
                .expect("game over should build")
                .to_bytes(),
            [0xF7, 0x14, 5, 0, 11]
        );
    }

    #[test]
    fn marks_the_virtual_host_as_loaded() {
        let frame = game_loaded_others_frame(11).expect("loaded frame should encode");

        assert_eq!(frame.to_bytes(), [0xF7, 0x08, 5, 0, 11]);
    }

    #[test]
    fn validates_game_loaded_self() {
        let loaded = Frame::new(GAME_LOADED_SELF_PACKET_ID, Vec::new())
            .expect("loaded-self frame should build");
        validate_game_loaded_self(&loaded).expect("loaded-self frame should validate");

        let invalid = Frame::new(GAME_LOADED_SELF_PACKET_ID, vec![1])
            .expect("invalid loaded-self frame should build");
        assert!(matches!(
            validate_game_loaded_self(&invalid),
            Err(PlayerFrameError::InvalidGameLoadedSelfPayload)
        ));
    }

    #[test]
    fn rejects_names_above_the_w3gs_limit() {
        let address = SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0);
        let error = player_info_frame(1, "1234567890123456", address, address)
            .expect_err("long name must fail");

        assert!(matches!(
            error,
            PlayerFrameError::InvalidPlayerNameLength {
                actual: 16,
                maximum: MAX_CLASSIC_PLAYER_NAME_BYTES
            }
        ));
    }
}
