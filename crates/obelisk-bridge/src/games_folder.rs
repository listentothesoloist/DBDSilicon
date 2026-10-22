//! Where Obelisk installs games, as Swift presents and changes it. Every
//! account's installations of one library share its games folder.
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, OnceLock},
};

use game_runtime::games_folder::{self, Candidate, ChoiceFailure, Refusal};

use crate::GameSessionError;

/// The games folder of Obelisk's `library`, the same for every caller.
pub(crate) fn shared(library: &Path) -> Arc<games_folder::GamesFolder> {
    static FOLDERS: OnceLock<Mutex<HashMap<PathBuf, Arc<games_folder::GamesFolder>>>> =
        OnceLock::new();
    FOLDERS
        .get_or_init(Default::default)
        .lock()
        .unwrap()
        .entry(library.to_owned())
        .or_insert_with(|| games_folder::GamesFolder::new(library))
        .clone()
}

/// Where new installations go. `revision` grows with every change.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct GamesFolderState {
    pub revision: u64,
    pub path: String,
    /// Obelisk's own folder, inside its library.
    pub is_default: bool,
    /// Its disk is there: Obelisk's own folder always is.
    pub is_connected: bool,
}

impl From<games_folder::View> for GamesFolderState {
    fn from(view: games_folder::View) -> Self {
        Self {
            revision: view.revision,
            path: view.path.to_string_lossy().into_owned(),
            is_default: view.default,
            is_connected: view.connected,
        }
    }
}

/// What choosing a folder would do.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum GamesFolderInspection {
    /// The games go in `path`: `games` installations move there, and
    /// `copied_bytes` of theirs are copied from another disk.
    Ready {
        path: String,
        games: u32,
        copied_bytes: u64,
    },
    /// The folder is gone.
    Missing,
    /// The folder is inside Obelisk's library or a game's installation.
    Inside,
}

impl From<Result<Candidate, Refusal>> for GamesFolderInspection {
    fn from(inspection: Result<Candidate, Refusal>) -> Self {
        match inspection {
            Ok(candidate) => Self::Ready {
                path: candidate.path.to_string_lossy().into_owned(),
                games: candidate.games,
                copied_bytes: candidate.copied_bytes,
            },
            Err(Refusal::Missing) => Self::Missing,
            Err(Refusal::Inside) => Self::Inside,
        }
    }
}

/// Obelisk's games folder.
#[derive(uniffi::Object)]
pub struct GamesFolder {
    folder: Arc<games_folder::GamesFolder>,
}

#[uniffi::export(async_runtime = "tokio")]
impl GamesFolder {
    /// The games folder of the Obelisk whose library is `library`.
    #[uniffi::constructor]
    pub fn new(library: String) -> Arc<Self> {
        Arc::new(Self {
            folder: shared(Path::new(&library)),
        })
    }

    pub fn state(&self) -> GamesFolderState {
        self.folder.view().into()
    }

    /// Waits for a revision newer than `after`.
    pub async fn changed(&self, after: u64) -> GamesFolderState {
        self.folder.changed(after).await.into()
    }

    /// What choosing `chosen` would do. It blocks on file reads.
    pub fn inspect(&self, chosen: String) -> GamesFolderInspection {
        self.folder.inspect(Path::new(&chosen)).into()
    }

    /// What making Obelisk's own folder the games folder again would do. It
    /// makes that folder, and blocks on file reads.
    pub fn inspect_default(&self) -> GamesFolderInspection {
        self.folder.inspect_default().into()
    }

    /// Makes the games folder `chosen` gives the one new installations go
    /// to, and queues the move of every installation into it.
    pub fn choose(&self, chosen: String) -> Result<(), GameSessionError> {
        self.folder
            .choose(Path::new(&chosen))
            .map_err(choice_failure)
    }

    /// Makes Obelisk's own folder the games folder again, and queues the
    /// move of every installation into it.
    pub fn choose_default(&self) -> Result<(), GameSessionError> {
        self.folder.choose_default().map_err(choice_failure)
    }

    /// Queues the move of the installations still outside the games folder.
    pub fn gather(&self) {
        self.folder.gather()
    }

    /// A disk came or left.
    pub fn volumes_changed(&self) {
        self.folder.volumes_changed()
    }
}

fn choice_failure(failure: ChoiceFailure) -> GameSessionError {
    match failure {
        ChoiceFailure::Refused(_) => GameSessionError::new("model.gamesFolder.refused"),
        ChoiceFailure::Io(error) => GameSessionError::new(error),
    }
}
