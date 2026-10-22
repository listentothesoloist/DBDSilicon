//! What a game asks of Steam's overlay. Without an overlay, Steam's engine
//! still turns every `ISteamFriends::ActivateGameOverlay*` call into the
//! internal callback `GameOverlayActivateRequested_t` (1010001), posted to
//! every pipe of the user whose AppID is 0 (0x5c53b8), this host's global
//! pipe included. Obelisk shows its own overlay instead.
//!
//! Layout from the pinned engine's builders, CUserFriends (vtable 0x175e868)
//! slots 184 to 190 and the Remote Play Together dialog (research report
//! docs/research/steam-chat-invites-2026-10-08.md, "Bouton Inviter dans le
//! jeu, sans overlay"). Each zeroes 0x211c bytes, copies its dialog name with
//! `strlcpy(…, 0x2000)` at 0, and stores the requesting pipe's AppID at
//! 0x2118:
//!
//! - slot 184 `ActivateGameOverlay(dialog)` (0xa9d2e8) is slot 185 with
//!   CSteamID 0;
//! - slot 185 `ActivateGameOverlayToUser(dialog, steam_id)` (0xa9d2f8): the
//!   CSteamID at 0x2000;
//! - slot 186 `ActivateGameOverlayToWebPage(url, mode)` (0xa9d4c4): the URL as
//!   the dialog, the byte 1 at 0x200c and the mode at 0x2014;
//! - slot 187 `ActivateGameOverlayToStore(app, flag)` (0xa9d3dc): "store", the
//!   AppID at 0x2008 and the flag at 0x2010;
//! - slot 188 `ActivateGameOverlayInviteDialog(lobby)` (0xa9d8d8):
//!   "lobbyinvite", the lobby at 0x2000;
//! - slot 189 `ActivateGameOverlayRemotePlayTogetherInviteDialog(lobby)`
//!   (0xa9d9b8): "remoteplaytogetherinvite", the lobby at 0x2000;
//! - slot 190 `ActivateGameOverlayInviteDialogConnectString(connect)`
//!   (0xa9da98): "lobbyinviteconnectstring", the caller's AppID at 0x2008 and
//!   the connect string at 0x2018;
//! - 0x9ba544: "remoteplaytogether", nothing else.
//!
//! Only slot 186 writes the byte at 0x200c, which tells a web page from a
//! dialog whose name looks like a URL.
use game_runtime::protocol::OverlayDialog;
use std::time::{Duration, Instant};

/// `GameOverlayActivateRequested_t`.
pub const REQUESTED: i32 = 1_010_001;
/// The callback's exact size; any other size is another layout.
pub const SIZE: usize = 0x211c;
const DIALOG: std::ops::Range<usize> = 0..0x2000;
const TARGET: usize = 0x2000;
const APP: usize = 0x2008;
const WEB_PAGE: usize = 0x200c;
const STORE_FLAG: usize = 0x2010;
const WEB_MODE: usize = 0x2014;
const CONNECT: std::ops::Range<usize> = 0x2018..0x2118;
const REQUESTER: usize = 0x2118;

/// What a request of game `app` asks the overlay for. Another game's
/// request and anything malformed or beyond `OverlayDialog::valid` is
/// ignored.
pub fn request(payload: &[u8], app: u32) -> Option<OverlayDialog> {
    if payload.len() != SIZE || app == 0 || u32_at(payload, REQUESTER)? != app {
        return None;
    }
    let name = terminated(&payload[DIALOG])?;
    let target = u64_at(payload, TARGET)?;
    let request = if payload[WEB_PAGE] == 1 {
        OverlayDialog::WebPage {
            url: text(name)?,
            modal: match u32_at(payload, WEB_MODE)? {
                0 => false,
                1 => true,
                _ => return None,
            },
        }
    } else {
        match name {
            b"lobbyinvite" => OverlayDialog::LobbyInvite { lobby_id: target },
            b"remoteplaytogetherinvite" => OverlayDialog::RemotePlayTogetherInvite {
                lobby_id: game_runtime::protocol::lobby(target).then_some(target),
            },
            b"lobbyinviteconnectstring" if u32_at(payload, APP)? == app => {
                OverlayDialog::ConnectInvite {
                    connect: text(terminated(&payload[CONNECT])?)?,
                }
            }
            b"lobbyinviteconnectstring" => return None,
            b"remoteplaytogether" => OverlayDialog::RemotePlayTogether {},
            b"store" => OverlayDialog::Store {
                app_id: u32_at(payload, APP)?,
                flag: u32_at(payload, STORE_FLAG)?,
            },
            _ if target == 0 => OverlayDialog::Dialog {
                dialog: text(name)?,
            },
            _ => OverlayDialog::User {
                dialog: text(name)?,
                steam_id: target,
            },
        }
    };
    request.valid().then_some(request)
}

pub(crate) fn u32_at(payload: &[u8], offset: usize) -> Option<u32> {
    Some(u32::from_le_bytes(
        payload.get(offset..offset + 4)?.try_into().ok()?,
    ))
}

