use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use strajer_protocol::{LobbyCatalog, LobbyPlayer, LobbyRoster};
use thiserror::Error;
use tokio::sync::{Mutex, broadcast};

const ROSTER_UPDATE_CAPACITY: usize = 32;

#[derive(Clone)]
pub(crate) struct LobbyRegistry {
    rooms: Arc<HashMap<String, Arc<LobbyRoom>>>,
}

impl LobbyRegistry {
    pub(crate) fn from_catalog(catalog: &LobbyCatalog) -> Self {
        let rooms = catalog
            .lobbies
            .iter()
            .map(|lobby| {
                (
                    lobby.id.clone(),
                    Arc::new(LobbyRoom::new(lobby.revision, lobby.players.max)),
                )
            })
            .collect();

        Self {
            rooms: Arc::new(rooms),
        }
    }

    pub(crate) fn room(&self, lobby_id: &str) -> Option<Arc<LobbyRoom>> {
        self.rooms.get(lobby_id).cloned()
    }
}

pub(crate) struct LobbyRoom {
    maximum_players: u8,
    state: Mutex<LobbyRoomState>,
    updates: broadcast::Sender<LobbyRoster>,
}

impl LobbyRoom {
    fn new(initial_revision: u64, maximum_players: u8) -> Self {
        let (updates, _) = broadcast::channel(ROSTER_UPDATE_CAPACITY);
        Self {
            maximum_players,
            state: Mutex::new(LobbyRoomState {
                revision: initial_revision,
                next_session_id: 1,
                players: BTreeMap::new(),
            }),
            updates,
        }
    }

    pub(crate) async fn join(
        &self,
        player_name: String,
    ) -> Result<LobbyMembership, LobbyJoinError> {
        let updates = self.updates.subscribe();
        let mut state = self.state.lock().await;

        if state.players.len() >= usize::from(self.maximum_players) {
            return Err(LobbyJoinError::Full);
        }

        if state
            .players
            .values()
            .any(|player| player.name == player_name)
        {
            return Err(LobbyJoinError::DuplicatePlayerName);
        }

        let player_id = (1..=self.maximum_players)
            .find(|candidate| !state.players.contains_key(candidate))
            .ok_or(LobbyJoinError::Full)?;
        let session_id = state.next_session_id;
        state.next_session_id = state
            .next_session_id
            .checked_add(1)
            .ok_or(LobbyJoinError::SessionIdExhausted)?;
        state.players.insert(
            player_id,
            ConnectedPlayer {
                session_id,
                name: player_name,
            },
        );
        state.revision = next_revision(state.revision);
        let roster = state.roster();
        drop(state);

        let _ = self.updates.send(roster.clone());
        Ok(LobbyMembership {
            player_id,
            session_id,
            roster,
            updates,
        })
    }

    pub(crate) async fn leave(&self, player_id: u8, session_id: u64) {
        let mut state = self.state.lock().await;
        let session_matches = state
            .players
            .get(&player_id)
            .is_some_and(|player| player.session_id == session_id);
        if !session_matches {
            return;
        }

        state.players.remove(&player_id);
        state.revision = next_revision(state.revision);
        let roster = state.roster();
        drop(state);
        let _ = self.updates.send(roster);
    }

    pub(crate) async fn snapshot(&self) -> LobbyRoster {
        self.state.lock().await.roster()
    }
}

pub(crate) struct LobbyMembership {
    pub(crate) player_id: u8,
    pub(crate) session_id: u64,
    pub(crate) roster: LobbyRoster,
    pub(crate) updates: broadcast::Receiver<LobbyRoster>,
}

#[derive(Debug, Error, Eq, PartialEq)]
pub(crate) enum LobbyJoinError {
    #[error("lobby is full")]
    Full,
    #[error("player name is already present in the lobby")]
    DuplicatePlayerName,
    #[error("lobby session id space is exhausted")]
    SessionIdExhausted,
}

struct LobbyRoomState {
    revision: u64,
    next_session_id: u64,
    players: BTreeMap<u8, ConnectedPlayer>,
}

impl LobbyRoomState {
    fn roster(&self) -> LobbyRoster {
        let players = self
            .players
            .iter()
            .map(|(&player_id, player)| LobbyPlayer {
                player_id,
                slot_index: player_id - 1,
                name: player.name.clone(),
            })
            .collect();

        LobbyRoster {
            revision: self.revision,
            players,
        }
    }
}

struct ConnectedPlayer {
    session_id: u64,
    name: String,
}

fn next_revision(current: u64) -> u64 {
    current.saturating_add(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn allocates_and_releases_two_distinct_players() {
        let room = LobbyRoom::new(1, 2);
        let first = room
            .join("First#1000".to_owned())
            .await
            .expect("first player should join");
        let second = room
            .join("Second#2000".to_owned())
            .await
            .expect("second player should join");

        assert_eq!(first.player_id, 1);
        assert_eq!(second.player_id, 2);
        assert_eq!(second.roster.players.len(), 2);
        assert_eq!(
            room.join("Third#3000".to_owned()).await.err(),
            Some(LobbyJoinError::Full)
        );

        room.leave(second.player_id, second.session_id).await;
        let roster = room.snapshot().await;
        assert_eq!(roster.players.len(), 1);
        assert_eq!(roster.players[0].player_id, 1);
    }

    #[tokio::test]
    async fn stale_disconnect_cannot_remove_a_reused_player_id() {
        let room = LobbyRoom::new(1, 1);
        let first = room
            .join("First#1000".to_owned())
            .await
            .expect("first player should join");
        room.leave(first.player_id, first.session_id).await;
        let replacement = room
            .join("Next#2000".to_owned())
            .await
            .expect("replacement should join");

        room.leave(first.player_id, first.session_id).await;
        assert_eq!(room.snapshot().await.players, replacement.roster.players);
    }
}
