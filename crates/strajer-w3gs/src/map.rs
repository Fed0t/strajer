use crc32fast::Hasher as Crc32Hasher;
use thiserror::Error;

use crate::{Frame, FrameError};

pub const MAP_CHECK_PACKET_ID: u8 = 0x3D;
pub const START_DOWNLOAD_PACKET_ID: u8 = 0x3F;
pub const MAP_SIZE_PACKET_ID: u8 = 0x42;
pub const MAP_PART_PACKET_ID: u8 = 0x43;
pub const MAP_PART_OK_PACKET_ID: u8 = 0x44;
pub const MAP_PART_NOT_OK_PACKET_ID: u8 = 0x45;
pub const MAP_PART_DATA_BYTES: usize = 1_442;
const MAP_SIZE_PAYLOAD_LENGTH: usize = 9;
const MAP_PART_OK_PAYLOAD_LENGTH: usize = 10;
const MAP_TRANSFER_MARKER: u32 = 1;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MapCheck {
    file_path: String,
    file_size: u32,
    file_crc32: u32,
    map_xoro: u32,
    map_sha1: [u8; 20],
}

impl MapCheck {
    pub fn new(
        file_path: String,
        file_size: u32,
        file_crc32: u32,
        map_xoro: u32,
        map_sha1: [u8; 20],
    ) -> Result<Self, MapCheckError> {
        if file_path.is_empty() || file_path.contains('\0') {
            return Err(MapCheckError::InvalidFilePath);
        }

        Ok(Self {
            file_path,
            file_size,
            file_crc32,
            map_xoro,
            map_sha1,
        })
    }

