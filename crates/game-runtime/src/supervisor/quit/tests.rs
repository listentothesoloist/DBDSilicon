use super::*;
use crate::launch_settings::Settings;

fn scope() -> Scope {
    Scope {
        runs: vec!["/lib/Voile/v1".into(), "/Games/Peak".into()],
        uses: vec![
            "/Games/Peak".into(),
            "/lib/Prefixes/Peak".into(),
            "/private/tmp/.wine-501/server-10-20".into(),
        ],
        spared: vec!["/Applications/Obelisk.app".into()],
    }
}

#[test]
fn force_quit_ends_only_what_runs_for_the_game() {
    let listing = [
        // The game in Wine, and a Wine service of its prefix.
        "p10\nfcwd\nn/Games/Peak\nftxt\nn/lib/Voile/v1/bin/wine-preloader",
        "p11\nfcwd\nn/lib/Prefixes/Peak/drive_c/windows/system32\nftxt\nn/lib/Voile/v1/lib/wine/ntdll.so",
        // Its Wine server, in the folder Wine names after the prefix.
        "p12\nfcwd\nn/private/tmp/.wine-501/server-10-20\nftxt\nn/lib/Voile/v1/bin/wineserver",
        // A child the Mac game started from its own files.
        "p13\nftxt\nn/Games/Peak/Peak.app/Contents/MacOS/CrashHandler\nf4\nn/Games/Peak/log.txt",
        // Another game in Wine.
        "p20\nfcwd\nn/lib/Prefixes/Other\nftxt\nn/lib/Voile/v1/bin/wine-preloader",
        // A shell the player opened in the game's folder.
        "p21\nfcwd\nn/Games/Peak\nftxt\nn/bin/zsh",
        // Obelisk reading the game's files.
        "p22\nftxt\nn/Applications/Obelisk.app/Contents/MacOS/Obelisk\nf9\nn/Games/Peak/Peak.exe",
        // A Wine process left from a Voile file that matches only by name.
        "p23\nfcwd\nn/Games/Peak-2\nftxt\nn/lib/Voile/v10/bin/wine",
        // The supervisor itself.
        "p30\nfcwd\nn/Games/Peak\nftxt\nn/lib/Voile/v1/bin/wine",
    ]
    .join("\n");
    assert_eq!(matching(&listing, &scope(), 30), [10, 11, 12, 13]);
}

#[test]
fn a_game_without_an_installation_or_prefix_leaves_nothing_to_sweep() {
    let listing = "p10\nfcwd\nn/Games/Peak\nftxt\nn/Games/Peak/Peak\n";
    let scope = Scope::of(
        &Game {
            environment: HashMap::new(),
            installation: None,
            program: None,
        },
        None,
        Some(10),
        Path::new("/Applications/Obelisk.app/Contents/MacOS/Obelisk"),
    );
    assert!(matching(listing, &scope, 1).is_empty());
}

#[test]
fn each_engine_scopes_the_sweep_to_the_game_and_spares_obelisk() {
    let game = Game {
        environment: HashMap::from([("TMPDIR".into(), "/var/folders/T/".into())]),
        installation: Some("/Games/Peak".into()),
        program: Some("/Games/Peak/Peak.exe".into()),
    };
    let own =
        Path::new("/Applications/Obelisk.app/Contents/Helpers/SteamNative/obelisk-steam-host");
    let native = Scope::of(&game, None, Some(7), own);
    assert_eq!(
        native,
        Scope {
            runs: vec!["/Games/Peak".into()],
            uses: vec!["/Games/Peak".into()],
            spared: vec!["/Applications/Obelisk.app".into()],
        }
    );

    let rosetta = wine::Config {
        runtime: "/lib/Voile/v1".into(),
        prefix: "/lib/Prefixes/Peak".into(),
        log: "/logs/wine-Peak.log".into(),
        settings: Settings::default(),
    };
    let windows = Scope::of(&game, Some(&rosetta), Some(7), own);
    assert_eq!(
        windows.runs,
        [Path::new("/Games/Peak"), Path::new("/lib/Voile/v1")]
    );
    assert_eq!(
        windows.uses[..2],
        [PathBuf::from("/Games/Peak"), "/lib/Prefixes/Peak".into()]
    );

    let linux = wine::Config {
        prefix: "/lib/Prefixes/Peak-linux/prefix".into(),
        runtime: "/lib/Voile/v1/linux".into(),
        settings: Settings {
            engine: wine::Engine::Linux,
            ..Settings::default()
        },
        ..rosetta
    };
    let engine = Scope::of(&game, Some(&linux), Some(7), own);
    assert_eq!(
        engine.uses,
        [
            Path::new("/Games/Peak"),
            Path::new("/lib/Prefixes/Peak-linux"),
            Path::new("/var/folders/T/obelisk-engine-7"),
        ]
    );
}

#[test]
fn obelisk_is_spared_from_its_outermost_bundle() {
    assert_eq!(
        bundle(Path::new(
            "/Applications/Obelisk.app/Contents/Helpers/Rosetta.app/Contents/MacOS/helper"
        )),
        Path::new("/Applications/Obelisk.app")
    );
    assert_eq!(
        bundle(Path::new("/target/debug/deps/game_runtime")),
        Path::new("/target/debug/deps")
    );
}

#[test]
fn everything_the_game_started_ends_with_it() {
    let listing = "  1     0\n 10     1\n 11    10\n 12    11\n 13    10\n 20     1\n 21    20\n";
    assert_eq!(descendants(listing, 10), [11, 13, 12]);
    assert!(descendants(listing, 12).is_empty());
}
