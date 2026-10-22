//! The library cache across UniFFI. Each storefront session keeps the library
//! it loads, with everything its storefront returned; Swift reads the last one
//! of an account while its session connects, records launches and forgets an
//! account that signs out.
use crate::{
    EpicGame, GameAccount, GameIdentity, GogGame, SteamGame, SteamGameArtwork,
    game_session::Bridged, installations::Named,
};
use futures_util::{StreamExt, stream};
use game_runtime::{epic, gog, steam};
use obelisk_core::library_cache::{self, AccountLibrary, Entry};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    future::Future,
    marker::PhantomData,
    sync::{Arc, Mutex},
    time::{SystemTime, UNIX_EPOCH},
};

const STEAM: &str = "steam";
const EPIC: &str = "epic";
const GOG: &str = "gog";
/// The entry field that keeps a game's achievement progress.
const ACHIEVEMENTS: &str = "achievements";
/// The entry field that keeps when a Steam game joined the library.
const ADDED_AT: &str = "addedAt";

/// The last library of every storefront account, in the library directory.
#[derive(uniffi::Object)]
pub struct LibraryCache {
    inner: library_cache::LibraryCache,
}

/// One achievement of a game, the same for every storefront.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, uniffi::Record)]
#[serde(rename_all = "camelCase")]
pub struct LibraryAchievement {
    pub id: String,
    pub name: String,
    pub description: String,
    pub icon_url: String,
    pub hidden: bool,
    pub unlocked: bool,
    /// Seconds since the Unix epoch; nil while locked, or when the
    /// storefront did not tell.
    pub unlocked_at: Option<u64>,
}

/// A game's achievements, by the id its storefront reads them with: Steam's
/// app ID, Epic's namespace, GOG's product id.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct GameAchievements {
    pub(crate) id: String,
    pub(crate) achievements: Vec<LibraryAchievement>,
}

/// The share of each of `read`'s games' achievements unlocked, by id; none
/// for a game without any.
pub(crate) fn unlocked_shares(read: Vec<GameAchievements>) -> impl Iterator<Item = (String, f64)> {
    read.into_iter()
        .filter_map(|game| Some((game.id, unlocked_share(&game.achievements)?)))
}

/// A Steam game of the cache, with its platforms, library artwork and what
/// the library orders by once Steam told them.
#[derive(Clone, Debug, PartialEq, uniffi::Record)]
pub struct CachedSteamGame {
    pub game: SteamGame,
    pub platforms: Option<Vec<String>>,
    #[uniffi(default = None)]
    pub artwork: Option<SteamGameArtwork>,
    /// When the game joined the library, in seconds since the Unix epoch.
    #[uniffi(default = None)]
    pub added_at: Option<u64>,
    /// The share of its achievements unlocked when they were last read;
    /// none for a game without any.
    #[uniffi(default = None)]
    pub achievement_share: Option<f64>,
}

/// An Epic game of the cache, with its playtime and achievements once Epic
/// told them.
#[derive(Clone, Debug, PartialEq, uniffi::Record)]
pub struct CachedEpicGame {
    pub game: EpicGame,
    pub playtime_seconds: Option<u64>,
    /// The share of its achievements unlocked when they were last read;
    /// none for a game without any.
    #[uniffi(default = None)]
    pub achievement_share: Option<f64>,
}

/// A GOG game of the cache, with its playtime and achievements once GOG
/// told them.
#[derive(Clone, Debug, PartialEq, uniffi::Record)]
pub struct CachedGogGame {
    pub game: GogGame,
    pub playtime_minutes: Option<u64>,
    /// The share of its achievements unlocked when they were last read;
    /// none for a game without any.
    #[uniffi(default = None)]
    pub achievement_share: Option<f64>,
}

/// When an account launched a game from Obelisk.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct GameLaunch {
    pub game: GameIdentity,
    /// Seconds since the Unix epoch.
    pub at: u64,
}

#[uniffi::export]
impl LibraryCache {
    /// The cache of the library in `directory`. Every cache of one directory
    /// shares one document.
    #[uniffi::constructor]
    pub fn new(directory: String) -> Arc<Self> {
        Arc::new(Self {
            inner: library_cache::LibraryCache::open(directory),
        })
    }