    pub fn frame(&self) -> Result<Frame, MapCheckError> {
        let mut payload = Vec::with_capacity(self.file_path.len() + 37);
        payload.extend_from_slice(&1_u32.to_le_bytes());
        payload.extend_from_slice(self.file_path.as_bytes());
        payload.push(0);
        payload.extend_from_slice(&self.file_size.to_le_bytes());
        payload.extend_from_slice(&self.file_crc32.to_le_bytes());
        payload.extend_from_slice(&self.map_xoro.to_le_bytes());
        payload.extend_from_slice(&self.map_sha1);
        Ok(Frame::new(MAP_CHECK_PACKET_ID, payload)?)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MapSize {
    size_flag: u8,
    map_size: u32,
}

impl MapSize {
    pub fn decode(frame: &Frame) -> Result<Self, MapSizeError> {
        if frame.packet_id() != MAP_SIZE_PACKET_ID {
            return Err(MapSizeError::UnexpectedPacketId {
                actual: frame.packet_id(),
            });
        }

        let payload = frame.payload();
        if payload.len() != MAP_SIZE_PAYLOAD_LENGTH {
            return Err(MapSizeError::InvalidPayloadLength {
                actual: payload.len(),
                expected: MAP_SIZE_PAYLOAD_LENGTH,
            });
        }

        let marker = u32::from_le_bytes([payload[0], payload[1], payload[2], payload[3]]);
        if marker != 1 {
            return Err(MapSizeError::InvalidMarker(marker));
        }

        Ok(Self {
            size_flag: payload[4],
            map_size: u32::from_le_bytes([payload[5], payload[6], payload[7], payload[8]]),
        })
    }

    pub fn has_map(self) -> bool {
        self.size_flag == 1
    }

    pub fn continues_download(self) -> bool {
        self.size_flag == 3
    }

    pub fn size_flag(self) -> u8 {
        self.size_flag
    }

    pub fn map_size(self) -> u32 {
        self.map_size
    }
}

pub fn start_download_frame(from_player_id: u8) -> Result<Frame, MapTransferError> {
    validate_player_id(from_player_id)?;

    let mut payload = Vec::with_capacity(5);
    payload.extend_from_slice(&MAP_TRANSFER_MARKER.to_le_bytes());
    payload.push(from_player_id);
    Ok(Frame::new(START_DOWNLOAD_PACKET_ID, payload)?)
}

pub fn map_part_frame(
    from_player_id: u8,
    to_player_id: u8,
    start: u32,
    data: &[u8],
) -> Result<Frame, MapTransferError> {
    validate_player_id(from_player_id)?;
    validate_player_id(to_player_id)?;
    if data.is_empty() || data.len() > MAP_PART_DATA_BYTES {
        return Err(MapTransferError::InvalidMapPartLength {
            actual: data.len(),
            maximum: MAP_PART_DATA_BYTES,
        });
    }
    start
        .checked_add(u32::try_from(data.len()).expect("map part length fits in u32"))
        .ok_or(MapTransferError::MapPartRangeOverflow)?;

    let mut crc32 = Crc32Hasher::new();
    crc32.update(data);

    let mut payload = Vec::with_capacity(data.len() + 14);
    payload.push(to_player_id);
    payload.push(from_player_id);
    payload.extend_from_slice(&MAP_TRANSFER_MARKER.to_le_bytes());
    payload.extend_from_slice(&start.to_le_bytes());
    payload.extend_from_slice(&crc32.finalize().to_le_bytes());
    payload.extend_from_slice(data);
    Ok(Frame::new(MAP_PART_PACKET_ID, payload)?)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MapPartAck {
    sender_player_id: u8,
    receiver_player_id: u8,
    map_size: u32,
}

impl MapPartAck {
    pub fn decode(frame: &Frame) -> Result<Self, MapTransferError> {
        if frame.packet_id() != MAP_PART_OK_PACKET_ID {
            return Err(MapTransferError::UnexpectedPacketId {
                actual: frame.packet_id(),
                expected: MAP_PART_OK_PACKET_ID,
            });
        }

        let payload = frame.payload();
        if payload.len() != MAP_PART_OK_PAYLOAD_LENGTH {
            return Err(MapTransferError::InvalidAckPayloadLength {
                actual: payload.len(),
                expected: MAP_PART_OK_PAYLOAD_LENGTH,
            });
        }

        let marker = u32::from_le_bytes([payload[2], payload[3], payload[4], payload[5]]);
        if marker != MAP_TRANSFER_MARKER {
            return Err(MapTransferError::InvalidMarker(marker));
        }

        validate_player_id(payload[0])?;
        validate_player_id(payload[1])?;
        Ok(Self {
            sender_player_id: payload[0],
            receiver_player_id: payload[1],
            map_size: u32::from_le_bytes([payload[6], payload[7], payload[8], payload[9]]),
        })
    }

    pub fn sender_player_id(self) -> u8 {
        self.sender_player_id
    }

    pub fn receiver_player_id(self) -> u8 {
        self.receiver_player_id
    }

    pub fn map_size(self) -> u32 {
        self.map_size
    }
}

fn validate_player_id(player_id: u8) -> Result<(), MapTransferError> {
    if player_id == 0 || player_id == u8::MAX {
        return Err(MapTransferError::InvalidPlayerId(player_id));
    }

    Ok(())
}

#[derive(Debug, Error)]
pub enum MapCheckError {
    #[error("W3GS map path must not be empty or contain a NUL byte")]
    InvalidFilePath,
    #[error(transparent)]
    Frame(#[from] FrameError),
}

#[derive(Debug, Error, Eq, PartialEq)]
pub enum MapSizeError {
    #[error("expected W3GS_MAPSIZE packet 0x42, got 0x{actual:02X}")]
    UnexpectedPacketId { actual: u8 },
    #[error("W3GS_MAPSIZE payload must contain {expected} bytes, got {actual}")]
    InvalidPayloadLength { actual: usize, expected: usize },
    #[error("W3GS_MAPSIZE marker must be 1, got {0}")]
    InvalidMarker(u32),
}

#[derive(Debug, Error)]
pub enum MapTransferError {
    #[error("W3GS player id must be in the range 1..=254, got {0}")]
    InvalidPlayerId(u8),
    #[error("W3GS map part must contain 1 to {maximum} bytes, got {actual}")]
    InvalidMapPartLength { actual: usize, maximum: usize },
    #[error("W3GS map part range exceeds the 4 GiB protocol limit")]
    MapPartRangeOverflow,
    #[error("expected W3GS packet 0x{expected:02X}, got 0x{actual:02X}")]
    UnexpectedPacketId { actual: u8, expected: u8 },
    #[error("W3GS_MAPPARTOK payload must contain {expected} bytes, got {actual}")]
    InvalidAckPayloadLength { actual: usize, expected: usize },
    #[error("W3GS map transfer marker must be 1, got {0}")]
    InvalidMarker(u32),
    #[error(transparent)]
    Frame(#[from] FrameError),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_map_check_in_the_confirmed_wire_order() {
        let map_check = MapCheck::new(
            "Maps\\Download\\DotA.w3x".to_owned(),
            0x0102_0304,
            0x0506_0708,
            0x090A_0B0C,
            [0xAA; 20],
        )
        .expect("map check should build");
        let frame = map_check.frame().expect("map check should encode");

        assert_eq!(frame.packet_id(), MAP_CHECK_PACKET_ID);
        assert!(frame.payload().starts_with(&[1, 0, 0, 0]));
        assert!(frame.payload()[4..].starts_with(b"Maps\\Download\\DotA.w3x\0"));
        assert!(frame.payload().ends_with(&[0xAA; 20]));
    }

    #[test]
    fn decodes_a_map_size_response() {
        let mut payload = Vec::new();
        payload.extend_from_slice(&1_u32.to_le_bytes());
        payload.push(1);
        payload.extend_from_slice(&35_053_979_u32.to_le_bytes());
        let frame = Frame::new(MAP_SIZE_PACKET_ID, payload).expect("frame should build");

        let map_size = MapSize::decode(&frame).expect("map size should decode");
        assert!(map_size.has_map());
        assert_eq!(map_size.map_size(), 35_053_979);
    }

    #[test]
    fn recognizes_a_map_download_progress_response() {
        let mut payload = Vec::new();
        payload.extend_from_slice(&1_u32.to_le_bytes());
        payload.push(3);
        payload.extend_from_slice(&144_200_u32.to_le_bytes());
        let frame = Frame::new(MAP_SIZE_PACKET_ID, payload).expect("frame should build");

        let map_size = MapSize::decode(&frame).expect("map size should decode");
        assert!(!map_size.has_map());
        assert!(map_size.continues_download());
        assert_eq!(map_size.size_flag(), 3);
        assert_eq!(map_size.map_size(), 144_200);
    }

    #[test]
    fn encodes_start_download_in_the_confirmed_wire_order() {
        let frame = start_download_frame(7).expect("frame should encode");

        assert_eq!(frame.packet_id(), START_DOWNLOAD_PACKET_ID);
        assert_eq!(frame.payload(), &[1, 0, 0, 0, 7]);
    }

    #[test]
    fn encodes_map_part_with_crc32_and_player_ids() {
        let frame = map_part_frame(1, 2, 1_442, b"123456789").expect("frame should encode");

        assert_eq!(frame.packet_id(), MAP_PART_PACKET_ID);
        assert_eq!(&frame.payload()[0..2], &[2, 1]);
        assert_eq!(&frame.payload()[2..6], &[1, 0, 0, 0]);
        assert_eq!(&frame.payload()[6..10], &1_442_u32.to_le_bytes());
        assert_eq!(&frame.payload()[10..14], &0xCBF4_3926_u32.to_le_bytes());
        assert_eq!(&frame.payload()[14..], b"123456789");
    }

    #[test]
    fn rejects_oversized_map_parts() {
        let data = vec![0_u8; MAP_PART_DATA_BYTES + 1];

        assert!(matches!(
            map_part_frame(1, 2, 0, &data),
            Err(MapTransferError::InvalidMapPartLength {
                actual,
                maximum: MAP_PART_DATA_BYTES,
            }) if actual == MAP_PART_DATA_BYTES + 1
        ));
    }

    #[test]
    fn decodes_map_part_acknowledgements() {
        let mut payload = vec![2, 1];
        payload.extend_from_slice(&1_u32.to_le_bytes());
        payload.extend_from_slice(&2_884_u32.to_le_bytes());
        let frame = Frame::new(MAP_PART_OK_PACKET_ID, payload).expect("frame should build");

        let ack = MapPartAck::decode(&frame).expect("ack should decode");
        assert_eq!(ack.sender_player_id(), 2);
        assert_eq!(ack.receiver_player_id(), 1);
        assert_eq!(ack.map_size(), 2_884);
    }
}
