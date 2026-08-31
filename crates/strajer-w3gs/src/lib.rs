#![forbid(unsafe_code)]

mod control;
mod frame;
mod map;
mod net;
mod player;
mod protobuf;
mod req_join;
mod slot;

pub use control::{
    CHAT_FROM_HOST_PACKET_ID, CHAT_TO_HOST_PACKET_ID, COUNTDOWN_END_PACKET_ID,
    COUNTDOWN_START_PACKET_ID, ControlFrameError, LEAVE_ACK_PACKET_ID, LEAVE_REQUEST_PACKET_ID,
    LobbyChatToHost, MAX_CHAT_MESSAGE_BYTES, PING_FROM_HOST_PACKET_ID, PONG_TO_HOST_PACKET_ID,
    chat_from_host, countdown_end, countdown_start, leave_ack, ping_from_host,
};
pub use frame::{FRAME_HEADER_LENGTH, Frame, FrameError, FrameHeader, FrameReader, W3GS_SIGNATURE};
pub use map::{
    MAP_CHECK_PACKET_ID, MAP_PART_DATA_BYTES, MAP_PART_NOT_OK_PACKET_ID, MAP_PART_OK_PACKET_ID,
    MAP_PART_PACKET_ID, MAP_SIZE_PACKET_ID, MapCheck, MapCheckError, MapPartAck, MapSize,
    MapSizeError, MapTransferError, START_DOWNLOAD_PACKET_ID, map_part_frame, start_download_frame,
};
pub use player::{
    GAME_LOADED_OTHERS_PACKET_ID, PLAYER_INFO_PACKET_ID, PLAYER_LEAVE_OTHERS_PACKET_ID,
    PlayerFrameError, game_loaded_others_frame, player_info_frame, player_leave_others_frame,
};
pub use protobuf::{
    PLAYER_PROFILE_MESSAGE_TYPE, PLAYER_SKINS_MESSAGE_TYPE, PLAYER_UNKNOWN_5_MESSAGE_TYPE,
    PROTOBUF_PACKET_ID, PlayerProfileMessage, PlayerProfileRealm, ProtobufEnvelope, ProtobufError,
    player_profile_frame, player_skins_frame,
};
pub use req_join::{MAX_PLAYER_NAME_BYTES, REQ_JOIN_PACKET_ID, ReqJoin, ReqJoinError};
pub use slot::{
    MAX_SLOT_COUNT, RACE_HUMAN, RACE_NIGHT_ELF, RACE_ORC, RACE_RANDOM, RACE_SELECTABLE,
    RACE_UNDEAD, SLOT_INFO_JOIN_PACKET_ID, SLOT_INFO_PACKET_ID, SlotData, SlotInfo, SlotInfoError,
    SlotLayout, SlotStatus,
};
