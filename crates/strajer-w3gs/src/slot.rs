use std::net::SocketAddrV4;

use thiserror::Error;

use crate::net::{WIRE_SOCKET_ADDRESS_LENGTH, encode_ipv4_socket_address};
use crate::{Frame, FrameError};

pub const SLOT_INFO_JOIN_PACKET_ID: u8 = 0x04;
pub const SLOT_INFO_PACKET_ID: u8 = 0x09;
pub const MAX_SLOT_COUNT: usize = 24;
const SLOT_DATA_LENGTH: usize = 9;
const SLOT_INFO_FIXED_LENGTH: usize = 7;

pub const RACE_HUMAN: u8 = 0x01;
pub const RACE_ORC: u8 = 0x02;
pub const RACE_NIGHT_ELF: u8 = 0x04;
pub const RACE_UNDEAD: u8 = 0x08;
pub const RACE_RANDOM: u8 = 0x20;
pub const RACE_SELECTABLE: u8 = 0x40;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum SlotStatus {
    Open = 0,
    Closed = 1,
    Occupied = 2,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum SlotLayout {
    Melee = 0,
    CustomForces = 1,
    FixedPlayerSettings = 2,
    CustomForcesFixedPlayerSettings = 3,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SlotData {
    player_id: u8,
    download_status: u8,
    status: SlotStatus,
    computer: bool,
    team: u8,
    color: u8,
    race: u8,
    computer_type: u8,
    handicap: u8,
}

impl SlotData {
    pub fn open(team: u8, color: u8, race: u8) -> Self {
        Self {
            player_id: 0,
            download_status: u8::MAX,
            status: SlotStatus::Open,
            computer: false,
            team,
            color,
            race,
            computer_type: 1,
            handicap: 100,
        }
    }

    pub fn occupied_human(player_id: u8, team: u8, color: u8, race: u8) -> Self {
        Self {
            player_id,
            download_status: 100,
            status: SlotStatus::Occupied,
            computer: false,
            team,
            color,
            race,
            computer_type: 1,
            handicap: 100,
        }
    }

    pub fn player_id(&self) -> u8 {
        self.player_id
    }

    pub fn status(&self) -> SlotStatus {
        self.status
    }

    fn encode_into(&self, buffer: &mut Vec<u8>) {
        buffer.push(self.player_id);
        buffer.push(self.download_status);
        buffer.push(self.status as u8);
        buffer.push(u8::from(self.computer));
        buffer.push(self.team);
        buffer.push(self.color);
        buffer.push(self.race);
        buffer.push(self.computer_type);
        buffer.push(self.handicap);
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SlotInfo {
    slots: Vec<SlotData>,
    random_seed: u32,
    layout: SlotLayout,
    player_slots: u8,
}

impl SlotInfo {
    pub fn new(
        slots: Vec<SlotData>,
        random_seed: u32,
        layout: SlotLayout,
        player_slots: u8,
    ) -> Result<Self, SlotInfoError> {
        if slots.is_empty() {
            return Err(SlotInfoError::EmptySlots);
        }

        if slots.len() > MAX_SLOT_COUNT {
            return Err(SlotInfoError::TooManySlots {
                actual: slots.len(),
                maximum: MAX_SLOT_COUNT,
            });
        }

        if usize::from(player_slots) > slots.len() {
            return Err(SlotInfoError::TooManyPlayerSlots {
                actual: player_slots,
                slot_count: slots.len(),
            });
        }

        Ok(Self {
            slots,
            random_seed,
            layout,
            player_slots,
        })
    }

    pub fn slots(&self) -> &[SlotData] {
        &self.slots
    }

    pub fn frame(&self) -> Result<Frame, SlotInfoError> {
        Ok(Frame::new(SLOT_INFO_PACKET_ID, self.encode_payload())?)
    }

    pub fn join_frame(
        &self,
        assigned_player_id: u8,
        external_address: SocketAddrV4,
    ) -> Result<Frame, SlotInfoError> {
        let assigned_slot_exists = self.slots.iter().any(|slot| {
            slot.player_id == assigned_player_id && slot.status == SlotStatus::Occupied
        });
        if assigned_player_id == 0 || !assigned_slot_exists {
            return Err(SlotInfoError::InvalidAssignedPlayerId(assigned_player_id));
        }

        let mut payload = self.encode_payload();
        payload.reserve(1 + WIRE_SOCKET_ADDRESS_LENGTH);
        payload.push(assigned_player_id);
        encode_ipv4_socket_address(&mut payload, external_address);
        Ok(Frame::new(SLOT_INFO_JOIN_PACKET_ID, payload)?)
    }

    fn encode_payload(&self) -> Vec<u8> {
        let slot_data_length = SLOT_INFO_FIXED_LENGTH + SLOT_DATA_LENGTH * self.slots.len();
        let mut payload = Vec::with_capacity(2 + slot_data_length);
        payload.extend_from_slice(&(slot_data_length as u16).to_le_bytes());
        payload.push(self.slots.len() as u8);
        for slot in &self.slots {
            slot.encode_into(&mut payload);
        }
        payload.extend_from_slice(&self.random_seed.to_le_bytes());
        payload.push(self.layout as u8);
        payload.push(self.player_slots);
        payload
    }
}

#[derive(Debug, Error)]
pub enum SlotInfoError {
    #[error("W3GS slot info must contain at least one slot")]
    EmptySlots,
    #[error("W3GS slot info contains {actual} slots; maximum is {maximum}")]
    TooManySlots { actual: usize, maximum: usize },
    #[error("W3GS player slot count {actual} exceeds total slot count {slot_count}")]
    TooManyPlayerSlots { actual: u8, slot_count: usize },
    #[error("assigned W3GS player id {0} does not identify an occupied human slot")]
    InvalidAssignedPlayerId(u8),
    #[error(transparent)]
    Frame(#[from] FrameError),
}

#[cfg(test)]
mod tests {
    use std::net::Ipv4Addr;

    use super::*;

    #[test]
    fn encodes_slot_info_and_slot_info_join() {
        let slot_info = two_slot_info();
        let slot_frame = slot_info.frame().expect("slot info should encode");

        assert_eq!(slot_frame.packet_id(), SLOT_INFO_PACKET_ID);
        assert_eq!(slot_frame.payload()[..3], [25, 0, 2]);
        assert_eq!(slot_frame.payload().len(), 27);
        assert_eq!(slot_frame.payload()[21..25], 0x1234_5678_u32.to_le_bytes());
        assert_eq!(slot_frame.payload()[25..], [3, 2]);

        let join_frame = slot_info
            .join_frame(1, SocketAddrV4::new(Ipv4Addr::new(127, 0, 0, 1), 6_112))
            .expect("slot info join should encode");
        assert_eq!(join_frame.packet_id(), SLOT_INFO_JOIN_PACKET_ID);
        assert_eq!(join_frame.encoded_length(), 48);
        assert_eq!(join_frame.payload()[27], 1);
        assert_eq!(
            &join_frame.payload()[28..],
            &[2, 0, 0xE0, 0x17, 127, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0]
        );
    }

    #[test]
    fn rejects_an_unoccupied_assignment() {
        let slot_info = two_slot_info();
        let error = slot_info
            .join_frame(2, SocketAddrV4::new(Ipv4Addr::LOCALHOST, 6_112))
            .expect_err("open slot must not be assigned");

        assert!(matches!(error, SlotInfoError::InvalidAssignedPlayerId(2)));
    }

    fn two_slot_info() -> SlotInfo {
        SlotInfo::new(
            vec![
                SlotData::occupied_human(1, 0, 1, RACE_NIGHT_ELF),
                SlotData::open(1, 7, RACE_UNDEAD),
            ],
            0x1234_5678,
            SlotLayout::CustomForcesFixedPlayerSettings,
            2,
        )
        .expect("fixture should be valid")
    }
}
