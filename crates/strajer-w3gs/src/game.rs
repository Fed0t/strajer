use crc32fast::hash;
use thiserror::Error;

use crate::{Frame, FrameError};

pub const INCOMING_ACTION_PACKET_ID: u8 = 0x0C;
pub const OUTGOING_ACTION_PACKET_ID: u8 = 0x26;
pub const OUTGOING_KEEPALIVE_PACKET_ID: u8 = 0x27;
pub const INCOMING_ACTION_2_PACKET_ID: u8 = 0x48;
pub const MAX_ACTION_DATA_BYTES: usize = 1_449;

const MAX_ACTION_SUBPACKET_BYTES: usize = 1_452;
const MAX_INCOMING_ACTION_FRAME_BYTES: usize = 1_460;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OutgoingAction {
    data: Vec<u8>,
}

impl OutgoingAction {
    pub fn decode(frame: &Frame) -> Result<Self, W3gsGameFrameError> {
        require_packet_id(frame, OUTGOING_ACTION_PACKET_ID)?;
        let payload = frame.payload();
        if payload.len() < 4 {
            return Err(W3gsGameFrameError::InvalidOutgoingActionLength {
                actual: payload.len(),
            });
        }

        let expected_crc = u32::from_le_bytes(
            payload[..4]
                .try_into()
                .expect("outgoing action CRC length was validated"),
        );
        let data = &payload[4..];
        validate_action_data(data)?;
        let actual_crc = hash(data);
        if expected_crc != actual_crc {
            return Err(W3gsGameFrameError::InvalidActionChecksum {
                expected: expected_crc,
                actual: actual_crc,
            });
        }

        Ok(Self {
            data: data.to_vec(),
        })
    }

