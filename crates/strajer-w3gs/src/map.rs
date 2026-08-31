use thiserror::Error;

use crate::{Frame, FrameError};

pub const MAP_CHECK_PACKET_ID: u8 = 0x3D;
pub const MAP_SIZE_PACKET_ID: u8 = 0x42;
const MAP_SIZE_PAYLOAD_LENGTH: usize = 9;

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

    pub fn map_size(self) -> u32 {
        self.map_size
    }
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
}
