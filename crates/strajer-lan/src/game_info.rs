use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use prost::Message;
use strajer_protocol::LobbyDescriptor;

use crate::LanError;

const DEFAULT_GAME_SETTING_FLAGS: u32 = 0x0006_7802;
const GAME_FLAGS_OBSERVERS_FULL: u32 = 0x0010_0000;

#[derive(Clone, PartialEq, Message)]
struct GameInfoEntry {
    #[prost(string, tag = "1")]
    key: String,
    #[prost(string, tag = "2")]
    value: String,
}

#[derive(Clone, PartialEq, Message)]
struct GameInfo {
    #[prost(string, tag = "1")]
    name: String,
    #[prost(int32, tag = "2")]
    message_id: i32,
    #[prost(message, repeated, tag = "3")]
    entries: Vec<GameInfoEntry>,
}

pub fn encode_game_info_record(
    lobby: &LobbyDescriptor,
    local_port: u16,
) -> Result<Vec<u8>, LanError> {
    lobby.validate()?;

    let game_data = encode_game_data(lobby, local_port)?;
    let entries = vec![
        entry("players_num", lobby.players.current),
        entry("_name", &lobby.name),
        entry("players_max", lobby.players.max),
        entry("game_create_time", lobby.created_at_unix_seconds),
        entry("_type", 1),
        entry("_subtype", 0),
        entry("game_secret", lobby.game_secret),
        entry("game_data", STANDARD.encode(game_data)),
        entry("game_id", lobby.lan_game_id),
        entry("_flags", 0),
    ];

    let message = GameInfo {
        name: lobby.name.clone(),
        message_id: lobby.revision as i32,
        entries,
    };

    Ok(message.encode_to_vec())
}

fn entry(key: &str, value: impl ToString) -> GameInfoEntry {
    GameInfoEntry {
        key: key.to_owned(),
        value: value.to_string(),
    }
}

fn encode_game_data(lobby: &LobbyDescriptor, local_port: u16) -> Result<Vec<u8>, LanError> {
    let settings = encode_game_settings(lobby)?;
    let encoded_settings = encode_stat_string(&settings);
    let mut output = Vec::with_capacity(
        lobby.name.len() + encoded_settings.len() + std::mem::size_of::<u32>() * 2 + 5,
    );

    push_c_string(&mut output, &lobby.name);
    output.push(0);
    output.extend_from_slice(&encoded_settings);
    output.push(0);
    output.extend_from_slice(&u32::from(lobby.players.max).to_le_bytes());
    output.extend_from_slice(&GAME_FLAGS_OBSERVERS_FULL.to_le_bytes());
    output.extend_from_slice(&local_port.to_le_bytes());

    Ok(output)
}

fn encode_game_settings(lobby: &LobbyDescriptor) -> Result<Vec<u8>, LanError> {
    let map_sha1 = lobby.map.sha1_bytes()?;
    let map_path = lobby.map.path.replace('\\', "/");
    let mut settings = Vec::with_capacity(map_path.len() + lobby.virtual_host.name.len() + 36);

    settings.extend_from_slice(&DEFAULT_GAME_SETTING_FLAGS.to_le_bytes());
    settings.push(0);
    settings.extend_from_slice(&lobby.map.width.to_le_bytes());
    settings.extend_from_slice(&lobby.map.height.to_le_bytes());
    settings.extend_from_slice(&lobby.map.checksum.to_le_bytes());
    push_c_string(&mut settings, &map_path);
    push_c_string(&mut settings, &lobby.virtual_host.name);
    settings.push(0);
    settings.extend_from_slice(&map_sha1);

    Ok(settings)
}

fn push_c_string(buffer: &mut Vec<u8>, value: &str) {
    buffer.extend_from_slice(value.as_bytes());
    buffer.push(0);
}

