//! The account's own game as Steam pushes it to the UI session, and whether
//! that game can invite a friend right now. The friends list never includes
//! the account itself, so its persona feeds this separate watch channel.
//! Asking Steam for the account's own persona makes its game drop out and
//! come back, which tells every friend again that it started: Obelisk never
//! asks, and only reads what Steam pushes.
use prost::Message;
use tokio::sync::watch;

use crate::{
    friends::{self, RICH_PRESENCE},
    proto,
};
/// EClientPersonaStateFlag Status | PlayerName | Presence | GameExtraInfo |
/// GameDataBlob | RichPresence (SteamKit enums.steamd), as fixtures push them.
#[cfg(any(test, debug_assertions))]
pub(crate) const REQUESTED: u32 = friends::STATUS
    | friends::PLAYER_NAME
    | friends::PRESENCE
    | friends::GAME_EXTRA_INFO
    | friends::GAME_DATA_BLOB
    | RICH_PRESENCE;

/// What Steam's own friend menu would offer: a lobby invitation when the game
/// published a lobby, else its rich presence `connect` string, else nothing.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum GameInvite {
    #[default]
    None,
    Lobby {
        lobby_id: u64,
    },
    Connect {
        connect: String,
    },
}

impl GameInvite {
    /// Steam's limit on a connect string, without its terminating NUL.
    pub const MAX_CONNECT: usize = 255;

