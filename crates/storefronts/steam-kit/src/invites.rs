//! Game invitations friends send the account, as they reach its UI session.
//! Steam writes each one into the friends chat as BBCode, and also sends a
//! lobby invitation as ClientChatInvite (800) and a connect invitation as
//! ClientInviteToGame (7005); whichever comes first is the invitation, the
//! others repeat it. Like Valve's chat, the source keeps only the latest
//! invitation of each friend.
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use prost::Message;
use tokio::sync::watch;

use crate::{chat, own_game::GameInvite, proto};

pub const CHAT_INVITE: u32 = 800;
pub const INVITE_TO_GAME: u32 = 7005;
/// EChatRoomType::Lobby (SteamKit enums.steamd).
const LOBBY_ROOM: i32 = 3;
/// How long the same invitation from the same friend counts as one: its
/// copies arrive together, a resent one much later.
const REPEAT: Duration = Duration::from_secs(120);
const MAX_INVITES: usize = 32;

/// An invitation into a game, as its friend sent it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReceivedInvite {
    pub friend: u64,
    pub app_id: u32,
    /// Never `GameInvite::None`.
    pub invite: GameInvite,
    /// When this session received it, in seconds since the Unix epoch.
    pub received_at: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReceivedInvites {
    pub revision: u64,
    pub connected: bool,
    /// Newest first, at most one per friend.
    pub invites: Vec<ReceivedInvite>,
}

/// The invitation a friends chat message carries: the first `[lobbyinvite]`
/// or `[gameinvite]` tag, read as Valve's chat reads its arguments. A Remote
/// Play invitation, or one naming no app, is none.
pub fn chat_invite(bbcode: &str) -> Option<(u32, GameInvite)> {
    let (name, arguments) = tags(bbcode).find(|(name, _)| {
        name.eq_ignore_ascii_case("lobbyinvite") || name.eq_ignore_ascii_case("gameinvite")
    })?;
    let argument = |key: &str| {
        arguments
            .iter()
            .find(|(name, _)| name == key)
            .map(|(_, value)| value.as_str())
    };
    let app_id = argument("appid")?
        .trim()
        .parse::<u32>()
        .ok()
        .filter(|id| *id != 0)?;
    let invite = if name.eq_ignore_ascii_case("lobbyinvite") {
        GameInvite::Lobby {
            lobby_id: argument("lobbyid")?.trim().parse().ok()?,
        }
    } else {
        if argument("remoteplay").is_some_and(|value| !value.is_empty()) {
            return None;
        }
        GameInvite::Connect {
            connect: argument("connect")?.to_owned(),
        }
    };
    invite.sendable().then_some((app_id, invite))
}

/// The opening tags of `bbcode` with their arguments. A backslash escapes the
/// next character; a quoted argument may hold a closing bracket.
fn tags(bbcode: &str) -> impl Iterator<Item = (String, Vec<(String, String)>)> + '_ {
    let mut characters = bbcode.chars();
    std::iter::from_fn(move || {
        loop {
            match characters.next()? {
                '\\' => {
                    characters.next();
                }
                '[' => {
                    let mut inner = String::new();
                    let (mut quoted, mut escaped) = (false, false);
                    for character in characters.by_ref() {
                        if !escaped && character == ']' && !quoted {
                            break;
                        }
                        if !escaped && character == '"' {
                            quoted = !quoted;
                        }
                        escaped = !escaped && character == '\\';
                        inner.push(character);
                    }
                    if let Some(tag) = tag(&inner) {
                        return Some(tag);
                    }
                }
                _ => {}
            }
        }
    })
}