fn encode_stat_string(source: &[u8]) -> Vec<u8> {
    let control_bytes = source.len().div_ceil(7);
    let mut output = Vec::with_capacity(source.len() + control_bytes);

    for chunk in source.chunks(7) {
        let control_position = output.len();
        output.push(1);
        let mut control = 1_u8;

        for (index, byte) in chunk.iter().copied().enumerate() {
            if byte.is_multiple_of(2) {
                output.push(byte.wrapping_add(1));
            } else {
                output.push(byte);
                control |= 1 << (index + 1);
            }
        }

        output[control_position] = control;
    }

    output
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use strajer_protocol::{
        DEFAULT_WARCRAFT_PRODUCT, DEFAULT_WARCRAFT_VERSION, LobbyDescriptor, LobbyPlayer,
        MapDescriptor, PlayerCount, WarcraftDescriptor,
    };

    use super::*;

    #[test]
    fn encodes_even_bytes_without_embedded_nuls() {
        assert_eq!(encode_stat_string(&[0]), vec![1, 1]);
        assert_eq!(encode_stat_string(&[1, 2, 3]), vec![0b0000_1011, 1, 3, 3]);
    }

    #[test]
    fn encodes_the_reforged_game_info_record() {
        let lobby = test_lobby();
        let bytes = encode_game_info_record(&lobby, 16_000).expect("record should encode");
        let decoded = GameInfo::decode(bytes.as_slice()).expect("protobuf should decode");
        let entries = decoded
            .entries
            .into_iter()
            .map(|item| (item.key, item.value))
            .collect::<HashMap<String, String>>();

        assert_eq!(decoded.name, "Strajer Test #1");
        assert_eq!(decoded.message_id, 1);
        assert_eq!(entries.get("players_num").map(String::as_str), Some("1"));
        assert_eq!(entries.get("players_max").map(String::as_str), Some("24"));
        assert_eq!(entries.get("game_id").map(String::as_str), Some("17"));

        let game_data = STANDARD
            .decode(entries.get("game_data").expect("game_data should exist"))
            .expect("game_data should be base64");
        assert!(game_data.starts_with(b"Strajer Test #1\0\0"));
        assert_eq!(&game_data[game_data.len() - 2..], &16_000_u16.to_le_bytes());
        assert!(!game_data["Strajer Test #1\0\0".len()..game_data.len() - 11].contains(&0));

        let encoded_settings_start = "Strajer Test #1\0\0".len();
        let encoded_settings_end = game_data[encoded_settings_start..]
            .iter()
            .position(|byte| *byte == 0)
            .expect("encoded settings should be null terminated")
            + encoded_settings_start;
        let settings =
            decode_stat_string_for_test(&game_data[encoded_settings_start..encoded_settings_end]);
        assert!(
            String::from_utf8_lossy(&settings).contains("Maps/Strajer/Synthetic.w3x"),
            "Bonjour map paths must use Warcraft's forward-slash wire format"
        );
    }

    fn decode_stat_string_for_test(source: &[u8]) -> Vec<u8> {
        let mut output = Vec::with_capacity(source.len());
        let mut position = 0;

        while position < source.len() {
            let control = source[position];
            position += 1;

            for bit in 1..=7 {
                if position == source.len() {
                    break;
                }

                let byte = source[position];
                position += 1;
                if control & (1 << bit) == 0 {
                    output.push(byte.wrapping_sub(1));
                } else {
                    output.push(byte);
                }
            }
        }

        output
    }

    fn test_lobby() -> LobbyDescriptor {
        LobbyDescriptor {
            id: "synthetic-1".to_owned(),
            revision: 1,
            lan_game_id: 17,
            game_secret: 23,
            name: "Strajer Test #1".to_owned(),
            created_at_unix_seconds: 1_000,
            warcraft: WarcraftDescriptor {
                version: DEFAULT_WARCRAFT_VERSION.to_owned(),
                product: DEFAULT_WARCRAFT_PRODUCT.to_owned(),
            },
            map: MapDescriptor {
                path: "Maps\\Strajer\\Synthetic.w3x".to_owned(),
                file_size: 1,
                file_crc32: 0,
                sha1_hex: "00".repeat(20),
                checksum: u32::MAX,
                width: 0,
                height: 0,
            },
            players: PlayerCount {
                current: 1,
                max: 24,
            },
            virtual_host: LobbyPlayer {
                player_id: 24,
                slot_index: 23,
                name: "Strajer".to_owned(),
            },
        }
    }
}
