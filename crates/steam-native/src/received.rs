//! What Steam tells the account while its game runs, which Steam's overlay
//! would show: invitations from friends and purchases to confirm. Layouts
//! from the pinned engine's posting sites:
//!
//! - `LobbyInvite_t` (503, 24 bytes), posted at 0x6089fc through 0x5c523c to
//!   every pipe of the user: the inviter at 0, the lobby at 8 and the CGameID
//!   at 16, as Steamworks declares it.
//! - `RichInviteReceived_t` (1010035, internal, 0x10c bytes), posted at
//!   0xab0c78 through 0x5c53b8 to the pipes whose AppID is 0: the sender at
//!   0 (from the message body, 0xab0be4), a u32 at 8 from the message header
//!   (0xab0be0; most likely its routing AppID) and the connect string
//!   copied with `strlcpy(…, 256)` at 0xc (0xab0c50).
//! - `MicroTxnAuthRequestCallback_t` (1020043, internal, 16 bytes), posted at
//!   0x60b3a4 through 0x5c53b8: the order at 0 (0x60b364), the AppID at 8
//!   (0x60b370) and an unknown u32 at 0xc.
//! - `MicroTxnAuthDismissCallback_t` (1020044, 16 bytes), posted at 0xa79a58
//!   once the player answered: the order at 0 and its AppID at 8.
use crate::overlay::{terminated, text, u32_at, u64_at};
use game_runtime::protocol::{Event, individual};

pub const LOBBY_INVITE: (i32, usize) = (503, 24);
pub const RICH_INVITE: (i32, usize) = (1_010_035, 0x10c);
pub const MICROTXN_REQUESTED: (i32, usize) = (1_020_043, 16);
pub const MICROTXN_DISMISSED: (i32, usize) = (1_020_044, 16);
/// Every callback this module reads.
pub const CALLBACKS: [(i32, usize); 4] = [
    LOBBY_INVITE,
    RICH_INVITE,
    MICROTXN_REQUESTED,
    MICROTXN_DISMISSED,
];

/// The event callback `id` tells, for the host of game `app`. A lobby
/// invitation names only that game; anything malformed is ignored.
pub fn event(id: i32, payload: &[u8], app: u32) -> Option<Event> {
    if !CALLBACKS.contains(&(id, payload.len())) {
        return None;
    }
    let event = match id {
        503 => {
            // CGameID: AppID in the low 24 bits, type 0 (app), no mod.
            if app == 0 || u64_at(payload, 16)? != u64::from(app) {
                return None;
            }
            let (inviter, lobby_id) = (u64_at(payload, 0)?, u64_at(payload, 8)?);
            if !individual(inviter) || !game_runtime::protocol::lobby(lobby_id) {
                return None;
            }
            Event::LobbyInviteReceived {
                inviter,
                lobby_id,
                app_id: app,
            }
        }
        1_010_035 => {
            let (friend, app_id) = (u64_at(payload, 0)?, u32_at(payload, 8)?);
            let connect = text(terminated(payload.get(0xc..0x10c)?)?)?;
            if !individual(friend) || app_id == 0 || !game_runtime::protocol::connect(&connect) {
                return None;
            }
            Event::RichInviteReceived {
                friend,
                app_id,
                connect,
            }
        }
        1_020_043 => {
            let (order_id, app_id) = (u64_at(payload, 0)?, u32_at(payload, 8)?);
            if order_id == 0 || app_id == 0 {
                return None;
            }
            Event::MicroTxnRequested { order_id, app_id }
        }
        _ => {
            let order_id = u64_at(payload, 0)?;
            if order_id == 0 {
                return None;
            }
            Event::MicroTxnDismissed { order_id }
        }
    };
    Some(event)
}

#[cfg(test)]
mod tests {
    use super::*;

    const PEAK: u32 = 3_527_290;
    const LOBBY: u64 = 109_775_244_421_126_053;
    const FRIEND: u64 = 76_561_197_960_265_729;

    fn lobby_invite(inviter: u64, lobby: u64, game: u64) -> Vec<u8> {
        [inviter, lobby, game]
            .iter()
            .flat_map(|field| field.to_le_bytes())
            .collect()
    }