    /// The account's last Steam library, empty when it has none, with each
    /// game's artwork in `language` and what the library orders by.
    pub fn steam_library(&self, steam_id: u64, language: String) -> Vec<CachedSteamGame> {
        // Each field on its own: reading the entry flattened would copy its PICS.
        self.inner.library(STEAM, &steam_id.to_string(), |entry| {
            Some(CachedSteamGame {
                game: SteamGame::deserialize(entry).ok()?,
                platforms: entry
                    .get("platforms")
                    .and_then(|platforms| Vec::deserialize(platforms).ok()),
                artwork: entry.get("pics").map(|pics| {
                    SteamGameArtwork::from(steam_kit::artwork::library_artwork(pics, &language))
                }),
                added_at: entry.get(ADDED_AT).and_then(Value::as_u64),
                achievement_share: kept(entry).and_then(|kept| unlocked_share(&kept.list)),
            })
        })
    }

    /// The account's last Epic library, empty when it has none.
    pub fn epic_library(&self, account_id: String) -> Vec<CachedEpicGame> {
        // Each field on its own: reading the entry flattened would copy its
        // metadata.
        self.inner.library(EPIC, &account_id, |entry| {
            Some(CachedEpicGame {
                game: EpicGame::deserialize(entry).ok()?,
                playtime_seconds: entry.get("playtimeSeconds").and_then(Value::as_u64),
                achievement_share: kept(entry).and_then(|kept| unlocked_share(&kept.list)),
            })
        })
    }

    /// The account's last GOG library, empty when it has none.
    pub fn gog_library(&self, user_id: String) -> Vec<CachedGogGame> {
        // Read field by field: the entry's metadata stays in the cache.
        self.inner.library(GOG, &user_id, |entry| {
            Some(CachedGogGame {
                game: GogGame::deserialize(entry).ok()?,
                playtime_minutes: entry.get("playtimeMinutes").and_then(Value::as_u64),
                achievement_share: kept(entry).and_then(|kept| unlocked_share(&kept.list)),
            })
        })
    }

    /// Every launch the cache keeps, for every storefront and account.
    pub fn launches(&self) -> Vec<GameLaunch> {
        self.inner
            .launches()
            .into_iter()
            .filter_map(|launch| {
                let game = match launch.storefront.as_str() {
                    STEAM => <steam::Steam as Bridged>::identity(
                        &<steam::Steam as Named>::named(&launch.game)?,
                        &launch.account.parse().ok()?,
                    ),
                    EPIC => <epic::Epic as Bridged>::identity(
                        &<epic::Epic as Named>::named(&launch.game)?,
                        &launch.account,
                    ),
                    GOG => <gog::Gog as Bridged>::identity(
                        &<gog::Gog as Named>::named(&launch.game)?,
                        &launch.account,
                    ),
                    _ => return None,
                };
                Some(GameLaunch {
                    game,
                    at: launch.at,
                })
            })
            .collect()
    }

    /// Records a launch in its game's entry. A game whose library was never
    /// kept has no entry to keep it.
    pub fn record_launch(&self, launch: GameLaunch) {
        let (storefront, account, game) = match launch.game {
            GameIdentity::Steam { app_id, account_id } => (
                STEAM,
                account_id.to_string(),
                <steam::Steam as Named>::name(&app_id),
            ),
            GameIdentity::Epic {
                account_id,
                namespace,
                app_name,
            } => (
                EPIC,
                account_id,
                <epic::Epic as Named>::name(&epic::Store {
                    namespace,
                    app_name,
                }),
            ),
            GameIdentity::Gog {
                account_id,
                product_id,
            } => (GOG, account_id, product_id),
        };
        self.inner
            .record_launch(storefront, &account, &game, launch.at);
    }

    /// Removes the account's library and its launches, as signing it out
    /// does. Its session can no longer write it.
    pub fn forget(&self, account: GameAccount) {
        match account {
            GameAccount::Steam { steam_id } => self.inner.forget(STEAM, &steam_id.to_string()),
            GameAccount::Epic { account_id } => self.inner.forget(EPIC, &account_id),
            GameAccount::Gog { user_id } => self.inner.forget(GOG, &user_id),
        }
    }

    /// Writes the last change now, as Obelisk quits.
    pub fn flush(&self) {
        self.inner.flush();
    }
}

impl LibraryCache {
    pub(crate) fn steam(&self, steam_id: u64) -> SteamLibrary {
        SteamLibrary(self.inner.account(STEAM, &steam_id.to_string()))
    }

    pub(crate) fn epic(&self, account_id: &str) -> EpicLibrary {
        PlaytimeLibrary::new(self.inner.account(EPIC, account_id))
    }

    pub(crate) fn gog(&self, user_id: &str) -> GogLibrary {
        PlaytimeLibrary::new(self.inner.account(GOG, user_id))
    }
}

/// The Steam library a session keeps for its account.
pub(crate) struct SteamLibrary(AccountLibrary);

