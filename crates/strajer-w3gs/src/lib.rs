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
    CHAT_TO_HOST_PACKET_ID, LEAVE_ACK_PACKET_ID, LEAVE_REQUEST_PACKET_ID, PING_FROM_HOST_PACKET_ID,
    PONG_TO_HOST_PACKET_ID, leave_ack, ping_from_host,
};
pub use frame::{FRAME_HEADER_LENGTH, Frame, FrameError, FrameHeader, FrameReader, W3GS_SIGNATURE};
pub use map::{
    MAP_CHECK_PACKET_ID, MAP_SIZE_PACKET_ID, MapCheck, MapCheckError, MapSize, MapSizeError,
};
pub use player::{
    PLAYER_INFO_PACKET_ID, PLAYER_LEAVE_OTHERS_PACKET_ID, PlayerFrameError, player_info_frame,
    player_leave_others_frame,
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
