use std::net::SocketAddrV4;

pub(crate) const WIRE_SOCKET_ADDRESS_LENGTH: usize = 16;

pub(crate) fn encode_ipv4_socket_address(buffer: &mut Vec<u8>, address: SocketAddrV4) {
    buffer.extend_from_slice(&2_u16.to_le_bytes());
    buffer.extend_from_slice(&address.port().to_le_bytes());
    buffer.extend_from_slice(&address.ip().octets());
    buffer.extend_from_slice(&[0_u8; 8]);
}

#[cfg(test)]
mod tests {
    use std::net::Ipv4Addr;

    use super::*;

    #[test]
    fn encodes_the_w3gs_ipv4_socket_layout() {
        let mut bytes = Vec::new();
        encode_ipv4_socket_address(
            &mut bytes,
            SocketAddrV4::new(Ipv4Addr::new(192, 168, 1, 6), 0x1234),
        );

        assert_eq!(
            bytes,
            [2, 0, 0x34, 0x12, 192, 168, 1, 6, 0, 0, 0, 0, 0, 0, 0, 0,]
        );
    }
}