impl SteamLibrary {
    /// Keeps the account's games with every field Steam returned. Each keeps
    /// the platforms, PICS app info, library date and achievement progress
    /// earlier requests loaded.
    pub(crate) fn games(&self, games: Vec<steam_kit::client::OwnedGame>) -> Vec<SteamGame> {
        let (entries, games) = games
            .into_iter()
            .map(|mut owned| {
                let record = std::mem::take(&mut owned.record);
                let game = SteamGame::from(owned);
                let mut entry = entry_of(&CachedEntry {
                    id: <steam::Steam as Named>::name(&game.app_id),
                    game: &game,
                });
                entry.insert("record".into(), record);
                (entry, game)
            })
            .unzip();
        self.0
            .replace(entries, &["platforms", "pics", ADDED_AT, ACHIEVEMENTS]);
        games
    }

    /// Keeps when each game joined the library, by app ID, from the account's
    /// licenses. A game none of them grants has no date.
    pub(crate) fn acquisitions(&self, acquired: &BTreeMap<u32, u32>) {
        self.0.update(|entry| {
            let added_at = entry
                .get("id")
                .and_then(Value::as_str)
                .and_then(<steam::Steam as Named>::named)
                .and_then(|app_id| acquired.get(&app_id));
            match added_at {
                Some(at) => set(entry, ADDED_AT, (*at).into()),
                None => entry.remove(ADDED_AT).is_some(),
            }
        });
    }

    /// Where the library keeps the games' achievements.
    pub(crate) fn achievements(&self) -> LibraryAchievements<'_> {
        LibraryAchievements {
            account: &self.0,
            id: "id",
            playtime: "playtimeMinutes",
        }
    }

    /// Keeps each game's platforms and whole PICS app info.
    pub(crate) fn platforms(&self, games: Vec<steam_kit::client::GamePlatforms>) {
        let mut games: BTreeMap<String, _> = games
            .into_iter()
            .map(|game| (<steam::Steam as Named>::name(&game.app_id), game))
            .collect();
        self.0.update(|entry| {
            let Some(game) = entry
                .get("id")
                .and_then(Value::as_str)
                .and_then(|id| games.remove(id))
            else {
                return false;
            };
            let platforms = set(entry, "platforms", game.platforms.into());
            game.pics.is_some_and(|pics| set(entry, "pics", pics)) | platforms
        });
    }
}

/// A game a storefront loads apart from its playtime, as Epic and GOG do.
pub(crate) trait PlayedGame {
    /// The game as the library shows it.
    type Shown;
    /// The entry field the loaded playtimes name the game by.
    const PLAYTIME_ID: &'static str;
    /// The entry field that keeps its playtime.
    const PLAYTIME: &'static str;
    /// The entry field its achievements are read by: several games may
    /// share an Epic sandbox.
    const ACHIEVEMENTS_ID: &'static str;
    /// The game's entry, with everything its storefront returned but its
    /// playtime, and the game as the library shows it.
    fn entry(self) -> (Entry, Self::Shown);
}

impl PlayedGame for epic_kit::client::Game {
    type Shown = EpicGame;
    const PLAYTIME_ID: &'static str = "appName";
    const PLAYTIME: &'static str = "playtimeSeconds";
    const ACHIEVEMENTS_ID: &'static str = "namespace";
    fn entry(mut self) -> (Entry, EpicGame) {
        let metadata = std::mem::take(&mut self.metadata);
        let game = EpicGame::from(self);
        let mut entry = entry_of(&CachedEntry {
            id: <epic::Epic as Named>::name(&epic::Store {
                namespace: game.namespace.clone(),
                app_name: game.app_name.clone(),
            }),
            game: &game,
        });
        entry.insert("metadata".into(), metadata);
        (entry, game)
    }
}

impl PlayedGame for gog_kit::client::Game {
    type Shown = GogGame;
    const PLAYTIME_ID: &'static str = "id";
    const PLAYTIME: &'static str = "playtimeMinutes";
    const ACHIEVEMENTS_ID: &'static str = "id";
    fn entry(mut self) -> (Entry, GogGame) {
        let metadata = std::mem::take(&mut self.metadata);
        let game = GogGame::from(self);
        let mut entry = entry_of(&CachedEntry {
            id: game.product_id.clone(),
            game: &game,
        });
        entry.insert("metadata".into(), metadata);
        (entry, game)
    }
}

/// The library a session keeps for its account, whose playtimes load apart
/// from its games: Epic's or GOG's.
pub(crate) struct PlaytimeLibrary<G> {
    account: AccountLibrary,
    /// The playtimes the session last loaded: they may answer before the
    /// games.
    playtimes: Mutex<Option<BTreeMap<String, u64>>>,
    game: PhantomData<fn(G)>,
}