pub(crate) fn u64_at(payload: &[u8], offset: usize) -> Option<u64> {
    Some(u64::from_le_bytes(
        payload.get(offset..offset + 8)?.try_into().ok()?,
    ))
}

/// The C string at the start of `field`, which must end inside it.
pub(crate) fn terminated(field: &[u8]) -> Option<&[u8]> {
    field
        .iter()
        .position(|byte| *byte == 0)
        .map(|end| &field[..end])
}

pub(crate) fn text(bytes: &[u8]) -> Option<String> {
    std::str::from_utf8(bytes).ok().map(str::to_owned)
}

/// Coalesces a repeated request: a held key or a double press makes one
/// overlay. Another request always goes through.
pub struct Throttle<T> {
    interval: Duration,
    last: Option<(T, Instant)>,
}
impl<T: PartialEq> Throttle<T> {
    pub fn new(interval: Duration) -> Self {
        Self {
            interval,
            last: None,
        }
    }
    pub fn admit(&mut self, request: T, now: Instant) -> bool {
        if self.last.as_ref().is_some_and(|(last, at)| {
            *last == request && now.saturating_duration_since(*at) < self.interval
        }) {
            return false;
        }
        self.last = Some((request, now));
        true
    }
}

/// At most `limit` events per `window`: a flood the engine relays can never
/// fill the supervisor's bounded event queue.
pub struct Budget {
    limit: u32,
    window: Duration,
    started: Option<Instant>,
    spent: u32,
}
impl Budget {
    pub fn new(limit: u32, window: Duration) -> Self {
        Self {
            limit,
            window,
            started: None,
            spent: 0,
        }
    }
    pub fn admit(&mut self, now: Instant) -> bool {
        if self
            .started
            .is_none_or(|started| now.saturating_duration_since(started) >= self.window)
        {
            self.started = Some(now);
            self.spent = 0;
        }
        if self.spent >= self.limit {
            return false;
        }
        self.spent += 1;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PEAK: u32 = 3_527_290;
    const LOBBY: u64 = 109_775_244_421_126_053;
    const FRIEND: u64 = 76_561_197_960_265_729;

    /// The bytes a builder writes: memset, dialog name, target, requester.
    fn dialog_request(dialog: &[u8], target: u64, requester: u32) -> Vec<u8> {
        let mut payload = vec![0; SIZE];
        payload[..dialog.len()].copy_from_slice(dialog);
        payload[0x2000..0x2008].copy_from_slice(&target.to_le_bytes());
        payload[0x2118..].copy_from_slice(&requester.to_le_bytes());
        payload
    }

    /// The bytes slot 190 builds: its dialog, the caller's AppID and string.
    fn connect_request(connect: &[u8], app: u32, requester: u32) -> Vec<u8> {
        let mut payload = dialog_request(b"lobbyinviteconnectstring", 0, requester);
        payload[0x2008..0x200c].copy_from_slice(&app.to_le_bytes());
        payload[0x2018..0x2018 + connect.len()].copy_from_slice(connect);
        payload
    }

    /// The bytes slot 186 builds: the URL, its marker and its mode.
    fn web_request(url: &[u8], mode: u32, requester: u32) -> Vec<u8> {
        let mut payload = dialog_request(url, 0, requester);
        payload[0x200c] = 1;
        payload[0x2014..0x2018].copy_from_slice(&mode.to_le_bytes());
        payload
    }

    /// The bytes slot 187 builds: "store", the AppID and the flag.
    fn store_request(app: u32, flag: u32, requester: u32) -> Vec<u8> {
        let mut payload = dialog_request(b"store", 0, requester);
        payload[0x2008..0x200c].copy_from_slice(&app.to_le_bytes());
        payload[0x2010..0x2014].copy_from_slice(&flag.to_le_bytes());
        payload
    }

    #[test]
    fn every_dialog_kind_is_read_from_its_builders_bytes() {
        let dialog = |name: &str| OverlayDialog::Dialog {
            dialog: name.into(),
        };
        for name in [
            "friends",
            "community",
            "players",
            "settings",
            "officialgamegroup",
            "stats",
            "achievements",
            "chatroomgroup/3810571",
        ] {
            assert_eq!(
                request(&dialog_request(name.as_bytes(), 0, PEAK), PEAK),
                Some(dialog(name)),
                "slot 184 {name}"
            );
        }
        for name in ["steamid", "chat", "jointrade", "friendadd", "stats"] {
            assert_eq!(
                request(&dialog_request(name.as_bytes(), FRIEND, PEAK), PEAK),
                Some(OverlayDialog::User {
                    dialog: name.into(),
                    steam_id: FRIEND
                }),
                "slot 185 {name}"
            );
        }
        assert_eq!(
            request(&web_request(b"https://example.com/a?b=c", 0, PEAK), PEAK),
            Some(OverlayDialog::WebPage {
                url: "https://example.com/a?b=c".into(),
                modal: false
            })
        );
        assert_eq!(
            request(&web_request(b"http://example.com", 1, PEAK), PEAK),
            Some(OverlayDialog::WebPage {
                url: "http://example.com".into(),
                modal: true
            })
        );
        assert_eq!(
            request(&store_request(620, 2, PEAK), PEAK),
            Some(OverlayDialog::Store {
                app_id: 620,
                flag: 2
            })
        );
        assert_eq!(
            request(&dialog_request(b"lobbyinvite", LOBBY, PEAK), PEAK),
            Some(OverlayDialog::LobbyInvite { lobby_id: LOBBY })
        );
        assert_eq!(
            request(
                &dialog_request(b"remoteplaytogetherinvite", LOBBY, PEAK),
                PEAK
            ),
            Some(OverlayDialog::RemotePlayTogetherInvite {
                lobby_id: Some(LOBBY)
            })
        );
        assert_eq!(
            request(&dialog_request(b"remoteplaytogetherinvite", 0, PEAK), PEAK),
            Some(OverlayDialog::RemotePlayTogetherInvite { lobby_id: None })
        );
        assert_eq!(
            request(
                &connect_request(b"+connect_lobby 109775244421126053", PEAK, PEAK),
                PEAK
            ),
            Some(OverlayDialog::ConnectInvite {
                connect: "+connect_lobby 109775244421126053".into()
            })
        );
        assert_eq!(
            request(&dialog_request(b"remoteplaytogether", 0, PEAK), PEAK),
            Some(OverlayDialog::RemotePlayTogether {})
        );
    }

    #[test]
    fn other_games_layouts_and_targets_are_ignored() {
        let lobby = dialog_request(b"lobbyinvite", LOBBY, PEAK);
        assert_eq!(request(&lobby[..SIZE - 1], PEAK), None, "short");
        let mut long = lobby.clone();
        long.push(0);
        assert_eq!(request(&long, PEAK), None, "long");
        assert_eq!(request(&lobby, 0), None, "no game");
        assert_eq!(
            request(&dialog_request(b"lobbyinvite", LOBBY, 730), PEAK),
            None,
            "another game's request"
        );
        assert_eq!(
            request(&dialog_request(b"lobbyinvite", FRIEND, PEAK), PEAK),
            None,
            "an account is no lobby"
        );
        assert_eq!(
            request(&dialog_request(b"lobbyinvite", 0, PEAK), PEAK),
            None,
            "no lobby"
        );
        let mut unterminated = lobby.clone();
        unterminated[..0x2000].fill(b'a');
        assert_eq!(request(&unterminated, PEAK), None);
        for name in [&b""[..], b"a b", b"\xff\xfe", b"friends\n"] {
            assert_eq!(request(&dialog_request(name, 0, PEAK), PEAK), None);
        }
        assert_eq!(
            request(&dialog_request(&[b'a'; 129], 0, PEAK), PEAK),
            None,
            "too long a name"
        );
        assert_eq!(request(&store_request(620, 3, PEAK), PEAK), None);
        for (url, mode) in [
            (&b"file:///etc/passwd"[..], 0),
            (b"steam://run/620", 0),
            (b"https://a b", 0),
            (b"https://a\"b", 0),
            (b"https://example.com", 2),
        ] {
            assert_eq!(request(&web_request(url, mode, PEAK), PEAK), None);
        }
        let long_url = [&b"https://"[..], &[b'a'; 2041]].concat();
        assert_eq!(request(&web_request(&long_url, 0, PEAK), PEAK), None);
        let long_url = [&b"https://"[..], &[b'a'; 2040]].concat();
        assert!(request(&web_request(&long_url, 0, PEAK), PEAK).is_some());
    }

    #[test]
    fn connect_strings_must_be_terminated_text_of_the_same_game() {
        assert_eq!(request(&connect_request(b"", PEAK, PEAK), PEAK), None);
        assert_eq!(request(&connect_request(b"+x", 730, PEAK), PEAK), None);
        assert_eq!(
            request(&connect_request(&[0xff, 0xfe], PEAK, PEAK), PEAK),
            None
        );
        assert_eq!(
            request(&connect_request(&[b'a'; 256], PEAK, PEAK), PEAK),
            None,
            "the string fills its buffer without a terminator"
        );
        assert_eq!(
            request(&connect_request(&[b'a'; 255], PEAK, PEAK), PEAK),
            Some(OverlayDialog::ConnectInvite {
                connect: "a".repeat(255)
            })
        );
    }

    #[test]
    fn a_repeated_request_coalesces_another_goes_through() {
        let mut throttle = Throttle::new(Duration::from_millis(500));
        let start = Instant::now();
        assert!(throttle.admit("lobbyinvite", start));
        assert!(!throttle.admit("lobbyinvite", start + Duration::from_millis(499)));
        assert!(throttle.admit("friends", start + Duration::from_millis(499)));
        assert!(throttle.admit("lobbyinvite", start + Duration::from_millis(500)));
        assert!(!throttle.admit("lobbyinvite", start + Duration::from_millis(600)));
    }

    #[test]
    fn a_budget_admits_its_limit_per_window() {
        let mut budget = Budget::new(2, Duration::from_secs(1));
        let start = Instant::now();
        assert!(budget.admit(start));
        assert!(budget.admit(start + Duration::from_millis(10)));
        assert!(!budget.admit(start + Duration::from_millis(999)));
        assert!(budget.admit(start + Duration::from_secs(1)));
    }
}
