use super::*;
use std::{collections::HashSet, path::Component};

/// The profile of `game` found by its id, in an installation that holds no
/// file of the table.
fn by_id(game: Game<'_>) -> Profile {
    of(game, Path::new("/nowhere"))
}

const ROCKET_LEAGUE: Game<'static> = Game::Epic {
    namespace: "9773aa1aa54f4f7b80e44bef04986cea",
    app_name: "Sugar",
};

#[test]
fn rocket_league_and_elden_ring_run_on_the_linux_engine_and_nothing_else_does() {
    assert_eq!(by_id(ROCKET_LEAGUE).engine, Engine::Linux);
    assert_eq!(
        by_id(ROCKET_LEAGUE).executable,
        Some("Binaries/Win64/RocketLeague_EAC.exe")
    );
    assert_eq!(by_id(Game::Steam { app_id: 1245620 }).engine, Engine::Linux);
    for other in [
        Game::Epic {
            namespace: "9773aa1aa54f4f7b80e44bef04986cea",
            app_name: "Other",
        },
        Game::Epic {
            namespace: "5a6d34ba28bc45d081d9311c3885feef",
            app_name: "Sugar",
        },
        Game::Steam { app_id: 252950 },
        Game::Gog {
            product_id: "1245620",
        },
    ] {
        assert_eq!(by_id(other), Profile::default(), "{other:?}");
        assert_eq!(by_id(other).engine, Engine::Rosetta);
    }
}

#[test]
fn elden_ring_renders_through_d3dmetal_rocket_league_through_dxmt_and_others_through_d3dmetal() {
    assert_eq!(
        by_id(Game::Steam { app_id: 1245620 }).graphics,
        Graphics::D3DMetal
    );
    assert_eq!(by_id(ROCKET_LEAGUE).graphics, Graphics::Dxmt);
    assert_eq!(Profile::default().graphics, Graphics::D3DMetal);
    assert_eq!(Profile::default().options, GraphicsOptions::default());
}

/// Every game of `profiles.json` as written there.
fn entries() -> Vec<serde_json::Map<String, serde_json::Value>> {
    let table: serde_json::Value = serde_json::from_str(TABLE).unwrap();
    table["games"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| entry.as_object().unwrap().clone())
        .collect()
}

#[test]
fn every_game_of_the_table_has_a_name_a_note_an_id_or_a_file_and_only_what_obelisk_reads() {
    const KEYS: [&str; 12] = [
        "name",
        "note",
        "steam",
        "epic",
        "gog",
        "path",
        "engine",
        "graphics",
        "executable",
        "arguments",
        "variables",
        "dll_overrides",
    ];
    assert_eq!(entries().len(), table().len());
    for entry in entries() {
        let name = entry["name"].as_str().unwrap();
        assert!(!name.is_empty());
        assert!(
            entry["note"].as_str().is_some_and(|note| !note.is_empty()),
            "{name}"
        );
        assert!(
            ["steam", "epic", "gog", "path"]
                .iter()
                .any(|key| entry.contains_key(*key)),
            "{name}"
        );
        for key in entry.keys() {
            assert!(KEYS.contains(&key.as_str()), "{name}: {key}");
        }
        if let Some(epic) = entry.get("epic") {
            let keys: Vec<_> = epic.as_object().unwrap().keys().collect();
            assert_eq!(keys, ["app_name", "namespace"], "{name}");
        }
    }
}

#[test]
fn no_two_games_of_the_table_share_a_storefront_s_name() {
    let mut names = HashSet::new();
    for known in table() {
        let ids = [
            known.steam.map(|app_id| format!("steam {app_id}")),
            known
                .epic
                .as_ref()
                .map(|epic| format!("epic {}:{}", epic.namespace, epic.app_name)),
            known.gog.as_ref().map(|product| format!("gog {product}")),
        ];
        for id in ids.into_iter().flatten() {
            assert!(names.insert(id.clone()), "{id}");
        }
    }
}