pub(crate) type EpicLibrary = PlaytimeLibrary<epic_kit::client::Game>;
pub(crate) type GogLibrary = PlaytimeLibrary<gog_kit::client::Game>;

impl<G: PlayedGame> PlaytimeLibrary<G> {
    fn new(account: AccountLibrary) -> Self {
        Self {
            account,
            playtimes: Mutex::default(),
            game: PhantomData,
        }
    }

    /// Keeps the account's games with everything their storefront returned.
    /// Until the session loads the playtimes, each game keeps its earlier
    /// one.
    pub(crate) fn games(&self, games: Vec<G>) -> Vec<G::Shown> {
        let playtimes = self.playtimes.lock().unwrap();
        let (entries, games) = games
            .into_iter()
            .map(|loaded| {
                let (mut entry, game) = loaded.entry();
                let playtime = playtimes.as_ref().and_then(|playtimes| {
                    playtimes.get(entry.get(G::PLAYTIME_ID)?.as_str()?).copied()
                });
                if let Some(playtime) = playtime {
                    entry.insert(G::PLAYTIME.into(), playtime.into());
                }
                (entry, game)
            })
            .unzip();
        let kept: &[&str] = if playtimes.is_some() {
            &[ACHIEVEMENTS]
        } else {
            &[G::PLAYTIME, ACHIEVEMENTS]
        };
        self.account.replace(entries, kept);
        games
    }

    /// Where the library keeps the games' achievements.
    pub(crate) fn achievements(&self) -> LibraryAchievements<'_> {
        LibraryAchievements {
            account: &self.account,
            id: G::ACHIEVEMENTS_ID,
            playtime: G::PLAYTIME,
        }
    }

    /// Keeps every game's playtime. A game missing from `playtimes` was
    /// never played.
    pub(crate) fn playtimes(&self, playtimes: &BTreeMap<String, u64>) {
        *self.playtimes.lock().unwrap() = Some(playtimes.clone());
        self.account.update(|entry| {
            let playtime = entry
                .get(G::PLAYTIME_ID)
                .and_then(Value::as_str)
                .and_then(|id| playtimes.get(id).copied());
            match playtime {
                Some(playtime) => set(entry, G::PLAYTIME, playtime.into()),
                None => entry.remove(G::PLAYTIME).is_some(),
            }
        });
    }
}

/// Where an account's library keeps its games' achievements, each with the
/// language, moment and playtime of its last read.
pub(crate) struct LibraryAchievements<'a> {
    account: &'a AccountLibrary,
    /// The entry field the storefront reads a game's achievements by.
    id: &'static str,
    /// The entry field that keeps the game's playtime.
    playtime: &'static str,
}

/// A game's achievements as its entry keeps them: in `locale`, read at
/// `at`, in seconds since the Unix epoch, when the game's playtime was
/// `playtime`.
#[derive(Serialize, Deserialize)]
struct KeptAchievements {
    list: Vec<LibraryAchievement>,
    locale: String,
    at: u64,
    playtime: Option<u64>,
}

impl LibraryAchievements<'_> {
    /// The lists the library keeps of `ids`, named as their entries' id
    /// field names them, that are current in `locale`, and the ids to read
    /// again: those it keeps none of, keeps in another language, or keeps
    /// from before one of their games was last played, as its playtime
    /// changed since or Obelisk launched it after.
    fn current<K: ToString>(&self, ids: Vec<K>, locale: &str) -> (Vec<GameAchievements>, Vec<K>) {
        let asked: HashSet<String> = ids.iter().map(ToString::to_string).collect();
        let read = self.account.read(|entry| {
            let id = entry
                .get(self.id)?
                .as_str()
                .filter(|id| asked.contains(*id))?;
            let current = kept(entry).filter(|kept| {
                kept.locale == locale
                    && kept.playtime == entry.get(self.playtime).and_then(Value::as_u64)
                    && library_cache::launched_at(entry).is_none_or(|launched| launched <= kept.at)
            });
            Some((id.to_owned(), current.map(|kept| kept.list)))
        });
        // Games read by one id, such as an Epic sandbox's, are current together.
        let mut lists: HashMap<String, Option<Vec<LibraryAchievement>>> = HashMap::new();
        for (id, list) in read {
            lists
                .entry(id)
                .and_modify(|kept| {
                    if list.is_none() {
                        *kept = None;
                    }
                })
                .or_insert(list);
        }
        let mut current = Vec::new();
        let mut read_again = Vec::new();
        for id in ids {
            let name = id.to_string();
            match lists.remove(&name).flatten() {
                Some(achievements) => current.push(GameAchievements {
                    id: name,
                    achievements,
                }),
                None => read_again.push(id),
            }
        }
        (current, read_again)
    }

    /// Keeps each list read now in `locale`, with its game's playtime of the
    /// moment.
    fn keep(&self, read: &[GameAchievements], locale: &str) {
        let read: HashMap<&str, &[LibraryAchievement]> = read
            .iter()
            .map(|game| (game.id.as_str(), game.achievements.as_slice()))
            .collect();
        let at = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |since| since.as_secs());
        self.account.update(|entry| {
            let Some(list) = entry
                .get(self.id)
                .and_then(Value::as_str)
                .and_then(|id| read.get(id))
            else {
                return false;
            };
            let kept = KeptAchievements {
                list: list.to_vec(),
                locale: locale.to_owned(),
                at,
                playtime: entry.get(self.playtime).and_then(Value::as_u64),
            };
            set(
                entry,
                ACHIEVEMENTS,
                serde_json::to_value(kept).unwrap_or_default(),
            )
        });
    }
}