/// An opening tag's name and arguments, split as Valve's chat splits them:
/// `name=value` gives the unnamed argument "", `name key="value" …` the others.
fn tag(inner: &str) -> Option<(String, Vec<(String, String)>)> {
    if inner.starts_with('/') {
        return None;
    }
    let end = match (inner.find('='), inner.find(' ')) {
        (Some(equals), Some(space)) => equals.min(space),
        (equals, space) => equals.or(space).unwrap_or(inner.len()),
    };
    let (name, rest) = inner.split_at(end);
    if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        return None;
    }
    Some((name.to_owned(), arguments(rest)))
}

/// Valve's argument reader (friends.js, the BBCode parser's `args`), state
/// for state: 0 between arguments, 1 in a key, 2 after `=`, 3 in a bare
/// value, 4 in a quoted one.
fn arguments(source: &str) -> Vec<(String, String)> {
    let characters: Vec<char> = source.chars().collect();
    let mut arguments = Vec::new();
    let Some(first) = characters.first() else {
        return arguments;
    };
    let (mut key, mut value) = (String::new(), String::new());
    let mut state = if *first == '=' { 2 } else { 0 };
    let mut escaped = false;
    for &character in &characters[1..] {
        let (mut keep, mut commit) = (true, false);
        match state {
            0 => {
                if character == '=' {
                    return Vec::new();
                }
                if character == ' ' {
                    continue;
                }
                state = 1;
            }
            1 => {
                if (character == '=' || character == ' ') && !escaped {
                    state = if character == ' ' { 0 } else { 2 };
                    keep = false;
                }
            }
            2 => {
                if character == ' ' {
                    (state, keep, commit) = (0, false, true);
                } else if character == '"' {
                    (state, keep) = (4, false);
                } else {
                    state = 3;
                }
            }
            _ => {
                if (character == ' ' && state != 4 && !escaped)
                    || (character == '"' && state == 4 && !escaped)
                {
                    (state, keep, commit) = (0, false, true);
                }
            }
        }
        if keep {
            if character != '\\' || escaped {
                escaped = false;
                match state {
                    1 => key.push(character),
                    3 | 4 => value.push(character),
                    _ => {}
                }
            } else {
                escaped = true;
            }
        }
        if commit {
            arguments.push((std::mem::take(&mut key), std::mem::take(&mut value)));
        }
    }
    if state != 0 {
        arguments.push((key, value));
    }
    arguments
}

#[derive(Clone)]
pub struct InviteSource {
    state: watch::Sender<ReceivedInvites>,
}

impl Default for InviteSource {
    fn default() -> Self {
        Self::new()
    }
}

impl InviteSource {
    pub fn new() -> Self {
        Self {
            state: watch::channel(ReceivedInvites {
                revision: 1,
                connected: true,
                invites: Vec::new(),
            })
            .0,
        }
    }

    /// Applies a friends chat notification: a friend's message carrying an
    /// invitation. The account's own messages, from any session, are not.
    /// Malformed bodies are ignored.
    pub fn receive_notification(&self, method: &str, body: &[u8]) {
        if method != chat::INCOMING_MESSAGE {
            return;
        }
        let Ok(message) = proto::IncomingMessage::decode(body) else {
            return;
        };
        if message.local_echo || message.chat_entry_type != chat::CHAT_MESSAGE {
            return;
        }
        if let Some((app_id, invite)) = chat_invite(&message.message) {
            self.receive(message.steamid_friend, app_id, invite);
        }
    }

    /// Applies a ClientChatInvite into a lobby, or a ClientInviteToGame whose
    /// header's routing AppID names the game. Other packets are ignored.
    pub fn receive_packet(&self, kind: u32, routing_app: Option<u32>, body: &[u8]) {
        match kind {
            CHAT_INVITE => {
                let Ok(invite) = proto::ChatInvite::decode(body) else {
                    return;
                };
                // CGameID of an app: no type or mod above its 24-bit AppID.
                if invite.chatroom_type != LOBBY_ROOM || invite.game_id >> 24 != 0 {
                    return;
                }
                self.receive(
                    invite.steam_id_patron,
                    invite.game_id as u32,
                    GameInvite::Lobby {
                        lobby_id: invite.steam_id_chat,
                    },
                );
            }
            INVITE_TO_GAME => {
                let Ok(invite) = proto::InviteToGame::decode(body) else {
                    return;
                };
                if !invite.remote_play.is_empty() {
                    return;
                }
                self.receive(
                    invite.steam_id_src,
                    routing_app.unwrap_or(0),
                    GameInvite::Connect {
                        connect: invite.connect_string,
                    },
                );
            }
            _ => {}
        }
    }

