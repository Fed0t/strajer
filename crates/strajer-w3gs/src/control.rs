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

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LobbyChatToHost {
    from_player_id: u8,
    recipient_player_ids: Vec<u8>,
    message: String,
}

impl LobbyChatToHost {
    pub fn decode(frame: &Frame) -> Result<Self, ControlFrameError> {
        if frame.packet_id() != CHAT_TO_HOST_PACKET_ID {
            return Err(ControlFrameError::UnexpectedPacketId {
                actual: frame.packet_id(),
            });
        }

        let payload = frame.payload();
        let Some(&recipient_count) = payload.first() else {
            return Err(ControlFrameError::MalformedLobbyChat);
        };
        let recipient_count = usize::from(recipient_count);
        let sender_index = 1_usize
            .checked_add(recipient_count)
            .ok_or(ControlFrameError::MalformedLobbyChat)?;
        let flag_index = sender_index
            .checked_add(1)
            .ok_or(ControlFrameError::MalformedLobbyChat)?;
        let message_index = flag_index
            .checked_add(1)
            .ok_or(ControlFrameError::MalformedLobbyChat)?;
        if recipient_count == 0 || payload.len() <= message_index {
            return Err(ControlFrameError::MalformedLobbyChat);
        }

        let recipient_player_ids = payload[1..sender_index].to_vec();
        validate_chat_recipients(&recipient_player_ids)?;
        let from_player_id = payload[sender_index];
        if from_player_id == 0 {
            return Err(ControlFrameError::InvalidSenderPlayerId);
        }

        let flag = payload[flag_index];
        if flag != LOBBY_CHAT_FLAG {
            return Err(ControlFrameError::UnsupportedChatFlag(flag));
        }

        let encoded_message = &payload[message_index..];
        if encoded_message.last() != Some(&0)
            || encoded_message[..encoded_message.len() - 1].contains(&0)
        {
            return Err(ControlFrameError::MalformedLobbyChat);
        }
        let message = std::str::from_utf8(&encoded_message[..encoded_message.len() - 1])
            .map_err(|_| ControlFrameError::InvalidMessageEncoding)?
            .to_owned();
        validate_chat_message(&message)?;

        Ok(Self {
            from_player_id,
            recipient_player_ids,
            message,
        })
    }

    pub fn from_player_id(&self) -> u8 {
        self.from_player_id
    }

    pub fn recipient_player_ids(&self) -> &[u8] {
        &self.recipient_player_ids
    }

    pub fn message(&self) -> &str {
        &self.message
    }
}

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
    validate_chat_recipients(recipient_player_ids)?;
    validate_chat_message(message)?;

    let mut payload = Vec::with_capacity(4 + recipient_player_ids.len() + message.len());
    payload.push(recipient_player_ids.len() as u8);
    payload.extend_from_slice(recipient_player_ids);
    payload.push(from_player_id);
    payload.push(LOBBY_CHAT_FLAG);
    payload.extend_from_slice(message.as_bytes());
    payload.push(0);
    Ok(Frame::new(CHAT_FROM_HOST_PACKET_ID, payload)?)
}

fn validate_chat_recipients(recipient_player_ids: &[u8]) -> Result<(), ControlFrameError> {
    if recipient_player_ids.is_empty()
        || recipient_player_ids.len() > usize::from(u8::MAX)
        || recipient_player_ids.contains(&0)
    {
        return Err(ControlFrameError::InvalidRecipients);
    }

    let mut seen = [false; 256];
    for &player_id in recipient_player_ids {
        if seen[usize::from(player_id)] {
            return Err(ControlFrameError::InvalidRecipients);
        }
        seen[usize::from(player_id)] = true;
    }

    Ok(())
}

fn validate_chat_message(message: &str) -> Result<(), ControlFrameError> {
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

    Ok(())
}

#[derive(Debug, Error)]
pub enum ControlFrameError {
    #[error("expected W3GS_CHAT_TO_HOST packet, got 0x{actual:02X}")]
    UnexpectedPacketId { actual: u8 },
    #[error("W3GS lobby chat payload is malformed")]
    MalformedLobbyChat,
    #[error("W3GS chat sender player id must not be zero")]
    InvalidSenderPlayerId,
    #[error("W3GS chat must contain 1 to 255 non-zero recipient player ids")]
    InvalidRecipients,
    #[error("W3GS chat flag 0x{0:02X} is not a lobby message")]
    UnsupportedChatFlag(u8),
    #[error("W3GS chat message is not valid UTF-8")]
    InvalidMessageEncoding,
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
    fn decodes_lobby_chat_to_host() {
        let mut payload = vec![2, 1, 2, 1, LOBBY_CHAT_FLAG];
        payload.extend_from_slice(b"hello from player one\0");
        let frame = Frame::new(CHAT_TO_HOST_PACKET_ID, payload).expect("chat frame should build");

        let chat = LobbyChatToHost::decode(&frame).expect("chat should decode");

        assert_eq!(chat.from_player_id(), 1);
        assert_eq!(chat.recipient_player_ids(), &[1, 2]);
        assert_eq!(chat.message(), "hello from player one");
    }

    #[test]
    fn rejects_non_lobby_chat_flags_and_missing_terminators() {
        let unsupported = Frame::new(CHAT_TO_HOST_PACKET_ID, vec![1, 1, 1, 0x11, 2])
            .expect("slot change frame should build");
        assert!(matches!(
            LobbyChatToHost::decode(&unsupported),
            Err(ControlFrameError::UnsupportedChatFlag(0x11))
        ));

        let malformed = Frame::new(
            CHAT_TO_HOST_PACKET_ID,
            vec![1, 1, 1, LOBBY_CHAT_FLAG, b'h', b'i'],
        )
        .expect("malformed chat frame should build");
        assert!(matches!(
            LobbyChatToHost::decode(&malformed),
            Err(ControlFrameError::MalformedLobbyChat)
        ));
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