    fn rich_invite(friend: u64, app: u32, connect: &[u8]) -> Vec<u8> {
        let mut payload = vec![0; 0x10c];
        payload[..8].copy_from_slice(&friend.to_le_bytes());
        payload[8..12].copy_from_slice(&app.to_le_bytes());
        payload[0xc..0xc + connect.len()].copy_from_slice(connect);
        payload
    }

    fn microtxn(order: u64, app: u32, unknown: u32) -> Vec<u8> {
        [
            &order.to_le_bytes()[..],
            &app.to_le_bytes(),
            &unknown.to_le_bytes(),
        ]
        .concat()
    }

    #[test]
    fn a_lobby_invitation_names_a_lobby_of_this_game() {
        let invite = lobby_invite(FRIEND, LOBBY, u64::from(PEAK));
        assert_eq!(
            event(503, &invite, PEAK),
            Some(Event::LobbyInviteReceived {
                inviter: FRIEND,
                lobby_id: LOBBY,
                app_id: PEAK
            })
        );
        assert_eq!(event(503, &invite[..23], PEAK), None, "short");
        assert_eq!(event(503, &invite, 0), None, "no game");
        assert_eq!(
            event(503, &lobby_invite(FRIEND, LOBBY, 730), PEAK),
            None,
            "another game"
        );
        assert_eq!(
            event(
                503,
                &lobby_invite(FRIEND, LOBBY, u64::from(PEAK) | 1 << 32),
                PEAK
            ),
            None,
            "a mod"
        );
        assert_eq!(
            event(503, &lobby_invite(LOBBY, LOBBY, u64::from(PEAK)), PEAK),
            None,
            "a lobby invites no one"
        );
        assert_eq!(
            event(503, &lobby_invite(FRIEND, FRIEND, u64::from(PEAK)), PEAK),
            None,
            "an account is no lobby"
        );
    }

    #[test]
    fn a_rich_invitation_carries_its_connect_string() {
        assert_eq!(
            event(
                1_010_035,
                &rich_invite(FRIEND, 620, b"+connect 10.0.0.1"),
                PEAK
            ),
            Some(Event::RichInviteReceived {
                friend: FRIEND,
                app_id: 620,
                connect: "+connect 10.0.0.1".into()
            }),
            "another game's too: the supervisor keeps its own"
        );
        assert_eq!(
            event(1_010_035, &rich_invite(FRIEND, 620, &[b'a'; 255]), PEAK),
            Some(Event::RichInviteReceived {
                friend: FRIEND,
                app_id: 620,
                connect: "a".repeat(255)
            })
        );
        for (payload, why) in [
            (rich_invite(FRIEND, 620, b""), "empty"),
            (rich_invite(FRIEND, 620, &[b'a'; 256]), "unterminated"),
            (rich_invite(FRIEND, 620, &[0xff, 0xfe]), "not text"),
            (rich_invite(FRIEND, 0, b"+x"), "no game"),
            (rich_invite(LOBBY, 620, b"+x"), "from no one"),
        ] {
            assert_eq!(event(1_010_035, &payload, PEAK), None, "{why}");
        }
        let short = rich_invite(FRIEND, 620, b"+x");
        assert_eq!(event(1_010_035, &short[..0x10b], PEAK), None);
    }

    #[test]
    fn a_purchase_is_asked_then_dismissed_by_its_order() {
        assert_eq!(
            event(1_020_043, &microtxn(4242, 620, 7), PEAK),
            Some(Event::MicroTxnRequested {
                order_id: 4242,
                app_id: 620
            })
        );
        assert_eq!(event(1_020_043, &microtxn(0, 620, 0), PEAK), None);
        assert_eq!(event(1_020_043, &microtxn(4242, 0, 0), PEAK), None);
        assert_eq!(
            event(1_020_044, &microtxn(4242, 620, 0), PEAK),
            Some(Event::MicroTxnDismissed { order_id: 4242 })
        );
        assert_eq!(event(1_020_044, &microtxn(0, 620, 0), PEAK), None);
        assert_eq!(event(1_020_044, &microtxn(4242, 620, 0)[..12], PEAK), None);
        assert_eq!(
            event(331, &[0; 12], PEAK),
            None,
            "the host's own GameOverlayActivated_t echo"
        );
    }
}