/// `ids`' achievements in `locale`: the library's while current, unless
/// `fresh`, else read through `read`, `width` games at a time, which the
/// library then keeps. `ids` name games as their entries' id field does. A
/// game `read` failed for is left out; when no game could be told, the
/// first failure.
pub(crate) async fn achievements<K, A, E, Fut>(
    library: Option<LibraryAchievements<'_>>,
    ids: Vec<K>,
    locale: &str,
    fresh: bool,
    width: usize,
    read: impl Fn(K) -> Fut,
) -> Result<Vec<GameAchievements>, E>
where
    K: ToString,
    A: Into<LibraryAchievement>,
    Fut: Future<Output = Result<Vec<A>, E>>,
{
    let (mut current, stale) = match &library {
        Some(library) if !fresh => library.current(ids, locale),
        _ => (Vec::new(), ids),
    };
    let mut reads = stream::iter(stale)
        .map(|id| {
            let name = id.to_string();
            let reading = read(id);
            async move { (name, reading.await) }
        })
        .buffer_unordered(width);
    let mut failure = None;
    let mut read_now = Vec::new();
    while let Some((id, result)) = reads.next().await {
        match result {
            Ok(list) => read_now.push(GameAchievements {
                id,
                achievements: list.into_iter().map(Into::into).collect(),
            }),
            Err(error) => {
                failure.get_or_insert(error);
            }
        }
    }
    if let Some(library) = &library
        && !read_now.is_empty()
    {
        library.keep(&read_now, locale);
    }
    current.extend(read_now);
    match failure {
        Some(error) if current.is_empty() => Err(error),
        _ => Ok(current),
    }
}

/// The achievements an entry keeps, with when and how they were read.
fn kept(entry: &Entry) -> Option<KeptAchievements> {
    KeptAchievements::deserialize(entry.get(ACHIEVEMENTS)?).ok()
}

/// The share of `achievements` unlocked; none when there are none.
fn unlocked_share(achievements: &[LibraryAchievement]) -> Option<f64> {
    let unlocked = achievements
        .iter()
        .filter(|achievement| achievement.unlocked)
        .count();
    (!achievements.is_empty()).then(|| unlocked as f64 / achievements.len() as f64)
}

fn entry_of(entry: &impl Serialize) -> Entry {
    match serde_json::to_value(entry) {
        Ok(Value::Object(entry)) => entry,
        _ => Entry::new(),
    }
}

/// Sets the entry's `key` to `value`; `true` when it changed.
fn set(entry: &mut Entry, key: &str, value: Value) -> bool {
    if entry.get(key) == Some(&value) {
        return false;
    }
    entry.insert(key.into(), value);
    true
}

/// A game as the cache keeps it: as the library shows it. A Steam game's
/// entry also keeps its `Player.GetOwnedGames` `record`, its `platforms`,
/// absent until Steam told them, its whole PICS app info, `pics`, when it
/// joined the library, `addedAt`, and its `achievements` progress. An Epic or
/// GOG game's keeps its playtime, absent until the storefront told it or when
/// the game was never played, its `achievements` progress with when it was
/// read and the playtime then, and everything else the storefront returned,
/// `metadata`: Epic's catalog items, library records and assets, GOG's
/// library record and catalog data.
#[derive(Serialize)]
struct CachedEntry<'a, G> {
    id: String,
    #[serde(flatten)]
    game: &'a G,
}

#[cfg(test)]
mod tests;