#[test]
fn the_table_leaves_obelisk_s_own_wine_settings_alone() {
    let reserved: Vec<_> = crate::launch_settings::ENGINES
        .iter()
        .map(|offer| offer.engine.reserved())
        .collect();
    const MODES: [&str; 6] = ["", "n", "b", "n,b", "b,n", "d"];
    for known in table() {
        for name in known.variables.keys() {
            let mut characters = name.chars();
            assert!(
                characters
                    .next()
                    .is_some_and(|first| first.is_ascii_alphabetic() || first == '_')
                    && characters.all(|next| next.is_ascii_alphanumeric() || next == '_'),
                "{name}"
            );
            assert!(
                reserved
                    .iter()
                    .all(|own| !own.variables.contains(&name.as_str())),
                "{name}"
            );
        }
        for (dll, mode) in &known.dll_overrides {
            assert!(!dll.is_empty() && !dll.contains([',', '=', ';']), "{dll}");
            let dll_name = dll.to_ascii_lowercase();
            assert!(
                reserved
                    .iter()
                    .all(|own| !own.dlls.contains(&dll_name.as_str())),
                "{dll}"
            );
            assert!(MODES.contains(&mode.as_str()), "{dll}={mode}");
        }
        if let Some(executable) = &known.executable {
            assert!(
                Path::new(executable)
                    .components()
                    .all(|component| matches!(component, Component::Normal(_))),
                "{executable}"
            );
        }
    }
}

#[test]
fn a_game_of_the_table_keeps_the_defaults_it_does_not_change_and_adds_its_arguments() {
    let known: &'static Known = Box::leak(Box::new(
        serde_json::from_str(
            r#"{
                "steam": 42,
                "arguments": ["-dx11"],
                "variables": {"WINENCPU": "10"},
                "dll_overrides": {"xaudio2_7": "n,b"}
            }"#,
        )
        .unwrap(),
    ));
    let profile = known.profile();
    assert_eq!(profile.engine, Engine::Rosetta);
    assert_eq!(profile.graphics, Graphics::D3DMetal);
    assert_eq!(profile.variables["WINENCPU"], "10");
    assert_eq!(profile.dll_overrides["xaudio2_7"], "n,b");
    assert_eq!(profile.arguments, ["-dx11"]);
}

#[test]
fn a_file_of_the_table_is_inside_the_installation_and_only_one_game_holds_it() {
    const SHARED: [&str; 10] = [
        "start_protected_game.exe",
        "launcher.exe",
        "game.exe",
        "client.exe",
        "start.exe",
        "setup.exe",
        "play.exe",
        "crashreportclient.exe",
        "unitycrashhandler64.exe",
        "unitycrashhandler32.exe",
    ];
    let mut paths = HashSet::new();
    for path in table().iter().filter_map(|known| known.path.as_deref()) {
        let file = Path::new(path);
        assert!(
            file.components()
                .all(|component| matches!(component, Component::Normal(_))),
            "{path}"
        );
        let name = file.file_name().unwrap().to_string_lossy().to_lowercase();
        assert!(!SHARED.contains(&name.as_str()), "{path}");
        assert!(paths.insert(path.to_lowercase()), "{path}");
    }
}

#[test]
fn a_game_is_found_by_its_storefront_s_id_first_then_by_a_file_of_its_installation() {
    let directory = tempfile::tempdir().unwrap();
    let install = directory.path();
    let fallout_4 = by_id(Game::Steam { app_id: 377160 });
    let gog = Game::Gog {
        product_id: "not in the table",
    };
    assert_eq!(of(gog, install), Profile::default());

    std::fs::write(install.join("Fallout4.exe"), "").unwrap();

    assert_eq!(of(gog, install), fallout_4);
    assert_eq!(
        of(Game::Steam { app_id: 1245620 }, install),
        by_id(Game::Steam { app_id: 1245620 })
    );
}