    /// An invitation Steam can carry: a lobby's SteamID (account type Chat,
    /// in a real universe) or a connect string that fits Steam's buffer.
    pub fn sendable(&self) -> bool {
        match self {
            Self::None => false,
            Self::Lobby { lobby_id } => {
                (lobby_id >> 52) & 0xf == 8 && lobby_id >> 56 != 0 && *lobby_id as u32 != 0
            }
            Self::Connect { connect } => {
                !connect.is_empty() && connect.len() <= Self::MAX_CONNECT && !connect.contains('\0')
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OwnGame {
    pub revision: u64,
    pub connected: bool,
    /// 0 when the account plays nothing.
    pub app_id: u32,
    pub lobby_id: u64,
    pub connect: Option<String>,
}

impl OwnGame {
    /// What Steam's own friend menu would offer for this game now.
    pub fn invite(&self) -> GameInvite {
        if self.app_id == 0 {
            GameInvite::None
        } else if self.lobby_id != 0 {
            GameInvite::Lobby {
                lobby_id: self.lobby_id,
            }
        } else if let Some(connect) = &self.connect {
            GameInvite::Connect {
                connect: connect.clone(),
            }
        } else {
            GameInvite::None
        }
    }
}

#[derive(Clone)]
pub struct OwnGameSource {
    game: watch::Sender<OwnGame>,
}

impl Default for OwnGameSource {
    fn default() -> Self {
        Self::new()
    }
}

impl OwnGameSource {
    pub fn new() -> Self {
        Self {
            game: watch::channel(OwnGame {
                revision: 1,
                connected: true,
                app_id: 0,
                lobby_id: 0,
                connect: None,
            })
            .0,
        }
    }

    /// Applies a ClientPersonaState packet's entry for `account_id`, if any.
    /// Malformed optional packets are ignored.
    pub fn receive(&self, account_id: u64, body: &[u8]) {
        if let Ok(state) = proto::PersonaState::decode(body) {
            self.receive_state(account_id, &state);
        }
    }

    /// Applies a decoded ClientPersonaState's entry for `account_id`, if any.
    pub(crate) fn receive_state(&self, account_id: u64, state: &proto::PersonaState) {
        if state.status_flags & (friends::STATUS | RICH_PRESENCE) == 0 {
            return;
        }
        let Some(persona) = state
            .friends
            .iter()
            .find(|persona| account_id != 0 && persona.friend_id == account_id)
        else {
            return;
        };
        self.game.send_if_modified(|current| {
            if !current.connected {
                return false;
            }
            let mut next = current.clone();
            apply(&mut next, state.status_flags, persona);
            if next == *current {
                return false;
            }
            next.revision += 1;
            *current = next;
            true
        });
    }

    pub fn snapshot(&self) -> OwnGame {
        self.game.borrow().clone()
    }

    /// Waits for a revision other than `after_revision`; disconnection wakes
    /// every wait.
    pub async fn next(&self, after_revision: u64) -> OwnGame {
        match obelisk_core::watch::next_matching(self.game.subscribe(), |game| {
            game.revision != after_revision || !game.connected
        })
        .await
        {
            Ok(game) | Err(game) => game,
        }
    }

    pub fn disconnect(&self) {
        self.game.send_if_modified(|current| {
            if !current.connected {
                return false;
            }
            *current = OwnGame {
                revision: current.revision + 1,
                connected: false,
                app_id: 0,
                lobby_id: 0,
                connect: None,
            };
            true
        });
    }
}

/// Valve's friends client applies the game fields only with the Status flag
/// and replaces rich presence only with the RichPresence flag.
fn apply(game: &mut OwnGame, flags: u32, persona: &proto::Persona) {
    if flags & friends::STATUS != 0 {
        if persona.persona_state.unwrap_or(0) == 0 {
            game.app_id = 0;
            game.lobby_id = 0;
            game.connect = None;
        } else {
            let app_id = persona
                .game_played_app_id
                .filter(|id| *id != 0)
                .or_else(|| persona.game_id.map(|id| (id & 0x00ff_ffff) as u32))
                .unwrap_or(0);
            // Another game's connect string must never invite into this one.
            if app_id != game.app_id {
                game.connect = None;
            }
            game.app_id = app_id;
            game.lobby_id = persona.game_lobby_id.unwrap_or(0);
        }
    }
    if flags & RICH_PRESENCE != 0 {
        game.connect = persona
            .rich_presence
            .iter()
            .find(|pair| pair.key.eq_ignore_ascii_case("connect"))
            .map(|pair| pair.value.clone())
            .filter(|connect| !connect.is_empty());
    }
}

/// Wire bodies for debug fixtures, so tests feed the production decoder.
#[cfg(debug_assertions)]
pub mod fixture {
    use prost::Message;

    use crate::proto;

    /// The account's persona with the Status and RichPresence groups.
    pub fn persona(account_id: u64, app_id: u32, lobby_id: u64, connect: Option<&str>) -> Vec<u8> {
        proto::PersonaState {
            status_flags: super::REQUESTED,
            friends: vec![proto::Persona {
                friend_id: account_id,
                persona_state: Some(1),
                game_played_app_id: Some(app_id),
                game_lobby_id: Some(lobby_id),
                rich_presence: connect
                    .map(|value| proto::RichPresence {
                        key: "connect".into(),
                        value: value.into(),
                    })
                    .into_iter()
                    .collect(),
                ..Default::default()
            }],
        }
        .encode_to_vec()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ACCOUNT: u64 = 76_561_197_960_265_730;

    fn persona(flags: u32, persona: proto::Persona) -> Vec<u8> {
        proto::PersonaState {
            status_flags: flags,
            friends: vec![proto::Persona {
                friend_id: ACCOUNT,
                ..persona
            }],
        }
        .encode_to_vec()
    }

    fn playing(app_id: u32, lobby_id: u64, connect: Option<&str>) -> proto::Persona {
        proto::Persona {
            persona_state: Some(1),
            game_played_app_id: Some(app_id),
            game_lobby_id: Some(lobby_id),
            rich_presence: connect
                .map(|value| proto::RichPresence {
                    key: "Connect".into(),
                    value: value.into(),
                })
                .into_iter()
                .collect(),
            ..Default::default()
        }
    }

    #[test]
    fn independent_wire_fixture_decodes_lobby_and_rich_presence() {
        // status_flags 1 = 4097; friend: friendid fixed64 1, persona_state 2,
        // game_played_app_id 3 = 620, rich_presence 71 {key 1, value 2},
        // game_lobby_id fixed64 73.
        let mut friend = vec![0x09];
        friend.extend_from_slice(&ACCOUNT.to_le_bytes());
        friend.extend_from_slice(&[0x10, 0x01, 0x18, 0xec, 0x04]);
        friend.extend_from_slice(&[0xba, 0x04, 0x0e, 0x0a, 0x07]);
        friend.extend_from_slice(b"connect");
        friend.extend_from_slice(&[0x12, 0x03]);
        friend.extend_from_slice(b"+cx");
        friend.extend_from_slice(&[0xc9, 0x04]);
        friend.extend_from_slice(&0x0186_0000_0000_0042_u64.to_le_bytes());
        let mut body = vec![0x08, 0x81, 0x20, 0x12, friend.len() as u8];
        body.extend_from_slice(&friend);
        let source = OwnGameSource::new();
        source.receive(ACCOUNT, &body);
        let game = source.snapshot();
        assert_eq!(game.app_id, 620);
        assert_eq!(game.connect.as_deref(), Some("+cx"));
        assert_eq!(
            game.invite(),
            GameInvite::Lobby {
                lobby_id: 0x0186_0000_0000_0042
            }
        );
    }

    #[test]
    fn invite_follows_steams_menu_rule() {
        let source = OwnGameSource::new();
        source.receive(
            ACCOUNT,
            &persona(REQUESTED, playing(620, 0, Some("+connect 1"))),
        );
        assert_eq!(
            source.snapshot().invite(),
            GameInvite::Connect {
                connect: "+connect 1".into()
            }
        );
        // A lobby wins over the connect string.
        source.receive(
            ACCOUNT,
            &persona(REQUESTED, playing(620, 9, Some("+connect 1"))),
        );
        assert_eq!(
            source.snapshot().invite(),
            GameInvite::Lobby { lobby_id: 9 }
        );
        // No game, no invitation, whatever the leftovers say.
        source.receive(
            ACCOUNT,
            &persona(REQUESTED, playing(0, 9, Some("+connect 1"))),
        );
        assert_eq!(source.snapshot().invite(), GameInvite::None);
        // A game without lobby or connect string offers nothing.
        source.receive(ACCOUNT, &persona(REQUESTED, playing(620, 0, None)));
        assert_eq!(source.snapshot().invite(), GameInvite::None);
        // The app may come from the game ID's low 24 bits.
        source.receive(
            ACCOUNT,
            &persona(
                friends::STATUS,
                proto::Persona {
                    persona_state: Some(1),
                    game_id: Some(0x0100_0000_0000_026c),
                    game_lobby_id: Some(5),
                    ..Default::default()
                },
            ),
        );
        assert_eq!(source.snapshot().app_id, 620);
        assert_eq!(
            source.snapshot().invite(),
            GameInvite::Lobby { lobby_id: 5 }
        );
    }

    #[test]
    fn game_fields_need_the_status_flag_and_other_accounts_are_ignored() {
        let source = OwnGameSource::new();
        source.receive(ACCOUNT, &persona(friends::STATUS, playing(620, 0, None)));
        let revision = source.snapshot().revision;
        // Without Status, a packet cannot change the game or lobby.
        source.receive(
            ACCOUNT,
            &persona(
                friends::PLAYER_NAME | friends::GAME_DATA_BLOB,
                playing(730, 3, None),
            ),
        );
        source.receive(
            ACCOUNT,
            &proto::PersonaState {
                status_flags: REQUESTED,
                friends: vec![proto::Persona {
                    friend_id: ACCOUNT + 1,
                    ..playing(730, 3, None)
                }],
            }
            .encode_to_vec(),
        );
        source.receive(ACCOUNT, &[0xff]);
        assert_eq!(source.snapshot().app_id, 620);
        assert_eq!(source.snapshot().revision, revision);
        // Rich presence alone updates the connect string of the current game.
        source.receive(
            ACCOUNT,
            &persona(RICH_PRESENCE, playing(0, 0, Some("+join"))),
        );
        assert_eq!(
            source.snapshot().invite(),
            GameInvite::Connect {
                connect: "+join".into()
            }
        );
        // A different game drops the previous game's connect string.
        source.receive(ACCOUNT, &persona(friends::STATUS, playing(730, 0, None)));
        assert_eq!(source.snapshot().connect, None);
        // Going offline clears everything.
        source.receive(
            ACCOUNT,
            &persona(friends::STATUS, proto::Persona::default()),
        );
        assert_eq!(source.snapshot().app_id, 0);
    }

    #[tokio::test]
    async fn observers_wake_on_change_and_disconnect() {
        let source = OwnGameSource::new();
        // An unchanged persona changes no revision.
        source.receive(ACCOUNT, &persona(REQUESTED, playing(0, 0, None)));
        assert_eq!(source.snapshot().revision, 1);

        let waiting = tokio::spawn({
            let source = source.clone();
            async move { source.next(1).await }
        });
        tokio::task::yield_now().await;
        source.receive(ACCOUNT, &persona(REQUESTED, playing(620, 2, None)));
        assert_eq!(waiting.await.unwrap().revision, 2);
        let waiting = tokio::spawn({
            let source = source.clone();
            async move { source.next(2).await }
        });
        tokio::task::yield_now().await;
        source.disconnect();
        let ended = waiting.await.unwrap();
        assert!(!ended.connected);
        assert_eq!(ended.invite(), GameInvite::None);
    }
}
