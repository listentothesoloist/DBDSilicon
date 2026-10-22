//! The lobby the account's own engine shows its game in. Steam may never
//! publish a private lobby in the persona other sessions see, but the engine
//! hosting the game is the lobby's member. Two sources tell it:
//!
//! - The pinned engine's IClientFriends slot 23 `GetFriendGamePlayed` (proxy
//!   0x8b4074: this, CSteamID by value, a 24-byte out buffer, bool result;
//!   server 0xa8f3d4) fills `FriendGameInfo_t`: the CGameID at 0, the game
//!   server's address at 8 and the lobby's CSteamID at 16. For the account
//!   itself, that lobby is only the one Steam's persona carries (written by
//!   the persona-state job alone, 0x6068f0), so never a private lobby.
//! - The lobby the game named in its invite dialog (callback 1010001), kept
//!   while the engine's matchmaking shows the account in it
//!   (`Engine::in_lobby`), and forgotten once it does not.
use std::time::{Duration, Instant};

/// `FriendGameInfo_t`'s size.
pub const SIZE: usize = 24;
/// How often the host reads it while the game runs.
pub const INTERVAL: Duration = Duration::from_secs(2);

/// The lobby of game `app` in `info`, if the engine shows the account in
/// one. Another game, a mod or shortcut game id, or anything but a Chat
/// SteamID is no lobby of this game.
pub fn current(info: &[u8; SIZE], app: u32) -> Option<u64> {
    let game = u64::from_le_bytes(info[..8].try_into().ok()?);
    // CGameID: AppID in the low 24 bits, type 0 (app) above, no mod.
    if app == 0 || game != u64::from(app) {
        return None;
    }
    let lobby = u64::from_le_bytes(info[16..24].try_into().ok()?);
    game_runtime::protocol::lobby(lobby).then_some(lobby)
}

/// The game's lobby from both sources, read on the follower's interval.
pub struct Lobbies {
    follower: Follower,
    named: Option<u64>,
    /// Each source stops for good once the engine refuses its ABI; the other
    /// goes on.
    reads_played: bool,
    checks_named: bool,
}
impl Default for Lobbies {
    fn default() -> Self {
        Self::new()
    }
}
impl Lobbies {
    pub fn new() -> Self {
        Self {
            follower: Follower::new(),
            named: None,
            reads_played: true,
            checks_named: true,
        }
    }
    /// The game named `lobby` in its invite dialog: it replaces any lobby
    /// named before, and the engine is read at once.
    pub fn named(&mut self, lobby: u64) {
        if game_runtime::protocol::lobby(lobby) {
            self.named = Some(lobby);
            self.follower.next = None;
        }
    }
    /// True when the engine is due another read.
    pub fn due(&self, now: Instant) -> bool {
        self.follower.due(now)
    }
    /// Reads the engine at `now`: `played` is slot 23's lobby, `member`
    /// whether the account is in a lobby the game named (`None` when it could
    /// not be asked). The lobby to tell when it changed.
    ///
    /// Slot 23's lobby comes first and spares the matchmaking question. A
    /// named lobby is told only while the engine shows the account in it;
    /// once it does not, it is forgotten, so a lobby the game left is never
    /// told again, even if the game stays in its title screen.
    pub fn read(
        &mut self,
        now: Instant,
        played: impl FnOnce() -> crate::Result<Option<u64>>,
        member: impl FnOnce(u64) -> crate::Result<Option<bool>>,
    ) -> Option<Option<u64>> {
        let played = if self.reads_played {
            played().unwrap_or_else(|_| {
                self.reads_played = false;
                None
            })
        } else {
            None
        };
        let current = played.or_else(|| self.confirmed(member));
        self.follower.read(now, current)
    }
    fn confirmed(
        &mut self,
        member: impl FnOnce(u64) -> crate::Result<Option<bool>>,
    ) -> Option<u64> {
        let lobby = self.named.filter(|_| self.checks_named)?;
        match member(lobby) {
            Ok(Some(true)) => Some(lobby),
            // Unknown this time: not told, still asked next time.
            Ok(None) => None,
            Ok(Some(false)) => {
                self.named = None;
                None
            }
            Err(_) => {
                self.checks_named = false;
                self.named = None;
                None
            }
        }
    }
    /// The engine lost Steam: the next read is told whatever it shows.
    pub fn forget(&mut self) {
        self.follower.forget();
    }
}

