use thiserror::Error;

use crate::{Frame, FrameError};

pub const PING_FROM_HOST_PACKET_ID: u8 = 0x01;
pub const COUNTDOWN_START_PACKET_ID: u8 = 0x0A;
pub const COUNTDOWN_END_PACKET_ID: u8 = 0x0B;
pub const CHAT_FROM_HOST_PACKET_ID: u8 = 0x0F;
pub const LEAVE_ACK_PACKET_ID: u8 = 0x1B;
pub const LEAVE_REQUEST_PACKET_ID: u8 = 0x21;
pub const CHAT_TO_HOST_PACKET_ID: u8 = 0x28;
pub const PONG_TO_HOST_PACKET_ID: u8 = 0x46;
pub const MAX_CHAT_MESSAGE_BYTES: usize = 254;
const LOBBY_CHAT_FLAG: u8 = 0x10;

pub fn ping_from_host(payload: u32) -> Result<Frame, FrameError> {
    Frame::new(PING_FROM_HOST_PACKET_ID, payload.to_le_bytes().to_vec())
}

pub fn leave_ack() -> Result<Frame, FrameError> {
    Frame::new(LEAVE_ACK_PACKET_ID, Vec::new())
}

pub fn countdown_start() -> Result<Frame, FrameError> {
    Frame::new(COUNTDOWN_START_PACKET_ID, Vec::new())
}

pub fn countdown_end() -> Result<Frame, FrameError> {
    Frame::new(COUNTDOWN_END_PACKET_ID, Vec::new())
}

pub fn chat_from_host(
    from_player_id: u8,
    recipient_player_ids: &[u8],
    message: &str,
) -> Result<Frame, ControlFrameError> {
    if from_player_id == 0 {
        return Err(ControlFrameError::InvalidSenderPlayerId);
    }
    if recipient_player_ids.is_empty()
        || recipient_player_ids.len() > usize::from(u8::MAX)
        || recipient_player_ids.contains(&0)
    {
        return Err(ControlFrameError::InvalidRecipients);
    }
    if message.is_empty()
        || message.len() > MAX_CHAT_MESSAGE_BYTES
        || message.contains('\0')
        || message.chars().any(char::is_control)
    {
        return Err(ControlFrameError::InvalidMessage {
            actual: message.len(),
            maximum: MAX_CHAT_MESSAGE_BYTES,
        });
    }

    let mut payload = Vec::with_capacity(4 + recipient_player_ids.len() + message.len());
    payload.push(recipient_player_ids.len() as u8);
    payload.extend_from_slice(recipient_player_ids);
    payload.push(from_player_id);
    payload.push(LOBBY_CHAT_FLAG);
    payload.extend_from_slice(message.as_bytes());
    payload.push(0);
    Ok(Frame::new(CHAT_FROM_HOST_PACKET_ID, payload)?)
}

#[derive(Debug, Error)]
pub enum ControlFrameError {
    #[error("W3GS chat sender player id must not be zero")]
    InvalidSenderPlayerId,
    #[error("W3GS chat must contain 1 to 255 non-zero recipient player ids")]
    InvalidRecipients,
    #[error(
        "W3GS chat contains {actual} message bytes; expected 1 to {maximum} without controls or NUL"
    )]
    InvalidMessage { actual: usize, maximum: usize },
    #[error(transparent)]
    Frame(#[from] FrameError),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_control_frames() {
        assert_eq!(
            ping_from_host(0x1234_5678)
                .expect("ping should build")
                .to_bytes(),
            [0xF7, 0x01, 8, 0, 0x78, 0x56, 0x34, 0x12]
        );
        assert_eq!(
            leave_ack().expect("leave ack should build").to_bytes(),
            [0xF7, 0x1B, 4, 0]
        );
        assert_eq!(
            countdown_start()
                .expect("countdown start should build")
                .to_bytes(),
            [0xF7, 0x0A, 4, 0]
        );
        assert_eq!(
            countdown_end()
                .expect("countdown end should build")
                .to_bytes(),
            [0xF7, 0x0B, 4, 0]
        );
    }

    #[test]
    fn encodes_lobby_chat_from_the_virtual_host() {
        let frame =
            chat_from_host(11, &[1, 2], "Game starts in 60 seconds.").expect("chat should build");

        assert_eq!(frame.packet_id(), CHAT_FROM_HOST_PACKET_ID);
        assert_eq!(&frame.payload()[..5], &[2, 1, 2, 11, 0x10]);
        assert_eq!(&frame.payload()[5..], b"Game starts in 60 seconds.\0");
    }

    #[test]
    fn rejects_invalid_lobby_chat() {
        assert!(matches!(
            chat_from_host(0, &[1], "message"),
            Err(ControlFrameError::InvalidSenderPlayerId)
        ));
        assert!(matches!(
            chat_from_host(11, &[], "message"),
            Err(ControlFrameError::InvalidRecipients)
        ));
        assert!(matches!(
            chat_from_host(11, &[1], "bad\nmessage"),
            Err(ControlFrameError::InvalidMessage { .. })
        ));
    }
}
