//! The diagnostics a bug report attaches, as the app names its subject.
use crate::{ObeliskLocations, SessionStorefront};
use game_runtime::diagnostics::{self, Game};
use std::{path::Path, time::SystemTime};

/// What a bug report is about, as far as its diagnostics go: Obelisk, or one
/// game as its storefront names it.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum DiagnosticsSubject {
    App,
    Game {
        storefront: SessionStorefront,
        id: String,
    },
}

/// The diagnostics a bug report on `subject` attaches, most telling first.
#[uniffi::export]
pub fn diagnostic_files(
    locations: ObeliskLocations,
    subject: DiagnosticsSubject,
    now: SystemTime,
) -> Vec<String> {
    let game = match &subject {
        DiagnosticsSubject::App => None,
        // A Steam game that names no app has no diagnostics, and never
        // Obelisk's own.
        DiagnosticsSubject::Game {
            storefront: SessionStorefront::Steam,
            id,
        } => match id.parse() {
            Ok(app_id) => Some(Game::Steam(app_id)),
            Err(_) => return Vec::new(),
        },
        DiagnosticsSubject::Game {
            storefront: SessionStorefront::Epic,
            id,
        } => Some(Game::Epic(id)),
        DiagnosticsSubject::Game {
            storefront: SessionStorefront::Gog,
            id,
        } => Some(Game::Gog(id)),
    };
    diagnostics::files(Path::new(&locations.home), game, now)
        .into_iter()
        .map(|path| path.to_string_lossy().into_owned())
        .collect()
}

/// `text` without the Metal HUD's frame lines, which bug reports leave out.
#[uniffi::export]
pub fn without_metal_hud_frames(text: String) -> String {
    game_runtime::metal_hud::without_frame_lines(&text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::obelisk_locations;
    use obelisk_core::Platform;
    use std::fs;

    #[test]
    fn a_game_s_diagnostics_are_found_under_its_own_storefront() {
        let home = tempfile::tempdir().unwrap();
        let locations = obelisk_locations(
            "/library".into(),
            "/Obelisk.app".into(),
            home.path().to_string_lossy().into_owned(),
        );
        // One ID in every storefront, so a subject routed to the wrong one
        // finds that one's log.
        let games = [
            (SessionStorefront::Steam, Game::Steam(7)),
            (SessionStorefront::Epic, Game::Epic("7")),
            (SessionStorefront::Gog, Game::Gog("7")),
        ];
        for (_, game) in games {
            let log = diagnostics::game_log(home.path(), game, Platform::Mac);
            fs::create_dir_all(log.parent().unwrap()).unwrap();
            fs::write(&log, "log\n").unwrap();
        }
        for (storefront, game) in games {
            let subject = DiagnosticsSubject::Game {
                storefront,
                id: "7".into(),
            };
            let log = diagnostics::game_log(home.path(), game, Platform::Mac);
            assert_eq!(
                diagnostic_files(locations.clone(), subject, SystemTime::now()),
                [log.to_string_lossy().into_owned()],
                "{game:?}"
            );
        }

        // Obelisk's diagnostics are launch reports, never a game's logs.
        assert!(
            diagnostic_files(
                locations.clone(),
                DiagnosticsSubject::App,
                SystemTime::now()
            )
            .is_empty()
        );
        let unnamed = DiagnosticsSubject::Game {
            storefront: SessionStorefront::Steam,
            id: "abc".into(),
        };
        assert!(diagnostic_files(locations, unnamed, SystemTime::now()).is_empty());
    }
}
