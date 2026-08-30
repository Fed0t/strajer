use std::io;

use thiserror::Error;
use tokio::io::{AsyncRead, AsyncReadExt};

pub const W3GS_SIGNATURE: u8 = 0xF7;
pub const FRAME_HEADER_LENGTH: usize = 4;
const MAX_WIRE_FRAME_LENGTH: usize = u16::MAX as usize;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FrameHeader {
    packet_id: u8,
    frame_length: u16,
}

impl FrameHeader {
    pub fn decode(bytes: [u8; FRAME_HEADER_LENGTH]) -> Result<Self, FrameError> {
        if bytes[0] != W3GS_SIGNATURE {
            return Err(FrameError::InvalidSignature { actual: bytes[0] });
        }

        let frame_length = u16::from_le_bytes([bytes[2], bytes[3]]);
        if usize::from(frame_length) < FRAME_HEADER_LENGTH {
            return Err(FrameError::InvalidFrameLength {
                actual: usize::from(frame_length),
            });
        }

        Ok(Self {
            packet_id: bytes[1],
            frame_length,
        })
    }

    pub fn packet_id(self) -> u8 {
        self.packet_id
    }

    pub fn frame_length(self) -> usize {
        usize::from(self.frame_length)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Frame {
    header: FrameHeader,
    payload: Vec<u8>,
}

impl Frame {
    pub fn new(packet_id: u8, payload: Vec<u8>) -> Result<Self, FrameError> {
        let frame_length =
            FRAME_HEADER_LENGTH
                .checked_add(payload.len())
                .ok_or(FrameError::FrameTooLarge {
                    actual: usize::MAX,
                    maximum: MAX_WIRE_FRAME_LENGTH,
                })?;

        if frame_length > MAX_WIRE_FRAME_LENGTH {
            return Err(FrameError::FrameTooLarge {
                actual: frame_length,
                maximum: MAX_WIRE_FRAME_LENGTH,
            });
        }

        Ok(Self {
            header: FrameHeader {
                packet_id,
                frame_length: frame_length as u16,
            },
            payload,
        })
    }

    pub fn decode_exact(bytes: &[u8], maximum_frame_length: usize) -> Result<Self, FrameError> {
        validate_maximum_frame_length(maximum_frame_length)?;

        if bytes.len() < FRAME_HEADER_LENGTH {
            return Err(FrameError::TruncatedHeader {
                actual: bytes.len(),
            });
        }

        let header = FrameHeader::decode([bytes[0], bytes[1], bytes[2], bytes[3]])?;
        validate_declared_frame_length(header.frame_length(), maximum_frame_length)?;

        if bytes.len() != header.frame_length() {
            return Err(FrameError::FrameLengthMismatch {
                declared: header.frame_length(),
                actual: bytes.len(),
            });
        }

        Ok(Self {
            header,
            payload: bytes[FRAME_HEADER_LENGTH..].to_vec(),
        })
    }

    pub fn header(&self) -> FrameHeader {
        self.header
    }

    pub fn packet_id(&self) -> u8 {
        self.header.packet_id()
    }

    pub fn encoded_length(&self) -> usize {
        self.header.frame_length()
    }

    pub fn payload(&self) -> &[u8] {
        &self.payload
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(self.encoded_length());
        bytes.push(W3GS_SIGNATURE);
        bytes.push(self.packet_id());
        bytes.extend_from_slice(&self.header.frame_length.to_le_bytes());
        bytes.extend_from_slice(&self.payload);
        bytes
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FrameReader {
    maximum_frame_length: usize,
}

impl FrameReader {
    pub fn new(maximum_frame_length: usize) -> Result<Self, FrameError> {
        validate_maximum_frame_length(maximum_frame_length)?;
        Ok(Self {
            maximum_frame_length,
        })
    }

    pub async fn read_next<R>(&self, source: &mut R) -> Result<Option<Frame>, FrameError>
    where
        R: AsyncRead + Unpin,
    {
        let mut header_bytes = [0_u8; FRAME_HEADER_LENGTH];
        let first_byte_count = source.read(&mut header_bytes[..1]).await?;
        if first_byte_count == 0 {
            return Ok(None);
        }

        source.read_exact(&mut header_bytes[1..]).await?;
        let header = FrameHeader::decode(header_bytes)?;
        validate_declared_frame_length(header.frame_length(), self.maximum_frame_length)?;

        let payload_length = header.frame_length() - FRAME_HEADER_LENGTH;
        let mut payload = vec![0_u8; payload_length];
        source.read_exact(&mut payload).await?;

        Ok(Some(Frame { header, payload }))
    }
}

#[derive(Debug, Error)]
pub enum FrameError {
    #[error("W3GS signature must be 0xF7, got 0x{actual:02X}")]
    InvalidSignature { actual: u8 },
    #[error("W3GS frame length must be at least {FRAME_HEADER_LENGTH}, got {actual}")]
    InvalidFrameLength { actual: usize },
    #[error(
        "maximum W3GS frame length must be between {FRAME_HEADER_LENGTH} and {MAX_WIRE_FRAME_LENGTH}, got {actual}"
    )]
    InvalidMaximumFrameLength { actual: usize },
    #[error("W3GS frame length {actual} exceeds configured maximum {maximum}")]
    FrameTooLarge { actual: usize, maximum: usize },
    #[error("W3GS frame header is truncated: expected 4 bytes, got {actual}")]
    TruncatedHeader { actual: usize },
    #[error("W3GS frame length mismatch: header declares {declared}, buffer contains {actual}")]
    FrameLengthMismatch { declared: usize, actual: usize },
    #[error("I/O error while reading W3GS frame: {0}")]
    Io(#[from] io::Error),
}

fn validate_maximum_frame_length(maximum_frame_length: usize) -> Result<(), FrameError> {
    if !(FRAME_HEADER_LENGTH..=MAX_WIRE_FRAME_LENGTH).contains(&maximum_frame_length) {
        return Err(FrameError::InvalidMaximumFrameLength {
            actual: maximum_frame_length,
        });
    }

    Ok(())
}

fn validate_declared_frame_length(
    frame_length: usize,
    maximum_frame_length: usize,
) -> Result<(), FrameError> {
    if frame_length > maximum_frame_length {
        return Err(FrameError::FrameTooLarge {
            actual: frame_length,
            maximum: maximum_frame_length,
        });
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use tokio::io::{AsyncWriteExt, duplex};

    use super::*;

    #[test]
    fn decodes_and_encodes_an_exact_frame() {
        let bytes = [W3GS_SIGNATURE, 0x1E, 0x08, 0x00, 1, 2, 3, 4];
        let frame = Frame::decode_exact(&bytes, 64).expect("frame should decode");

        assert_eq!(frame.packet_id(), 0x1E);
        assert_eq!(frame.encoded_length(), 8);
        assert_eq!(frame.payload(), &[1, 2, 3, 4]);
        assert_eq!(frame.to_bytes(), bytes);
    }

    #[test]
    fn rejects_an_invalid_signature() {
        let error = Frame::decode_exact(&[0x00, 0x1E, 0x04, 0x00], 64)
            .expect_err("signature should be rejected");

        assert!(matches!(
            error,
            FrameError::InvalidSignature { actual: 0x00 }
        ));
    }

    #[test]
    fn rejects_a_length_smaller_than_the_header() {
        let error = Frame::decode_exact(&[W3GS_SIGNATURE, 0x1E, 0x03, 0x00], 64)
            .expect_err("short declared length should be rejected");

        assert!(matches!(
            error,
            FrameError::InvalidFrameLength { actual: 3 }
        ));
    }

    #[test]
    fn rejects_a_mismatched_buffer_length() {
        let error = Frame::decode_exact(&[W3GS_SIGNATURE, 0x1E, 0x08, 0x00, 1], 64)
            .expect_err("truncated buffer should be rejected");

        assert!(matches!(
            error,
            FrameError::FrameLengthMismatch {
                declared: 8,
                actual: 5
            }
        ));
    }

    #[test]
    fn rejects_a_frame_above_the_configured_limit() {
        let error = Frame::decode_exact(&[W3GS_SIGNATURE, 0x1E, 0x08, 0x00, 1, 2, 3, 4], 7)
            .expect_err("oversized frame should be rejected");

        assert!(matches!(
            error,
            FrameError::FrameTooLarge {
                actual: 8,
                maximum: 7
            }
        ));
    }

    #[tokio::test]
    async fn reads_fragmented_input_without_over_reading() {
        let expected = Frame::new(0x1E, vec![1, 2, 3, 4, 5]).expect("frame should build");
        let encoded = expected.to_bytes();
        let (mut writer, mut reader_stream) = duplex(1);

        let writer_task = tokio::spawn(async move {
            for byte in encoded {
                writer
                    .write_all(&[byte])
                    .await
                    .expect("fragment should write");
            }
        });

        let reader = FrameReader::new(64).expect("limit should be valid");
        let actual = reader
            .read_next(&mut reader_stream)
            .await
            .expect("frame should read");

        writer_task.await.expect("writer task should finish");
        assert_eq!(actual, Some(expected));
    }

    #[tokio::test]
    async fn reads_coalesced_frames_one_at_a_time() {
        let first = Frame::new(0x1E, vec![1, 2]).expect("first frame should build");
        let second = Frame::new(0x46, vec![3, 4, 5, 6]).expect("second frame should build");
        let mut bytes = first.to_bytes();
        bytes.extend_from_slice(&second.to_bytes());
        let mut input = bytes.as_slice();
        let reader = FrameReader::new(64).expect("limit should be valid");

        assert_eq!(
            reader.read_next(&mut input).await.expect("first read"),
            Some(first)
        );
        assert_eq!(
            reader.read_next(&mut input).await.expect("second read"),
            Some(second)
        );
        assert_eq!(reader.read_next(&mut input).await.expect("EOF read"), None);
    }

    #[tokio::test]
    async fn reports_a_truncated_async_frame() {
        let mut input = [W3GS_SIGNATURE, 0x1E, 0x08, 0x00, 1].as_slice();
        let reader = FrameReader::new(64).expect("limit should be valid");
        let error = reader
            .read_next(&mut input)
            .await
            .expect_err("truncated frame should fail");

        assert!(matches!(
            error,
            FrameError::Io(ref source) if source.kind() == io::ErrorKind::UnexpectedEof
        ));
    }
}
