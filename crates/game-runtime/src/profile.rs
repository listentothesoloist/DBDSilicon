//! What Obelisk knows about particular Windows games, whatever their
//! storefront: the engine that runs one, the graphics backend its Direct3D
//! goes through there, the program started instead of its publisher's, and
//! what that program needs besides: arguments, environment variables and DLL
//! overrides. The table is `profiles.json`, built into Obelisk. It names a
//! game by its storefronts' ids, or by a file of its installation, which
//! finds the game whatever the storefront. Every game it does not name runs
//! with the defaults, in Rosetta's Wine with D3DMetal. The player's launch
//! settings apply over it.
use crate::wine::{Engine, Graphics, GraphicsOptions};
use serde::Deserialize;
use std::{collections::BTreeMap, path::Path, sync::OnceLock};

const TABLE: &str = include_str!("../profiles.json");

/// A game as its storefront names it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Game<'a> {
    Steam {
        app_id: u32,
    },
    Epic {
        namespace: &'a str,
        app_name: &'a str,
    },
    Gog {
        product_id: &'a str,
    },
}

/// How Obelisk runs one Windows game.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Profile {
    pub engine: Engine,
    /// What renders its Direct3D on that engine.
    pub graphics: Graphics,
    /// What it asks of that backend.
    pub options: GraphicsOptions,
    /// The program the game starts with instead of its publisher's, relative
    /// to the installation, when that one cannot run there.
    pub executable: Option<&'static str>,
    /// What it adds to the storefront's arguments, before the player's.
    pub arguments: &'static [String],
    /// The environment variables every Wine command of its prefix runs
    /// with, over Obelisk's own.
    pub variables: &'static BTreeMap<String, String>,
    /// How Wine loads some of its DLLs, by DLL, as `WINEDLLOVERRIDES` spells
    /// it: `n,b` for the game's own copy first, nothing to never load it.
    pub dll_overrides: &'static BTreeMap<String, String>,
}

static NONE: BTreeMap<String, String> = BTreeMap::new();

impl Default for Profile {
    fn default() -> Self {
        Self {
            engine: Engine::Rosetta,
            graphics: Graphics::default_on(Engine::Rosetta),
            options: GraphicsOptions::default(),
            executable: None,
            arguments: &[],
            variables: &NONE,
            dll_overrides: &NONE,
        }
    }
}

/// The table, as `profiles.json` holds it. Each entry also has a `name` and
/// a `note`, for its readers.
#[derive(Debug, Deserialize)]
struct Table {
    games: Vec<Known>,
}

/// One game of the table: the storefronts' names for it, a file only its
/// installation holds, and what its profile changes from the defaults.
#[derive(Debug, Deserialize)]
struct Known {
    steam: Option<u32>,
    epic: Option<EpicGame>,
    gog: Option<String>,
    /// Relative to the installation.
    path: Option<String>,
    engine: Option<Engine>,
    graphics: Option<Graphics>,
    executable: Option<String>,
    #[serde(default)]
    arguments: Vec<String>,
    #[serde(default)]
    variables: BTreeMap<String, String>,
    #[serde(default)]
    dll_overrides: BTreeMap<String, String>,
}

#[derive(Debug, Deserialize)]
struct EpicGame {
    namespace: String,
    app_name: String,
}

fn table() -> &'static [Known] {
    static VALUE: OnceLock<Table> = OnceLock::new();
    &VALUE
        .get_or_init(|| serde_json::from_str(TABLE).expect("checked-in profiles"))
        .games
}

impl Known {
    /// Whether this entry names `game` by its storefront's id.
    fn names(&self, game: Game<'_>) -> bool {
        match game {
            Game::Steam { app_id } => self.steam == Some(app_id),
            Game::Epic {
                namespace,
                app_name,
            } => self
                .epic
                .as_ref()
                .is_some_and(|epic| epic.namespace == namespace && epic.app_name == app_name),
            Game::Gog { product_id } => self.gog.as_deref() == Some(product_id),
        }
    }

    fn profile(&'static self) -> Profile {
        let engine = self.engine.unwrap_or_default();
        Profile {
            engine,
            graphics: self.graphics.unwrap_or(Graphics::default_on(engine)),
            options: GraphicsOptions::default(),
            executable: self.executable.as_deref(),
            arguments: &self.arguments,
            variables: &self.variables,
            dll_overrides: &self.dll_overrides,
        }
    }

    /// Whether the installation in `install` holds this entry's file.
    fn found_in(&self, install: &Path) -> bool {
        self.path
            .as_ref()
            .is_some_and(|path| install.join(path).is_file())
    }
}

/// The profile of `game`, installed in `install`: the one the table gives
/// its storefront's id, else the one whose file the installation holds, else
/// the defaults.
pub(crate) fn of(game: Game<'_>, install: &Path) -> Profile {
    let table = table();
    table
        .iter()
        .find(|known| known.names(game))
        .or_else(|| table.iter().find(|known| known.found_in(install)))
        .map(Known::profile)
        .unwrap_or_default()
}

#[cfg(test)]
mod tests;