/// What the host last told of the lobby, and when it reads it next.
pub struct Follower {
    told: Option<Option<u64>>,
    next: Option<Instant>,
}
impl Default for Follower {
    fn default() -> Self {
        Self::new()
    }
}
impl Follower {
    pub fn new() -> Self {
        Self {
            told: None,
            next: None,
        }
    }
    /// True when the engine is due another read.
    pub fn due(&self, now: Instant) -> bool {
        self.next.is_none_or(|next| now >= next)
    }
    /// Takes a read at `now`; the lobby to tell when it changed.
    pub fn read(&mut self, now: Instant, lobby: Option<u64>) -> Option<Option<u64>> {
        self.next = Some(now + INTERVAL);
        (self.told != Some(lobby)).then(|| {
            self.told = Some(lobby);
            lobby
        })
    }
    /// The engine lost Steam: the next read is told whatever it shows.
    pub fn forget(&mut self) {
        self.told = None;
        self.next = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PEAK: u32 = 3_527_290;
    const LOBBY: u64 = 109_775_244_421_126_053;

    /// The bytes the engine's server writes for a game in a lobby.
    fn info(game: u64, lobby: u64) -> [u8; SIZE] {
        let mut info = [0; SIZE];
        info[..8].copy_from_slice(&game.to_le_bytes());
        info[8..12].copy_from_slice(&0x0a00_0001_u32.to_le_bytes());
        info[12..14].copy_from_slice(&27015_u16.to_le_bytes());
        info[16..].copy_from_slice(&lobby.to_le_bytes());
        info
    }

    #[test]
    fn the_games_lobby_is_its_chat_steam_id() {
        assert_eq!(current(&info(u64::from(PEAK), LOBBY), PEAK), Some(LOBBY));
        assert_eq!(current(&info(u64::from(PEAK), 0), PEAK), None, "no lobby");
        assert_eq!(
            current(&info(u64::from(PEAK), 76_561_197_960_265_729), PEAK),
            None,
            "an account is no lobby"
        );
        assert_eq!(current(&info(730, LOBBY), PEAK), None, "another game");
        assert_eq!(
            current(&info(u64::from(PEAK) | 2 << 24, LOBBY), PEAK),
            None,
            "a shortcut's game id"
        );
        assert_eq!(
            current(&info(u64::from(PEAK) | 1 << 32, LOBBY), PEAK),
            None,
            "a mod"
        );
        assert_eq!(current(&info(0, LOBBY), 0), None, "no game");
        assert_eq!(current(&[0; SIZE], PEAK), None, "the engine's empty answer");
    }

    const ALICE: u64 = 76_561_197_960_265_729;
    const NEXT: u64 = 109_775_244_423_207_541;

    fn none() -> crate::Result<Option<u64>> {
        Ok(None)
    }

    fn unasked(_: u64) -> crate::Result<Option<bool>> {
        panic!("the named lobby is not asked")
    }

    #[test]
    fn a_named_lobby_is_told_while_the_engine_shows_the_account_in_it() {
        let mut lobbies = Lobbies::new();
        let start = Instant::now();
        assert_eq!(lobbies.read(start, none, unasked), Some(None), "none named");
        lobbies.named(LOBBY);
        assert!(lobbies.due(start), "a named lobby is read at once");
        let asked = std::cell::Cell::new(0);
        assert_eq!(
            lobbies.read(start, none, |lobby| {
                assert_eq!(lobby, LOBBY);
                Ok(Some(true))
            }),
            Some(Some(LOBBY))
        );
        assert_eq!(
            lobbies.read(start + INTERVAL, none, |_| {
                asked.set(asked.get() + 1);
                Ok(Some(true))
            }),
            None,
            "still in it"
        );
        assert_eq!(asked.get(), 1);
        // The game left its lobby, still running: told, and never asked again.
        assert_eq!(
            lobbies.read(start + INTERVAL * 2, none, |_| Ok(Some(false))),
            Some(None)
        );
        assert_eq!(lobbies.read(start + INTERVAL * 3, none, unasked), None);
        // Its next lobby, once named.
        lobbies.named(NEXT);
        assert_eq!(
            lobbies.read(start + INTERVAL * 3, none, |lobby| Ok(Some(lobby == NEXT))),
            Some(Some(NEXT))
        );
    }

    #[test]
    fn a_new_name_replaces_the_last_and_anything_but_a_lobby_is_ignored() {
        let mut lobbies = Lobbies::new();
        let start = Instant::now();
        lobbies.named(LOBBY);
        lobbies.named(ALICE);
        lobbies.named(0);
        assert_eq!(
            lobbies.read(start, none, |lobby| Ok(Some(lobby == LOBBY))),
            Some(Some(LOBBY))
        );
        lobbies.named(NEXT);
        assert_eq!(
            lobbies.read(start, none, |lobby| Ok(Some(lobby == LOBBY))),
            Some(None),
            "the dialog moved on to a lobby the account is not in"
        );
        assert_eq!(lobbies.read(start + INTERVAL, none, unasked), None);
    }

    #[test]
    fn an_unanswered_question_tells_nothing_and_asks_again() {
        let mut lobbies = Lobbies::new();
        let start = Instant::now();
        lobbies.named(LOBBY);
        assert_eq!(
            lobbies.read(start, none, |_| Ok(Some(true))),
            Some(Some(LOBBY))
        );
        assert_eq!(
            lobbies.read(start + INTERVAL, none, |_| Ok(None)),
            Some(None),
            "an unconfirmed lobby is not told"
        );
        assert_eq!(
            lobbies.read(start + INTERVAL * 2, none, |_| Ok(Some(true))),
            Some(Some(LOBBY)),
            "but still asked"
        );
    }

    #[test]
    fn the_personas_lobby_comes_first() {
        let mut lobbies = Lobbies::new();
        let start = Instant::now();
        lobbies.named(LOBBY);
        assert_eq!(
            lobbies.read(start, || Ok(Some(NEXT)), unasked),
            Some(Some(NEXT))
        );
        assert_eq!(
            lobbies.read(start + INTERVAL, none, |_| Ok(Some(true))),
            Some(Some(LOBBY)),
            "the named lobby once slot 23 shows none"
        );
    }

    #[test]
    fn a_refused_source_stops_alone() {
        let mut lobbies = Lobbies::new();
        let start = Instant::now();
        lobbies.named(LOBBY);
        assert_eq!(
            lobbies.read(start, || Err("refused".into()), |_| Ok(Some(true))),
            Some(Some(LOBBY)),
            "slot 23 refused, the named lobby still told"
        );
        assert_eq!(
            lobbies.read(
                start + INTERVAL,
                || panic!("slot 23 is not read again"),
                |_| Ok(Some(true))
            ),
            None
        );
        assert_eq!(
            lobbies.read(
                start + INTERVAL * 2,
                || panic!("slot 23 is not read again"),
                |_| Err("refused".into())
            ),
            Some(None),
            "the check refused: nothing unconfirmed is told"
        );
        lobbies.named(NEXT);
        assert_eq!(
            lobbies.read(
                start + INTERVAL * 3,
                || panic!("slot 23 is not read again"),
                unasked
            ),
            None,
            "no named lobby is asked about again"
        );
    }

    #[test]
    fn after_a_disconnection_the_lobby_is_told_again() {
        let mut lobbies = Lobbies::new();
        let start = Instant::now();
        lobbies.named(LOBBY);
        assert_eq!(
            lobbies.read(start, none, |_| Ok(Some(true))),
            Some(Some(LOBBY))
        );
        lobbies.forget();
        assert!(lobbies.due(start));
        assert_eq!(
            lobbies.read(start, none, |_| Ok(Some(true))),
            Some(Some(LOBBY))
        );
    }

    #[test]
    fn the_follower_reads_on_its_interval_and_tells_changes_once() {
        let mut follower = Follower::new();
        let start = Instant::now();
        assert!(follower.due(start));
        assert_eq!(follower.read(start, None), Some(None), "the first read");
        assert!(!follower.due(start + INTERVAL - Duration::from_millis(1)));
        assert!(follower.due(start + INTERVAL));
        assert_eq!(follower.read(start + INTERVAL, None), None);
        assert_eq!(
            follower.read(start + INTERVAL * 2, Some(LOBBY)),
            Some(Some(LOBBY))
        );
        assert_eq!(follower.read(start + INTERVAL * 3, Some(LOBBY)), None);
        follower.forget();
        assert!(follower.due(start + INTERVAL * 3));
        assert_eq!(
            follower.read(start + INTERVAL * 3, Some(LOBBY)),
            Some(Some(LOBBY)),
            "after a disconnection, told again"
        );
        assert_eq!(follower.read(start + INTERVAL * 4, None), Some(None));
    }
}