    fn receive(&self, friend: u64, app_id: u32, invite: GameInvite) {
        if !individual(friend) || app_id == 0 || !invite.sendable() {
            return;
        }
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_secs());
        self.state.send_if_modified(|state| {
            if !state.connected {
                return false;
            }
            if state.invites.iter().any(|known| {
                known.friend == friend
                    && known.app_id == app_id
                    && known.invite == invite
                    && now.saturating_sub(known.received_at) < REPEAT.as_secs()
            }) {
                return false;
            }
            state.invites.retain(|known| known.friend != friend);
            state.invites.insert(
                0,
                ReceivedInvite {
                    friend,
                    app_id,
                    invite,
                    received_at: now,
                },
            );
            state.invites.truncate(MAX_INVITES);
            state.revision += 1;
            true
        });
    }

    pub fn snapshot(&self) -> ReceivedInvites {
        self.state.borrow().clone()
    }

    /// Waits for a revision other than `after_revision`; disconnection wakes
    /// every wait.
    pub async fn next(&self, after_revision: u64) -> ReceivedInvites {
        match obelisk_core::watch::next_matching(self.state.subscribe(), |state| {
            state.revision != after_revision || !state.connected
        })
        .await
        {
            Ok(state) | Err(state) => state,
        }
    }

    pub fn disconnect(&self) {
        self.state.send_if_modified(|state| {
            if !state.connected {
                return false;
            }
            state.connected = false;
            state.revision += 1;
            true
        });
    }
}

/// A person's SteamID: account type Individual, in a real universe, with an
/// account number.
fn individual(steam_id: u64) -> bool {
    (steam_id >> 52) & 0xf == 1 && steam_id >> 56 != 0 && steam_id as u32 != 0
}

/// Wire bodies for debug fixtures, so tests feed the production decoders.
#[cfg(debug_assertions)]
pub mod fixture {
    use prost::Message;

    use crate::proto;

    /// A ClientChatInvite into `lobby` of `app_id`, from `inviter`.
    pub fn lobby_invite(inviter: u64, lobby: u64, app_id: u32) -> Vec<u8> {
        proto::ChatInvite {
            steam_id_invited: 0,
            steam_id_chat: lobby,
            steam_id_patron: inviter,
            chatroom_type: super::LOBBY_ROOM,
            game_id: u64::from(app_id),
        }
        .encode_to_vec()
    }

