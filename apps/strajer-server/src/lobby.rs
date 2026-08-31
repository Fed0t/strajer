use std::collections::{BTreeMap, HashMap, VecDeque};
use std::sync::Arc;
use std::time::Duration;

use strajer_protocol::{
    GameEndReason, LOBBY_COUNTDOWN_SECONDS, LOBBY_COUNTDOWN_STEP_SECONDS, LobbyCatalog,
    LobbyLeaveReason, LobbyPlayer, LobbyRoster,
};
use strajer_w3gs::{PlayerAction, incoming_action_frames};
use thiserror::Error;
use tokio::sync::{Mutex, broadcast};
use tokio::time::{Instant, MissedTickBehavior, interval_at, sleep};
use tracing::error;

const LOBBY_UPDATE_CAPACITY: usize = 512;
const MINIMUM_MANUAL_START_PLAYERS: usize = 2;
const MAX_PENDING_GAME_ACTIONS: usize = 512;
const MAX_PENDING_GAME_ACTION_BYTES: usize = 512 * 1_452;
const MAX_PENDING_CHECKSUMS_PER_PLAYER: usize = 64;

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
    gameplay_policy: GameplayPolicy,
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
        Self::new_with_policies(
            initial_revision,
            maximum_players,
            countdown_policy,
            GameplayPolicy::default(),
        )
    }

    fn new_with_policies(
        initial_revision: u64,
        maximum_players: u8,
        countdown_policy: CountdownPolicy,
        gameplay_policy: GameplayPolicy,
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
            gameplay_policy,
            state: Mutex::new(LobbyRoomState {
                revision: initial_revision,
                next_session_id: 1,
                players: BTreeMap::new(),
                countdown_generation: 0,
                countdown_active: false,
                countdown_remaining_seconds: None,
                manual_start_requested: false,
                started: false,
                gameplay_active: false,
                game_ended: false,
                gameplay_generation: 0,
                gameplay_sequence: 0,
                pending_actions: VecDeque::new(),
                pending_action_bytes: 0,
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
                last_game_sequence: 0,
                checksums: VecDeque::new(),
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
        self: &Arc<Self>,
        player_id: u8,
        session_id: u64,
    ) -> Result<(), LobbyMembershipError> {
        let gameplay_generation = {
            let mut state = self.state.lock().await;
            state.require_active_session(player_id, session_id)?;
            if !state.started || state.game_ended {
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
            self.begin_gameplay_if_ready(&mut state)?
        };

        if let Some(generation) = gameplay_generation {
            self.spawn_gameplay(generation);
        }
        Ok(())
    }

    pub(crate) async fn submit_action(
        &self,
        player_id: u8,
        session_id: u64,
        sequence: u64,
        action: PlayerAction,
    ) -> Result<(), LobbyMembershipError> {
        let mut state = self.state.lock().await;
        state.require_active_session(player_id, session_id)?;
        if !state.gameplay_active || state.game_ended {
            return Err(LobbyMembershipError::GameNotRunning);
        }
        if action.player_id() != player_id {
            return Err(LobbyMembershipError::ActionPlayerMismatch);
        }

        let action_bytes = action.data().len() + 3;
        if state.pending_actions.len() >= MAX_PENDING_GAME_ACTIONS
            || state.pending_action_bytes.saturating_add(action_bytes)
                > MAX_PENDING_GAME_ACTION_BYTES
        {
            return Err(LobbyMembershipError::ActionQueueFull);
        }
        state.accept_game_sequence(player_id, sequence)?;
        state.pending_action_bytes += action_bytes;
        state.pending_actions.push_back(action);
        Ok(())
    }

    pub(crate) async fn submit_keepalive(
        &self,
        player_id: u8,
        session_id: u64,
        sequence: u64,
        checksum: u32,
    ) -> Result<(), LobbyMembershipError> {
        let desync_detected =
            {
                let mut state = self.state.lock().await;
                state.require_active_session(player_id, session_id)?;
                if !state.gameplay_active || state.game_ended {
                    return Err(LobbyMembershipError::GameNotRunning);
                }
                if state.players.get(&player_id).is_some_and(|player| {
                    player.checksums.len() >= MAX_PENDING_CHECKSUMS_PER_PLAYER
                }) {
                    return Err(LobbyMembershipError::ChecksumQueueFull);
                }

                state.accept_game_sequence(player_id, sequence)?;
                state
                    .players
                    .get_mut(&player_id)
                    .ok_or(LobbyMembershipError::UnknownSession)?
                    .checksums
                    .push_back(checksum);
                state.consume_checksum_consensus()
            };

        if desync_detected {
            self.end_game(GameEndReason::Desync).await;
        }
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

    pub(crate) async fn leave(
        self: &Arc<Self>,
        player_id: u8,
        session_id: u64,
        reason: LobbyLeaveReason,
    ) -> Result<(), LobbyMembershipError> {
        let (gameplay_generation, game_over_generation) = {
            let mut state = self.state.lock().await;
            let session_matches = state
                .players
                .get(&player_id)
                .is_some_and(|player| player.session_id == session_id);
            if !session_matches {
                return Ok(());
            }

            if state.gameplay_active && !state.pending_actions.is_empty() {
                self.publish_pending_actions(&mut state, 0)?;
            }

            let game_had_started = state.started;
            state.players.remove(&player_id);
            state.revision = next_revision(state.revision);
            let roster = state.roster();
            let countdown_was_active = state.countdown_active;
            state.countdown_active = false;
            state.countdown_remaining_seconds = None;
            state.manual_start_requested = false;

            if game_had_started {
                let _ = self.updates.send(LobbyUpdate::PlayerLeft {
                    player_id,
                    reason,
                    roster,
                });
            } else {
                let _ = self.updates.send(LobbyUpdate::Roster(roster));
                if countdown_was_active {
                    let _ = self.updates.send(LobbyUpdate::CountdownCancelled);
                }
            }

            if state.players.is_empty() {
                state.reset_game();
                (None, None)
            } else if game_had_started && state.players.len() == 1 {
                (None, Some(state.gameplay_generation))
            } else {
                (self.begin_gameplay_if_ready(&mut state)?, None)
            }
        };

        if let Some(generation) = gameplay_generation {
            self.spawn_gameplay(generation);
        }
        if let Some(generation) = game_over_generation {
            self.spawn_last_player_game_over(generation);
        }
        Ok(())
    }

    #[cfg(test)]
    pub(crate) async fn snapshot(&self) -> LobbyRoster {
        self.state.lock().await.roster()
    }

    pub(crate) async fn control_snapshot(&self) -> LobbyControlSnapshot {
        let state = self.state.lock().await;
        let phase = if state.game_ended {
            LobbyPhase::Ended
        } else if state.gameplay_active {
            LobbyPhase::Playing
        } else if state.started {
            LobbyPhase::Loading
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

    fn begin_gameplay_if_ready(
        &self,
        state: &mut LobbyRoomState,
    ) -> Result<Option<u64>, LobbyMembershipError> {
        if !state.started
            || state.gameplay_active
            || state.game_ended
            || state.players.len() < MINIMUM_MANUAL_START_PLAYERS
            || !state.players.values().all(|player| player.loaded)
        {
            return Ok(None);
        }

        state.gameplay_generation = state
            .gameplay_generation
            .checked_add(1)
            .ok_or(LobbyMembershipError::GameplayGenerationExhausted)?;
        state.gameplay_active = true;
        state.gameplay_sequence = 0;
        Ok(Some(state.gameplay_generation))
    }

    fn spawn_gameplay(self: &Arc<Self>, generation: u64) {
        let room = Arc::clone(self);
        tokio::spawn(async move {
            room.run_gameplay(generation).await;
        });
    }

    async fn run_gameplay(self: Arc<Self>, generation: u64) {
        let mut tick_interval = interval_at(
            Instant::now() + self.gameplay_policy.tick_interval,
            self.gameplay_policy.tick_interval,
        );
        tick_interval.set_missed_tick_behavior(MissedTickBehavior::Delay);

        loop {
            tick_interval.tick().await;
            match self.publish_gameplay_tick(generation).await {
                Ok(true) => {}
                Ok(false) => return,
                Err(runtime_error) => {
                    error!(%runtime_error, "authoritative gameplay actor stopped");
                    self.end_game(GameEndReason::ProtocolError).await;
                    return;
                }
            }
        }
    }

    async fn publish_gameplay_tick(&self, generation: u64) -> Result<bool, LobbyMembershipError> {
        let mut state = self.state.lock().await;
        if !state.gameplay_active
            || state.game_ended
            || state.gameplay_generation != generation
            || state.players.is_empty()
        {
            return Ok(false);
        }

        self.publish_pending_actions(&mut state, self.gameplay_policy.tick_increment_ms)?;
        Ok(true)
    }

    fn publish_pending_actions(
        &self,
        state: &mut LobbyRoomState,
        time_increment_ms: u16,
    ) -> Result<(), LobbyMembershipError> {
        let actions = state.pending_actions.drain(..).collect::<Vec<_>>();
        state.pending_action_bytes = 0;
        let frames = incoming_action_frames(time_increment_ms, &actions)
            .map_err(|_| LobbyMembershipError::GameplayFrameEncoding)?;

        for frame in frames {
            state.gameplay_sequence = state
                .gameplay_sequence
                .checked_add(1)
                .ok_or(LobbyMembershipError::GameplaySequenceExhausted)?;
            let _ = self.updates.send(LobbyUpdate::GameFrame {
                sequence: state.gameplay_sequence,
                frame: frame.to_bytes(),
            });
        }
        Ok(())
    }

    fn spawn_last_player_game_over(self: &Arc<Self>, generation: u64) {
        let room = Arc::clone(self);
        tokio::spawn(async move {
            sleep(room.gameplay_policy.last_player_game_over_delay).await;
            let should_end = {
                let state = room.state.lock().await;
                state.started
                    && !state.game_ended
                    && state.players.len() == 1
                    && state.gameplay_generation == generation
            };
            if should_end {
                room.end_game(GameEndReason::LastPlayerStanding).await;
            }
        });
    }

    async fn end_game(&self, reason: GameEndReason) {
        let mut state = self.state.lock().await;
        if !state.started || state.game_ended {
            return;
        }
        if state.players.is_empty() {
            state.reset_game();
            return;
        }

        state.gameplay_active = false;
        state.game_ended = true;
        state.gameplay_generation = state.gameplay_generation.saturating_add(1);
        state.pending_actions.clear();
        state.pending_action_bytes = 0;
        let _ = self.updates.send(LobbyUpdate::GameEnded { reason });
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
    Countdown {
        remaining_seconds: u8,
    },
    CountdownCancelled,
    Chat {
        from_player_id: u8,
        message: String,
    },
    Start,
    PlayerLoaded {
        player_id: u8,
    },
    PlayerLeft {
        player_id: u8,
        reason: LobbyLeaveReason,
        roster: LobbyRoster,
    },
    GameFrame {
        sequence: u64,
        frame: Vec<u8>,
    },
    GameEnded {
        reason: GameEndReason,
    },
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
    Loading,
    Playing,
    Ended,
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
    #[error("authoritative gameplay is not running")]
    GameNotRunning,
    #[error("game action player does not match the authenticated session")]
    ActionPlayerMismatch,
    #[error("game tunnel sequence mismatch: expected {expected}, got {actual}")]
    InvalidGameSequence { expected: u64, actual: u64 },
    #[error("game tunnel sequence space is exhausted")]
    GameSequenceExhausted,
    #[error("pending game action queue is full")]
    ActionQueueFull,
    #[error("pending game checksum queue is full")]
    ChecksumQueueFull,
    #[error("gameplay generation space is exhausted")]
    GameplayGenerationExhausted,
    #[error("authoritative gameplay sequence space is exhausted")]
    GameplaySequenceExhausted,
    #[error("could not encode an authoritative W3GS gameplay frame")]
    GameplayFrameEncoding,
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
    gameplay_active: bool,
    game_ended: bool,
    gameplay_generation: u64,
    gameplay_sequence: u64,
    pending_actions: VecDeque<PlayerAction>,
    pending_action_bytes: usize,
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

    fn accept_game_sequence(
        &mut self,
        player_id: u8,
        sequence: u64,
    ) -> Result<(), LobbyMembershipError> {
        let player = self
            .players
            .get_mut(&player_id)
            .ok_or(LobbyMembershipError::UnknownSession)?;
        let expected = player
            .last_game_sequence
            .checked_add(1)
            .ok_or(LobbyMembershipError::GameSequenceExhausted)?;
        if sequence != expected {
            return Err(LobbyMembershipError::InvalidGameSequence {
                expected,
                actual: sequence,
            });
        }
        player.last_game_sequence = sequence;
        Ok(())
    }

    fn consume_checksum_consensus(&mut self) -> bool {
        while !self.players.is_empty()
            && self
                .players
                .values()
                .all(|player| !player.checksums.is_empty())
        {
            let expected_checksum = self
                .players
                .values()
                .find_map(|player| player.checksums.front().copied())
                .expect("all active players have a checksum");
            let mismatch = self
                .players
                .values()
                .any(|player| player.checksums.front().copied() != Some(expected_checksum));
            for player in self.players.values_mut() {
                player.checksums.pop_front();
            }
            if mismatch {
                return true;
            }
        }
        false
    }

    fn reset_game(&mut self) {
        self.countdown_active = false;
        self.countdown_remaining_seconds = None;
        self.manual_start_requested = false;
        self.started = false;
        self.gameplay_active = false;
        self.game_ended = false;
        self.gameplay_generation = self.gameplay_generation.saturating_add(1);
        self.gameplay_sequence = 0;
        self.pending_actions.clear();
        self.pending_action_bytes = 0;
    }
}

struct ConnectedPlayer {
    session_id: u64,
    name: String,
    ready: bool,
    loaded: bool,
    last_game_sequence: u64,
    checksums: VecDeque<u32>,
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

#[derive(Clone, Copy)]
struct GameplayPolicy {
    tick_interval: Duration,
    tick_increment_ms: u16,
    last_player_game_over_delay: Duration,
}

impl Default for GameplayPolicy {
    fn default() -> Self {
        Self {
            tick_interval: Duration::from_millis(100),
            tick_increment_ms: 100,
            last_player_game_over_delay: Duration::from_secs(60),
        }
    }
}

fn next_revision(current: u64) -> u64 {
    current.saturating_add(1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use strajer_w3gs::{Frame, IncomingActionFrame};
    use tokio::time::timeout;

    #[tokio::test]
    async fn allocates_and_releases_two_distinct_players() {
        let room = Arc::new(LobbyRoom::new(1, 2));
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

        room.leave(
            second.player_id,
            second.session_id,
            LobbyLeaveReason::Disconnect,
        )
        .await
        .expect("second player should leave");
        let roster = room.snapshot().await;
        assert_eq!(roster.players.len(), 1);
        assert_eq!(roster.players[0].player_id, 1);
    }

    #[tokio::test]
    async fn stale_disconnect_cannot_remove_a_reused_player_id() {
        let room = Arc::new(LobbyRoom::new(1, 1));
        let first = room
            .join("First#1000".to_owned())
            .await
            .expect("first player should join");
        room.leave(
            first.player_id,
            first.session_id,
            LobbyLeaveReason::Disconnect,
        )
        .await
        .expect("first player should leave");
        let replacement = room
            .join("Next#2000".to_owned())
            .await
            .expect("replacement should join");

        room.leave(
            first.player_id,
            first.session_id,
            LobbyLeaveReason::Disconnect,
        )
        .await
        .expect("stale disconnect should be idempotent");
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

        room.leave(
            second.player_id,
            second.session_id,
            LobbyLeaveReason::Disconnect,
        )
        .await
        .expect("second player should leave");
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

    #[tokio::test]
    async fn relays_authoritative_actions_and_terminates_on_desync() {
        let room = fast_gameplay_room();
        let (first, second, mut updates) = start_loaded_game(&room).await;

        let empty_timeslot = receive_game_frame(&mut updates).await;
        let decoded_empty = IncomingActionFrame::decode(&empty_timeslot)
            .expect("empty authoritative timeslot should decode");
        assert_eq!(decoded_empty.time_increment_ms(), 100);
        assert!(decoded_empty.actions().is_empty());

        room.submit_action(
            first.player_id,
            first.session_id,
            1,
            PlayerAction::new(first.player_id, vec![0x10, 0x20, 0x30])
                .expect("test action should build"),
        )
        .await
        .expect("first action should enqueue");

        let action_timeslot = loop {
            let frame = receive_game_frame(&mut updates).await;
            let decoded = IncomingActionFrame::decode(&frame)
                .expect("authoritative action timeslot should decode");
            if !decoded.actions().is_empty() {
                break decoded;
            }
        };
        assert_eq!(action_timeslot.time_increment_ms(), 100);
        assert_eq!(
            action_timeslot.actions(),
            &[PlayerAction::new(first.player_id, vec![0x10, 0x20, 0x30])
                .expect("test action should build")]
        );

        room.submit_keepalive(first.player_id, first.session_id, 2, 0xAABB_CCDD)
            .await
            .expect("first checksum should enqueue");
        room.submit_keepalive(second.player_id, second.session_id, 1, 0xAABB_CCDD)
            .await
            .expect("matching second checksum should enqueue");
        room.submit_keepalive(first.player_id, first.session_id, 3, 0x1111_2222)
            .await
            .expect("next first checksum should enqueue");
        room.submit_keepalive(second.player_id, second.session_id, 2, 0x3333_4444)
            .await
            .expect("mismatching second checksum should terminate the game");

        loop {
            if let LobbyUpdate::GameEnded { reason } = receive_update(&mut updates).await {
                assert_eq!(reason, GameEndReason::Desync);
                break;
            }
        }
    }

    #[tokio::test]
    async fn propagates_in_game_leave_and_resets_after_the_last_player() {
        let room = fast_gameplay_room();
        let (first, second, mut updates) = start_loaded_game(&room).await;

        room.leave(second.player_id, second.session_id, LobbyLeaveReason::Lost)
            .await
            .expect("second player should leave the running game");
        loop {
            if let LobbyUpdate::PlayerLeft {
                player_id,
                reason,
                roster,
            } = receive_update(&mut updates).await
            {
                assert_eq!(player_id, second.player_id);
                assert_eq!(reason, LobbyLeaveReason::Lost);
                assert_eq!(roster.players.len(), 1);
                assert_eq!(roster.players[0].player_id, first.player_id);
                break;
            }
        }

        loop {
            if let LobbyUpdate::GameEnded { reason } = receive_update(&mut updates).await {
                assert_eq!(reason, GameEndReason::LastPlayerStanding);
                break;
            }
        }

        room.leave(
            first.player_id,
            first.session_id,
            LobbyLeaveReason::Disconnect,
        )
        .await
        .expect("last player should leave");
        let replacement = room
            .join("Replacement#3000".to_owned())
            .await
            .expect("empty room should accept a new game");
        assert_eq!(replacement.player_id, 1);
    }

    fn fast_gameplay_room() -> Arc<LobbyRoom> {
        Arc::new(LobbyRoom::new_with_policies(
            1,
            2,
            CountdownPolicy {
                initial_seconds: 10,
                step_seconds: 10,
                tick_interval: Duration::from_millis(2),
            },
            GameplayPolicy {
                tick_interval: Duration::from_millis(2),
                tick_increment_ms: 100,
                last_player_game_over_delay: Duration::from_millis(5),
            },
        ))
    }

    async fn start_loaded_game(
        room: &Arc<LobbyRoom>,
    ) -> (
        LobbyMembership,
        LobbyMembership,
        broadcast::Receiver<LobbyUpdate>,
    ) {
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
        loop {
            if receive_update(&mut updates).await == LobbyUpdate::Start {
                break;
            }
        }

        room.mark_loaded(first.player_id, first.session_id)
            .await
            .expect("first player should load");
        room.mark_loaded(second.player_id, second.session_id)
            .await
            .expect("second player should load");
        (first, second, updates)
    }

    async fn receive_game_frame(updates: &mut broadcast::Receiver<LobbyUpdate>) -> Frame {
        loop {
            if let LobbyUpdate::GameFrame { frame, .. } = receive_update(updates).await {
                return Frame::decode_exact(&frame, 1_460)
                    .expect("authoritative W3GS frame should decode");
            }
        }
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
