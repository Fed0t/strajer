use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use std::time::Duration;

use strajer_protocol::{
    LOBBY_COUNTDOWN_SECONDS, LOBBY_COUNTDOWN_STEP_SECONDS, LobbyCatalog, LobbyPlayer, LobbyRoster,
};
use thiserror::Error;
use tokio::sync::{Mutex, broadcast};
use tokio::time::sleep;

const LOBBY_UPDATE_CAPACITY: usize = 64;
const MINIMUM_MANUAL_START_PLAYERS: usize = 2;

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
                    Arc::new(LobbyRoom::new(
                        lobby.revision,
                        lobby.human_player_capacity(),
                    )),
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
    countdown_policy: CountdownPolicy,
    state: Mutex<LobbyRoomState>,
    updates: broadcast::Sender<LobbyUpdate>,
}

impl LobbyRoom {
    fn new(initial_revision: u64, maximum_players: u8) -> Self {
        Self::new_with_countdown_policy(
            initial_revision,
            maximum_players,
            CountdownPolicy::default(),
        )
    }

    fn new_with_countdown_policy(
        initial_revision: u64,
        maximum_players: u8,
        countdown_policy: CountdownPolicy,
    ) -> Self {
        debug_assert!(countdown_policy.initial_seconds > 0);
        debug_assert!(countdown_policy.step_seconds > 0);
        debug_assert!(
            countdown_policy
                .initial_seconds
                .is_multiple_of(countdown_policy.step_seconds)
        );

        let (updates, _) = broadcast::channel(LOBBY_UPDATE_CAPACITY);
        Self {
            maximum_players,
            countdown_policy,
            state: Mutex::new(LobbyRoomState {
                revision: initial_revision,
                next_session_id: 1,
                players: BTreeMap::new(),
                countdown_generation: 0,
                countdown_active: false,
                countdown_remaining_seconds: None,
                manual_start_requested: false,
                started: false,
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

        if state.started {
            return Err(LobbyJoinError::Started);
        }
        if state.countdown_active {
            return Err(LobbyJoinError::CountdownActive);
        }

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
                ready: false,
                loaded: false,
            },
        );
        state.revision = next_revision(state.revision);
        let roster = state.roster();
        drop(state);

        let _ = self.updates.send(LobbyUpdate::Roster(roster.clone()));
        Ok(LobbyMembership {
            player_id,
            session_id,
            roster,
            updates,
        })
    }

    pub(crate) async fn mark_ready(
        self: &Arc<Self>,
        player_id: u8,
        session_id: u64,
    ) -> Result<(), LobbyMembershipError> {
        let countdown_generation = {
            let mut state = self.state.lock().await;
            let player = state
                .players
                .get_mut(&player_id)
                .filter(|player| player.session_id == session_id)
                .ok_or(LobbyMembershipError::UnknownSession)?;

            if player.ready {
                return Ok(());
            }
            player.ready = true;

            if !state.can_start_automatic_countdown(self.maximum_players)
                && !state.can_start_requested_countdown()
            {
                return Ok(());
            }

            self.begin_countdown(&mut state)?
        };

        self.spawn_countdown(countdown_generation);
        Ok(())
    }

    pub(crate) async fn mark_loaded(
        &self,
        player_id: u8,
        session_id: u64,
    ) -> Result<(), LobbyMembershipError> {
        let mut state = self.state.lock().await;
        state.require_active_session(player_id, session_id)?;
        if !state.started {
            return Err(LobbyMembershipError::GameNotStarted);
        }

        let player = state
            .players
            .get_mut(&player_id)
            .ok_or(LobbyMembershipError::UnknownSession)?;
        if player.loaded {
            return Ok(());
        }

        player.loaded = true;
        let _ = self.updates.send(LobbyUpdate::PlayerLoaded { player_id });
        Ok(())
    }

    pub(crate) async fn publish_chat(
        &self,
        player_id: u8,
        session_id: u64,
        message: String,
    ) -> Result<(), LobbyMembershipError> {
        let state = self.state.lock().await;
        state.require_active_session(player_id, session_id)?;
        let _ = self.updates.send(LobbyUpdate::Chat {
            from_player_id: player_id,
            message,
        });
        drop(state);
        Ok(())
    }

    pub(crate) async fn request_manual_start(
        self: &Arc<Self>,
        player_id: u8,
        session_id: u64,
    ) -> Result<ManualStartOutcome, LobbyMembershipError> {
        let (outcome, countdown_generation) = {
            let mut state = self.state.lock().await;
            state.require_active_session(player_id, session_id)?;

            if state.started {
                (ManualStartOutcome::AlreadyStarted, None)
            } else if state.countdown_active {
                (ManualStartOutcome::AlreadyCountingDown, None)
            } else if state.players.len() < MINIMUM_MANUAL_START_PLAYERS {
                (ManualStartOutcome::NotEnoughPlayers, None)
            } else if !state.players.values().all(|player| player.ready) {
                state.manual_start_requested = true;
                (ManualStartOutcome::PlayersNotReady, None)
            } else {
                let generation = self.begin_countdown(&mut state)?;
                (ManualStartOutcome::Started, Some(generation))
            }
        };

        if let Some(generation) = countdown_generation {
            self.spawn_countdown(generation);
        }

        Ok(outcome)
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
        let countdown_was_active = state.countdown_active;
        state.countdown_active = false;
        state.countdown_remaining_seconds = None;
        state.manual_start_requested = false;
        let _ = self.updates.send(LobbyUpdate::Roster(roster));
        if countdown_was_active {
            let _ = self.updates.send(LobbyUpdate::CountdownCancelled);
        }
    }

    #[cfg(test)]
    pub(crate) async fn snapshot(&self) -> LobbyRoster {
        self.state.lock().await.roster()
    }

    pub(crate) async fn control_snapshot(&self) -> LobbyControlSnapshot {
        let state = self.state.lock().await;
        let phase = if state.started {
            LobbyPhase::Started
        } else if let Some(remaining_seconds) = state.countdown_remaining_seconds {
            LobbyPhase::Countdown { remaining_seconds }
        } else {
            LobbyPhase::Waiting
        };

        LobbyControlSnapshot {
            roster: state.roster(),
            phase,
            loaded_player_ids: state.loaded_player_ids(),
        }
    }

    fn begin_countdown(&self, state: &mut LobbyRoomState) -> Result<u64, LobbyMembershipError> {
        state.countdown_generation = state
            .countdown_generation
            .checked_add(1)
            .ok_or(LobbyMembershipError::CountdownGenerationExhausted)?;
        state.countdown_active = true;
        state.countdown_remaining_seconds = Some(self.countdown_policy.initial_seconds);
        state.manual_start_requested = false;
        let generation = state.countdown_generation;
        let _ = self.updates.send(LobbyUpdate::Countdown {
            remaining_seconds: self.countdown_policy.initial_seconds,
        });
        Ok(generation)
    }

    fn spawn_countdown(self: &Arc<Self>, generation: u64) {
        let room = Arc::clone(self);
        tokio::spawn(async move {
            room.run_countdown(generation).await;
        });
    }

    async fn run_countdown(self: Arc<Self>, generation: u64) {
        let mut remaining_seconds = self.countdown_policy.initial_seconds;

        while remaining_seconds > self.countdown_policy.step_seconds {
            sleep(self.countdown_policy.tick_interval).await;
            remaining_seconds -= self.countdown_policy.step_seconds;
            if !self
                .publish_countdown_tick(generation, remaining_seconds)
                .await
            {
                return;
            }
        }

        sleep(self.countdown_policy.tick_interval).await;
        let mut state = self.state.lock().await;
        if !state.countdown_matches(generation) {
            return;
        }

        state.countdown_active = false;
        state.countdown_remaining_seconds = None;
        state.started = true;
        let _ = self.updates.send(LobbyUpdate::Start);
    }

    async fn publish_countdown_tick(&self, generation: u64, remaining_seconds: u8) -> bool {
        let mut state = self.state.lock().await;
        if !state.countdown_matches(generation) {
            return false;
        }

        state.countdown_remaining_seconds = Some(remaining_seconds);
        let _ = self
            .updates
            .send(LobbyUpdate::Countdown { remaining_seconds });
        true
    }
}

pub(crate) struct LobbyMembership {
    pub(crate) player_id: u8,
    pub(crate) session_id: u64,
    pub(crate) roster: LobbyRoster,
    pub(crate) updates: broadcast::Receiver<LobbyUpdate>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum LobbyUpdate {
    Roster(LobbyRoster),
    Countdown { remaining_seconds: u8 },
    CountdownCancelled,
    Chat { from_player_id: u8, message: String },
    Start,
    PlayerLoaded { player_id: u8 },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ManualStartOutcome {
    Started,
    AlreadyCountingDown,
    AlreadyStarted,
    NotEnoughPlayers,
    PlayersNotReady,
}

pub(crate) struct LobbyControlSnapshot {
    pub(crate) roster: LobbyRoster,
    pub(crate) phase: LobbyPhase,
    pub(crate) loaded_player_ids: Vec<u8>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum LobbyPhase {
    Waiting,
    Countdown { remaining_seconds: u8 },
    Started,
}

#[derive(Debug, Error, Eq, PartialEq)]
pub(crate) enum LobbyJoinError {
    #[error("lobby is full")]
    Full,
    #[error("lobby has already started")]
    Started,
    #[error("lobby countdown has already started")]
    CountdownActive,
    #[error("player name is already present in the lobby")]
    DuplicatePlayerName,
    #[error("lobby session id space is exhausted")]
    SessionIdExhausted,
}

#[derive(Debug, Error, Eq, PartialEq)]
pub(crate) enum LobbyMembershipError {
    #[error("lobby session is no longer active")]
    UnknownSession,
    #[error("game loading has not started")]
    GameNotStarted,
    #[error("lobby countdown generation space is exhausted")]
    CountdownGenerationExhausted,
}

struct LobbyRoomState {
    revision: u64,
    next_session_id: u64,
    players: BTreeMap<u8, ConnectedPlayer>,
    countdown_generation: u64,
    countdown_active: bool,
    countdown_remaining_seconds: Option<u8>,
    manual_start_requested: bool,
    started: bool,
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

    fn can_start_automatic_countdown(&self, maximum_players: u8) -> bool {
        !self.started
            && !self.countdown_active
            && self.players.len() == usize::from(maximum_players)
            && self.players.values().all(|player| player.ready)
    }

    fn can_start_requested_countdown(&self) -> bool {
        self.manual_start_requested
            && !self.started
            && !self.countdown_active
            && self.players.len() >= MINIMUM_MANUAL_START_PLAYERS
            && self.players.values().all(|player| player.ready)
    }

    fn countdown_matches(&self, generation: u64) -> bool {
        self.countdown_active
            && self.countdown_generation == generation
            && self.players.values().all(|player| player.ready)
            && !self.started
    }

    fn loaded_player_ids(&self) -> Vec<u8> {
        self.players
            .iter()
            .filter_map(|(&player_id, player)| player.loaded.then_some(player_id))
            .collect()
    }

    fn require_active_session(
        &self,
        player_id: u8,
        session_id: u64,
    ) -> Result<(), LobbyMembershipError> {
        let session_matches = self
            .players
            .get(&player_id)
            .is_some_and(|player| player.session_id == session_id);
        if !session_matches {
            return Err(LobbyMembershipError::UnknownSession);
        }

        Ok(())
    }
}

struct ConnectedPlayer {
    session_id: u64,
    name: String,
    ready: bool,
    loaded: bool,
}

#[derive(Clone, Copy)]
struct CountdownPolicy {
    initial_seconds: u8,
    step_seconds: u8,
    tick_interval: Duration,
}

impl Default for CountdownPolicy {
    fn default() -> Self {
        Self {
            initial_seconds: LOBBY_COUNTDOWN_SECONDS,
            step_seconds: LOBBY_COUNTDOWN_STEP_SECONDS,
            tick_interval: Duration::from_secs(u64::from(LOBBY_COUNTDOWN_STEP_SECONDS)),
        }
    }
}

fn next_revision(current: u64) -> u64 {
    current.saturating_add(1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::time::timeout;

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

    #[tokio::test]
    async fn starts_a_sixty_second_countdown_only_after_every_player_is_ready() {
        let room = Arc::new(LobbyRoom::new_with_countdown_policy(
            1,
            2,
            CountdownPolicy {
                initial_seconds: 60,
                step_seconds: 10,
                tick_interval: Duration::from_millis(2),
            },
        ));
        let first = room
            .join("First#1000".to_owned())
            .await
            .expect("first player should join");
        let second = room
            .join("Second#2000".to_owned())
            .await
            .expect("second player should join");
        let mut updates = first.updates.resubscribe();
        drain_updates(&mut updates);

        room.mark_ready(first.player_id, first.session_id)
            .await
            .expect("first player should become ready");
        assert!(
            timeout(Duration::from_millis(10), updates.recv())
                .await
                .is_err(),
            "countdown must wait for every player"
        );

        room.mark_ready(second.player_id, second.session_id)
            .await
            .expect("second player should become ready");

        for expected_seconds in [60, 50, 40, 30, 20, 10] {
            assert_eq!(
                receive_update(&mut updates).await,
                LobbyUpdate::Countdown {
                    remaining_seconds: expected_seconds,
                }
            );
        }
        assert_eq!(receive_update(&mut updates).await, LobbyUpdate::Start);
        assert_eq!(
            room.join("Late#3000".to_owned()).await.err(),
            Some(LobbyJoinError::Started)
        );
    }

    #[tokio::test]
    async fn cancels_the_countdown_when_a_player_leaves() {
        let room = Arc::new(LobbyRoom::new_with_countdown_policy(
            1,
            2,
            CountdownPolicy {
                initial_seconds: 60,
                step_seconds: 10,
                tick_interval: Duration::from_millis(50),
            },
        ));
        let first = room
            .join("First#1000".to_owned())
            .await
            .expect("first player should join");
        let second = room
            .join("Second#2000".to_owned())
            .await
            .expect("second player should join");
        let mut updates = first.updates.resubscribe();
        drain_updates(&mut updates);

        room.mark_ready(first.player_id, first.session_id)
            .await
            .expect("first player should become ready");
        room.mark_ready(second.player_id, second.session_id)
            .await
            .expect("second player should become ready");
        assert_eq!(
            receive_update(&mut updates).await,
            LobbyUpdate::Countdown {
                remaining_seconds: 60,
            }
        );

        room.leave(second.player_id, second.session_id).await;
        assert!(matches!(
            receive_update(&mut updates).await,
            LobbyUpdate::Roster(_)
        ));
        assert_eq!(
            receive_update(&mut updates).await,
            LobbyUpdate::CountdownCancelled
        );
        assert!(
            timeout(Duration::from_millis(80), updates.recv())
                .await
                .is_err(),
            "cancelled countdown must not start the game"
        );
    }

    #[tokio::test]
    async fn relays_chat_and_allows_two_ready_players_to_start_manually() {
        let room = Arc::new(LobbyRoom::new_with_countdown_policy(
            1,
            10,
            CountdownPolicy {
                initial_seconds: 60,
                step_seconds: 10,
                tick_interval: Duration::from_millis(50),
            },
        ));
        let first = room
            .join("First#1000".to_owned())
            .await
            .expect("first player should join");
        let second = room
            .join("Second#2000".to_owned())
            .await
            .expect("second player should join");
        let mut updates = first.updates.resubscribe();
        drain_updates(&mut updates);

        room.publish_chat(first.player_id, first.session_id, "hello lobby".to_owned())
            .await
            .expect("chat should publish");
        assert_eq!(
            receive_update(&mut updates).await,
            LobbyUpdate::Chat {
                from_player_id: first.player_id,
                message: "hello lobby".to_owned(),
            }
        );

        assert_eq!(
            room.request_manual_start(first.player_id, first.session_id)
                .await
                .expect("manual start request should be handled"),
            ManualStartOutcome::PlayersNotReady
        );
        room.mark_ready(first.player_id, first.session_id)
            .await
            .expect("first player should become ready");
        room.mark_ready(second.player_id, second.session_id)
            .await
            .expect("second player should become ready");
        assert_eq!(
            receive_update(&mut updates).await,
            LobbyUpdate::Countdown {
                remaining_seconds: 60,
            }
        );
        assert_eq!(
            room.request_manual_start(first.player_id, first.session_id)
                .await
                .expect("repeated manual start should be handled"),
            ManualStartOutcome::AlreadyCountingDown
        );
        assert_eq!(
            room.join("Late#3000".to_owned()).await.err(),
            Some(LobbyJoinError::CountdownActive)
        );
    }

    #[tokio::test]
    async fn manual_start_requires_two_players() {
        let room = Arc::new(LobbyRoom::new(1, 10));
        let first = room
            .join("First#1000".to_owned())
            .await
            .expect("first player should join");
        room.mark_ready(first.player_id, first.session_id)
            .await
            .expect("first player should become ready");

        assert_eq!(
            room.request_manual_start(first.player_id, first.session_id)
                .await
                .expect("manual start request should be handled"),
            ManualStartOutcome::NotEnoughPlayers
        );
    }

    #[tokio::test]
    async fn publishes_each_loaded_player_once_after_start() {
        let room = Arc::new(LobbyRoom::new_with_countdown_policy(
            1,
            2,
            CountdownPolicy {
                initial_seconds: 60,
                step_seconds: 10,
                tick_interval: Duration::from_millis(2),
            },
        ));
        let first = room
            .join("First#1000".to_owned())
            .await
            .expect("first player should join");
        let second = room
            .join("Second#2000".to_owned())
            .await
            .expect("second player should join");
        let mut updates = first.updates.resubscribe();
        drain_updates(&mut updates);

        assert_eq!(
            room.mark_loaded(first.player_id, first.session_id).await,
            Err(LobbyMembershipError::GameNotStarted)
        );
        room.mark_ready(first.player_id, first.session_id)
            .await
            .expect("first player should become ready");
        room.mark_ready(second.player_id, second.session_id)
            .await
            .expect("second player should become ready");
        for _ in [60, 50, 40, 30, 20, 10] {
            receive_update(&mut updates).await;
        }
        assert_eq!(receive_update(&mut updates).await, LobbyUpdate::Start);

        room.mark_loaded(first.player_id, first.session_id)
            .await
            .expect("first loaded state should publish");
        assert_eq!(
            receive_update(&mut updates).await,
            LobbyUpdate::PlayerLoaded {
                player_id: first.player_id,
            }
        );
        room.mark_loaded(first.player_id, first.session_id)
            .await
            .expect("duplicate loaded state should be idempotent");
        assert!(
            timeout(Duration::from_millis(10), updates.recv())
                .await
                .is_err(),
            "duplicate loaded state must not publish"
        );

        room.mark_loaded(second.player_id, second.session_id)
            .await
            .expect("second loaded state should publish");
        assert_eq!(
            room.control_snapshot().await.loaded_player_ids,
            vec![first.player_id, second.player_id]
        );
    }

    fn drain_updates(updates: &mut broadcast::Receiver<LobbyUpdate>) {
        while updates.try_recv().is_ok() {}
    }

    async fn receive_update(updates: &mut broadcast::Receiver<LobbyUpdate>) -> LobbyUpdate {
        timeout(Duration::from_secs(1), updates.recv())
            .await
            .expect("lobby update should arrive")
            .expect("lobby update channel should remain open")
    }
}
