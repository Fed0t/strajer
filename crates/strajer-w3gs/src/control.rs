use crate::{Frame, FrameError};

pub const PING_FROM_HOST_PACKET_ID: u8 = 0x01;
pub const LEAVE_ACK_PACKET_ID: u8 = 0x1B;
pub const LEAVE_REQUEST_PACKET_ID: u8 = 0x21;
pub const CHAT_TO_HOST_PACKET_ID: u8 = 0x28;
pub const PONG_TO_HOST_PACKET_ID: u8 = 0x46;

pub fn ping_from_host(payload: u32) -> Result<Frame, FrameError> {
    Frame::new(PING_FROM_HOST_PACKET_ID, payload.to_le_bytes().to_vec())
}

pub fn leave_ack() -> Result<Frame, FrameError> {
    Frame::new(LEAVE_ACK_PACKET_ID, Vec::new())
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
    }
}