    /// A ClientInviteToGame from `inviter`, its routing AppID apart.
    pub fn connect_invite(inviter: u64, connect: &str) -> Vec<u8> {
        proto::InviteToGame {
            steam_id_dest: 0,
            steam_id_src: inviter,
            connect_string: connect.into(),
            remote_play: String::new(),
        }
        .encode_to_vec()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FRIEND: u64 = 76_561_197_960_265_729;
    const OTHER: u64 = FRIEND + 1;
    const LOBBY: u64 = 109_775_244_421_126_053;
    const PEAK: u32 = 3_527_290;

    fn lobby(lobby_id: u64) -> GameInvite {
        GameInvite::Lobby { lobby_id }
    }

    fn connect(connect: &str) -> GameInvite {
        GameInvite::Connect {
            connect: connect.into(),
        }
    }

    fn message(friend: u64, entry_type: i32, text: &str, local_echo: bool) -> Vec<u8> {
        crate::chat::fixture::incoming_message(
            friend,
            entry_type,
            text,
            1_700_000_000,
            1,
            local_echo,
        )
    }

    #[test]
    fn chat_reads_valves_invitation_tags() {
        assert_eq!(
            chat_invite(&format!(
                "[lobbyinvite appid=\"{PEAK}\" lobbyid=\"{LOBBY}\"][/lobbyinvite]"
            )),
            Some((PEAK, lobby(LOBBY)))
        );
        // Bare values and any argument order, and text around the tag.
        assert_eq!(
            chat_invite(&format!(
                "hey [LobbyInvite lobbyid={LOBBY} appid={PEAK}]join[/lobbyinvite]"
            )),
            Some((PEAK, lobby(LOBBY)))
        );
        assert_eq!(
            chat_invite(
                r#"[gameinvite appid="620" connect="+connect \"10.0.0.1:27015\" ]x"][/gameinvite]"#
            ),
            Some((620, connect(r#"+connect "10.0.0.1:27015" ]x"#)))
        );
        for (text, why) in [
            (
                format!("\\[lobbyinvite appid=\"{PEAK}\" lobbyid=\"{LOBBY}\"]"),
                "escaped",
            ),
            (
                format!("[lobbyinvite appid=\"0\" lobbyid=\"{LOBBY}\"]"),
                "no app",
            ),
            (format!("[lobbyinvite lobbyid=\"{LOBBY}\"]"), "app missing"),
            (
                format!("[lobbyinvite appid=\"{PEAK}\" lobbyid=\"{FRIEND}\"]"),
                "not a lobby",
            ),
            (
                "[gameinvite appid=\"620\" connect=\"\"]".into(),
                "empty connect",
            ),
            (
                "[gameinvite appid=\"620\" connect=\"\" remoteplay=\"abc\"]".into(),
                "remote play",
            ),
            (
                format!("[invite appid=\"{PEAK}\" lobbyid=\"{LOBBY}\"]"),
                "another tag",
            ),
            ("just text".into(), "no tag"),
        ] {
            assert_eq!(chat_invite(&text), None, "{why}");
        }
        let long = "a".repeat(256);
        assert_eq!(
            chat_invite(&format!("[gameinvite appid=\"620\" connect=\"{long}\"]")),
            None,
            "too long"
        );
    }

    #[test]
    fn arguments_follow_valves_reader() {
        assert_eq!(arguments("=https://x"), [("".into(), "https://x".into())]);
        assert_eq!(
            arguments(" a=1  b=\"two words\" c"),
            [
                ("a".into(), "1".into()),
                ("b".into(), "two words".into()),
                ("c".into(), String::new())
            ]
        );
        assert_eq!(arguments(" =x"), []);
        assert_eq!(
            arguments(r#" k="a\\b\"c""#),
            [("k".into(), r#"a\b"c"#.into())]
        );
    }

    #[test]
    fn every_channel_brings_the_same_invitation_once() {
        let source = InviteSource::new();
        let text = format!("[lobbyinvite appid=\"{PEAK}\" lobbyid=\"{LOBBY}\"][/lobbyinvite]");
        source.receive_notification(chat::INCOMING_MESSAGE, &message(FRIEND, 1, &text, false));
        source.receive_packet(
            CHAT_INVITE,
            None,
            &fixture::lobby_invite(FRIEND, LOBBY, PEAK),
        );
        let invites = source.snapshot();
        assert_eq!(invites.revision, 2);
        assert_eq!(invites.invites.len(), 1);
        assert_eq!(invites.invites[0].friend, FRIEND);
        assert_eq!(invites.invites[0].app_id, PEAK);
        assert_eq!(invites.invites[0].invite, lobby(LOBBY));

        source.receive_packet(
            INVITE_TO_GAME,
            Some(620),
            &fixture::connect_invite(OTHER, "+connect 1"),
        );
        let invites = source.snapshot();
        assert_eq!(invites.revision, 3);
        assert_eq!(invites.invites[0].friend, OTHER);
        assert_eq!(invites.invites[0].app_id, 620);
        assert_eq!(invites.invites[0].invite, connect("+connect 1"));
        assert_eq!(invites.invites[1].friend, FRIEND);
    }

    #[test]
    fn a_friends_newer_invitation_replaces_the_older() {
        let source = InviteSource::new();
        source.receive_packet(
            CHAT_INVITE,
            None,
            &fixture::lobby_invite(FRIEND, LOBBY, PEAK),
        );
        source.receive_packet(
            INVITE_TO_GAME,
            Some(620),
            &fixture::connect_invite(FRIEND, "+connect 1"),
        );
        let invites = source.snapshot();
        assert_eq!(invites.invites.len(), 1);
        assert_eq!(invites.invites[0].invite, connect("+connect 1"));
    }

    #[test]
    fn what_is_no_invitation_from_a_friend_is_ignored() {
        let source = InviteSource::new();
        let text = format!("[lobbyinvite appid=\"{PEAK}\" lobbyid=\"{LOBBY}\"][/lobbyinvite]");
        // The account's own invitation, echoed; typing; another service.
        source.receive_notification(chat::INCOMING_MESSAGE, &message(FRIEND, 1, &text, true));
        source.receive_notification(chat::INCOMING_MESSAGE, &message(FRIEND, 2, &text, false));
        source.receive_notification(chat::ACK_ECHO, &message(FRIEND, 1, &text, false));
        source.receive_notification(chat::INCOMING_MESSAGE, &[0xff]);
        // A chat room invitation, a mod's lobby, a lobby from no one.
        let mut room =
            proto::ChatInvite::decode(&fixture::lobby_invite(FRIEND, LOBBY, PEAK)[..]).unwrap();
        room.chatroom_type = 1;
        source.receive_packet(CHAT_INVITE, None, &room.encode_to_vec());
        let mut modded = room.clone();
        modded.chatroom_type = LOBBY_ROOM;
        modded.game_id |= 1 << 32;
        source.receive_packet(CHAT_INVITE, None, &modded.encode_to_vec());
        source.receive_packet(
            CHAT_INVITE,
            None,
            &fixture::lobby_invite(LOBBY, LOBBY, PEAK),
        );
        // A connect invitation without its game, or for Remote Play.
        source.receive_packet(INVITE_TO_GAME, None, &fixture::connect_invite(FRIEND, "+x"));
        let mut remote =
            proto::InviteToGame::decode(&fixture::connect_invite(FRIEND, "")[..]).unwrap();
        remote.remote_play = "token".into();
        source.receive_packet(INVITE_TO_GAME, Some(620), &remote.encode_to_vec());
        source.receive_packet(766, Some(620), &fixture::connect_invite(FRIEND, "+x"));
        assert_eq!(source.snapshot().revision, 1);
        assert!(source.snapshot().invites.is_empty());
    }

    #[tokio::test]
    async fn observers_wake_on_invitations_and_disconnect() {
        let source = InviteSource::new();
        let waiting = tokio::spawn({
            let source = source.clone();
            async move { source.next(1).await }
        });
        tokio::task::yield_now().await;
        source.receive_packet(
            CHAT_INVITE,
            None,
            &fixture::lobby_invite(FRIEND, LOBBY, PEAK),
        );
        assert_eq!(waiting.await.unwrap().invites.len(), 1);
        let waiting = tokio::spawn({
            let source = source.clone();
            async move { source.next(2).await }
        });
        tokio::task::yield_now().await;
        source.disconnect();
        assert!(!waiting.await.unwrap().connected);
        // Nothing arrives once disconnected.
        source.receive_packet(
            INVITE_TO_GAME,
            Some(620),
            &fixture::connect_invite(OTHER, "+x"),
        );
        assert_eq!(source.snapshot().invites.len(), 1);
    }
}