    pub fn data(&self) -> &[u8] {
        &self.data
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct OutgoingKeepAlive {
    unknown: u8,
    checksum: u32,
}

impl OutgoingKeepAlive {
    pub fn decode(frame: &Frame) -> Result<Self, W3gsGameFrameError> {
        require_packet_id(frame, OUTGOING_KEEPALIVE_PACKET_ID)?;
        let payload = frame.payload();
        if payload.len() != 5 {
            return Err(W3gsGameFrameError::InvalidKeepAliveLength {
                actual: payload.len(),
            });
        }

        Ok(Self {
            unknown: payload[0],
            checksum: u32::from_le_bytes(
                payload[1..5]
                    .try_into()
                    .expect("keepalive checksum length was validated"),
            ),
        })
    }

    pub fn unknown(self) -> u8 {
        self.unknown
    }

    pub fn checksum(self) -> u32 {
        self.checksum
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PlayerAction {
    player_id: u8,
    data: Vec<u8>,
}

impl PlayerAction {
    pub fn new(player_id: u8, data: Vec<u8>) -> Result<Self, W3gsGameFrameError> {
        validate_player_id(player_id)?;
        validate_action_data(&data)?;
        Ok(Self { player_id, data })
    }

    pub fn player_id(&self) -> u8 {
        self.player_id
    }

    pub fn data(&self) -> &[u8] {
        &self.data
    }

    fn encoded_length(&self) -> usize {
        3 + self.data.len()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IncomingActionFrame {
    fragment: bool,
    time_increment_ms: u16,
    actions: Vec<PlayerAction>,
}

impl IncomingActionFrame {
    pub fn decode(frame: &Frame) -> Result<Self, W3gsGameFrameError> {
        let fragment = match frame.packet_id() {
            INCOMING_ACTION_PACKET_ID => false,
            INCOMING_ACTION_2_PACKET_ID => true,
            actual => return Err(W3gsGameFrameError::UnexpectedPacketId { actual }),
        };
        if frame.encoded_length() > MAX_INCOMING_ACTION_FRAME_BYTES {
            return Err(W3gsGameFrameError::IncomingActionFrameTooLarge {
                actual: frame.encoded_length(),
                maximum: MAX_INCOMING_ACTION_FRAME_BYTES,
            });
        }

        let payload = frame.payload();
        if payload.len() < 2 {
            return Err(W3gsGameFrameError::MalformedIncomingAction);
        }
        let time_increment_ms = u16::from_le_bytes([payload[0], payload[1]]);
        if fragment && time_increment_ms != 0 {
            return Err(W3gsGameFrameError::FragmentAdvancesTime(time_increment_ms));
        }
        if payload.len() == 2 {
            if fragment {
                return Err(W3gsGameFrameError::EmptyActionFragment);
            }
            return Ok(Self {
                fragment,
                time_increment_ms,
                actions: Vec::new(),
            });
        }
        if payload.len() < 4 {
            return Err(W3gsGameFrameError::MalformedIncomingAction);
        }

        let expected_crc = u16::from_le_bytes([payload[2], payload[3]]);
        let action_bytes = &payload[4..];
        let actual_crc = hash(action_bytes) as u16;
        if expected_crc != actual_crc {
            return Err(W3gsGameFrameError::InvalidIncomingActionChecksum {
                expected: expected_crc,
                actual: actual_crc,
            });
        }

        let actions = decode_player_actions(action_bytes)?;
        if actions.is_empty() {
            return Err(W3gsGameFrameError::MalformedIncomingAction);
        }
        Ok(Self {
            fragment,
            time_increment_ms,
            actions,
        })
    }

    pub fn is_fragment(&self) -> bool {
        self.fragment
    }

    pub fn time_increment_ms(&self) -> u16 {
        self.time_increment_ms
    }

    pub fn actions(&self) -> &[PlayerAction] {
        &self.actions
    }
}

pub fn incoming_action_frames(
    time_increment_ms: u16,
    actions: &[PlayerAction],
) -> Result<Vec<Frame>, W3gsGameFrameError> {
    if actions.is_empty() {
        return Ok(vec![Frame::new(
            INCOMING_ACTION_PACKET_ID,
            time_increment_ms.to_le_bytes().to_vec(),
        )?]);
    }

    let mut groups: Vec<&[PlayerAction]> = Vec::new();
    let mut group_start = 0;
    let mut group_bytes = 0;
    for (index, action) in actions.iter().enumerate() {
        validate_player_id(action.player_id)?;
        validate_action_data(&action.data)?;
        let action_bytes = action.encoded_length();
        if action_bytes > MAX_ACTION_SUBPACKET_BYTES {
            return Err(W3gsGameFrameError::ActionRecordTooLarge {
                actual: action_bytes,
                maximum: MAX_ACTION_SUBPACKET_BYTES,
            });
        }
        if group_bytes > 0 && group_bytes + action_bytes > MAX_ACTION_SUBPACKET_BYTES {
            groups.push(&actions[group_start..index]);
            group_start = index;
            group_bytes = 0;
        }
        group_bytes += action_bytes;
    }
    groups.push(&actions[group_start..]);

    let last_index = groups.len() - 1;
    let mut frames = Vec::with_capacity(groups.len());
    for (index, group) in groups.into_iter().enumerate() {
        let fragment = index != last_index;
        frames.push(encode_action_group(
            if fragment {
                INCOMING_ACTION_2_PACKET_ID
            } else {
                INCOMING_ACTION_PACKET_ID
            },
            if fragment { 0 } else { time_increment_ms },
            group,
        )?);
    }
    Ok(frames)
}

fn encode_action_group(
    packet_id: u8,
    time_increment_ms: u16,
    actions: &[PlayerAction],
) -> Result<Frame, W3gsGameFrameError> {
    let action_bytes_length = actions.iter().map(PlayerAction::encoded_length).sum();
    let mut action_bytes = Vec::with_capacity(action_bytes_length);
    for action in actions {
        action_bytes.push(action.player_id);
        action_bytes.extend_from_slice(&(action.data.len() as u16).to_le_bytes());
        action_bytes.extend_from_slice(&action.data);
    }

    let crc = hash(&action_bytes) as u16;
    let mut payload = Vec::with_capacity(4 + action_bytes.len());
    payload.extend_from_slice(&time_increment_ms.to_le_bytes());
    payload.extend_from_slice(&crc.to_le_bytes());
    payload.extend_from_slice(&action_bytes);
    Ok(Frame::new(packet_id, payload)?)
}

fn decode_player_actions(bytes: &[u8]) -> Result<Vec<PlayerAction>, W3gsGameFrameError> {
    let mut actions = Vec::new();
    let mut offset = 0;
    while offset < bytes.len() {
        if bytes.len() - offset < 3 {
            return Err(W3gsGameFrameError::MalformedIncomingAction);
        }
        let player_id = bytes[offset];
        let action_length = usize::from(u16::from_le_bytes([bytes[offset + 1], bytes[offset + 2]]));
        offset += 3;
        let action_end = offset
            .checked_add(action_length)
            .ok_or(W3gsGameFrameError::MalformedIncomingAction)?;
        if action_end > bytes.len() {
            return Err(W3gsGameFrameError::MalformedIncomingAction);
        }
        actions.push(PlayerAction::new(
            player_id,
            bytes[offset..action_end].to_vec(),
        )?);
        offset = action_end;
    }
    Ok(actions)
}

fn require_packet_id(frame: &Frame, expected: u8) -> Result<(), W3gsGameFrameError> {
    if frame.packet_id() != expected {
        return Err(W3gsGameFrameError::UnexpectedPacketId {
            actual: frame.packet_id(),
        });
    }
    Ok(())
}

fn validate_player_id(player_id: u8) -> Result<(), W3gsGameFrameError> {
    if player_id == 0 {
        return Err(W3gsGameFrameError::InvalidPlayerId);
    }
    Ok(())
}

fn validate_action_data(data: &[u8]) -> Result<(), W3gsGameFrameError> {
    if data.len() > MAX_ACTION_DATA_BYTES {
        return Err(W3gsGameFrameError::ActionDataTooLarge {
            actual: data.len(),
            maximum: MAX_ACTION_DATA_BYTES,
        });
    }
    Ok(())
}

#[derive(Debug, Error)]
pub enum W3gsGameFrameError {
    #[error("unexpected W3GS gameplay packet 0x{actual:02X}")]
    UnexpectedPacketId { actual: u8 },
    #[error("W3GS_OUTGOING_ACTION payload contains {actual} bytes; expected at least 4")]
    InvalidOutgoingActionLength { actual: usize },
    #[error("W3GS action CRC32 mismatch: expected 0x{expected:08X}, got 0x{actual:08X}")]
    InvalidActionChecksum { expected: u32, actual: u32 },
    #[error("W3GS action contains {actual} bytes; maximum is {maximum}")]
    ActionDataTooLarge { actual: usize, maximum: usize },
    #[error("W3GS action record contains {actual} bytes; maximum is {maximum}")]
    ActionRecordTooLarge { actual: usize, maximum: usize },
    #[error("W3GS_OUTGOING_KEEPALIVE payload contains {actual} bytes; expected 5")]
    InvalidKeepAliveLength { actual: usize },
    #[error("W3GS player id must not be zero")]
    InvalidPlayerId,
    #[error("W3GS incoming action frame contains {actual} bytes; maximum is {maximum}")]
    IncomingActionFrameTooLarge { actual: usize, maximum: usize },
    #[error("malformed W3GS incoming action payload")]
    MalformedIncomingAction,
    #[error("W3GS incoming action CRC16 mismatch: expected 0x{expected:04X}, got 0x{actual:04X}")]
    InvalidIncomingActionChecksum { expected: u16, actual: u16 },
    #[error("W3GS_INCOMING_ACTION2 must not advance game time, got {0} ms")]
    FragmentAdvancesTime(u16),
    #[error("W3GS_INCOMING_ACTION2 must contain at least one action")]
    EmptyActionFragment,
    #[error(transparent)]
    Frame(#[from] FrameError),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_crc_checked_outgoing_actions() {
        let data = vec![0x10, 0x20, 0x30];
        let mut payload = hash(&data).to_le_bytes().to_vec();
        payload.extend_from_slice(&data);
        let frame =
            Frame::new(OUTGOING_ACTION_PACKET_ID, payload).expect("outgoing action should build");

        assert_eq!(
            OutgoingAction::decode(&frame)
                .expect("outgoing action should decode")
                .data(),
            data
        );

        let invalid = Frame::new(OUTGOING_ACTION_PACKET_ID, vec![0; 5])
            .expect("invalid outgoing action should build");
        assert!(matches!(
            OutgoingAction::decode(&invalid),
            Err(W3gsGameFrameError::InvalidActionChecksum { .. })
        ));
    }

    #[test]
    fn decodes_keepalive_checksum() {
        let frame = Frame::new(
            OUTGOING_KEEPALIVE_PACKET_ID,
            vec![7, 0x78, 0x56, 0x34, 0x12],
        )
        .expect("keepalive should build");
        let keepalive = OutgoingKeepAlive::decode(&frame).expect("keepalive should decode");

        assert_eq!(keepalive.unknown(), 7);
        assert_eq!(keepalive.checksum(), 0x1234_5678);
    }

    #[test]
    fn encodes_empty_timeslots_without_a_crc() {
        let frames = incoming_action_frames(100, &[]).expect("empty timeslot should encode");

        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].to_bytes(), [0xF7, 0x0C, 6, 0, 100, 0]);
        let decoded = IncomingActionFrame::decode(&frames[0]).expect("timeslot should decode");
        assert_eq!(decoded.time_increment_ms(), 100);
        assert!(decoded.actions().is_empty());
    }

    #[test]
    fn fragments_large_action_batches_before_the_timed_frame() {
        let actions = vec![
            PlayerAction::new(1, vec![0xAA; 800]).expect("first action should build"),
            PlayerAction::new(2, vec![0xBB; 800]).expect("second action should build"),
        ];
        let frames = incoming_action_frames(100, &actions).expect("timeslots should encode");

        assert_eq!(frames.len(), 2);
        let fragment = IncomingActionFrame::decode(&frames[0]).expect("fragment should decode");
        let final_frame = IncomingActionFrame::decode(&frames[1]).expect("final should decode");
        assert!(fragment.is_fragment());
        assert_eq!(fragment.time_increment_ms(), 0);
        assert_eq!(fragment.actions(), &actions[..1]);
        assert!(!final_frame.is_fragment());
        assert_eq!(final_frame.time_increment_ms(), 100);
        assert_eq!(final_frame.actions(), &actions[1..]);
        assert!(frames.iter().all(|frame| frame.encoded_length() <= 1_460));
    }
}
