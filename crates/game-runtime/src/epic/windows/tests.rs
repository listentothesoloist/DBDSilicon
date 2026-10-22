use super::*;
use crate::wine::tests::{calls, installation, locations, pe, record_calls};
use obelisk_core::Platform;
use std::fs;

fn configuration(root: &Path) -> Configuration {
    let install = root.join("library/Games/Epic/epic-hash-Windows");
    Configuration {
        root: root.join("library/SteamCloud"),
        home: root.join("home"),
        install: install.clone(),
        session_directory: root.join("library/EpicSessions"),
        helper: root.join("Obelisk.app/Contents/Helpers/SteamNative/obelisk-steam-host"),
        namespace: "space".into(),
        catalog_item_id: "base".into(),
        app_name: "game".into(),
        platform: Platform::Windows,
        language: "en".into(),
        title: String::new(),
        prefix: Some(root.join("library/Prefixes/epic-hash-Windows")),
    }
}

/// A completed Windows installation whose receipt names `executable` and
/// `prerequisite`.
fn install(
    config: &Configuration,
    executable: &[u8],
    prerequisite: Option<epic_kit::client::Prerequisite>,
) {
    fs::create_dir_all(config.install.join("Binaries")).unwrap();
    fs::write(config.install.join("Binaries/Game.exe"), executable).unwrap();
    let receipt = epic_kit::client::Installation {
        app_name: config.app_name.clone(),
        namespace: config.namespace.clone(),
        catalog_item_id: config.catalog_item_id.clone(),
        account_id: "account".into(),
        directory: config.install.to_string_lossy().into_owned(),
        platform: config.platform,
        total_bytes: 1,
        build_version: "1.0".into(),
        launch_executable: "Binaries/Game.exe".into(),
        launch_command: String::new(),
        deployment_id: None,
        prerequisite,
    };
    fs::write(
        config.install.join(obelisk_core::install::RECEIPT_FILE),
        serde_json::to_vec(&receipt).unwrap(),
    )
    .unwrap();
}

fn prerequisite(ids: &[&str], path: &str) -> epic_kit::client::Prerequisite {
    epic_kit::client::Prerequisite {
        ids: ids.iter().map(|id| (*id).into()).collect(),
        name: "Prerequisites".into(),
        path: path.into(),
        arguments: "/quiet /norestart".into(),
    }
}

#[test]
fn an_epic_prerequisite_runs_unless_it_leaves_the_installation() {
    use wine::Storefront;
    let install = Path::new("/games/epic");
    let listed = |prerequisite: Option<&epic_kit::client::Prerequisite>| {
        Prefix {
            install,
            prerequisite,
        }
        .prerequisites()
        .unwrap()
    };

    assert_eq!(
        listed(Some(&prerequisite(
            &["a", "b"],
            "Redist\\DirectX\\setup.exe"
        ))),
        [wine::Prerequisite {
            id: "epic:a,b".into(),
            programs: vec![(
                install.join("Redist/DirectX/setup.exe"),
                "/quiet /norestart".into()
            )],
        }]
    );
    assert_eq!(
        listed(Some(&prerequisite(&[], "Redist/setup.exe")))[0].id,
        "epic:Redist/setup.exe"
    );
    // Visual C++ and Unreal's prerequisites install as under Proton.
    assert_eq!(
        listed(Some(&prerequisite(
            &["ue"],
            "Engine\\Extras\\Redist\\en-us\\UEPrereqSetup_x64.exe"
        )))[0]
            .programs,
        [(
            install.join("Engine/Extras/Redist/en-us/UEPrereqSetup_x64.exe"),
            "/quiet /norestart".into()
        )]
    );
    for path in ["..\\outside.exe", "/Applications/setup.exe"] {
        assert!(
            listed(Some(&prerequisite(&["a"], path))).is_empty(),
            "{path}"
        );
    }
    assert!(listed(None).is_empty());
}

#[test]
fn a_windows_build_readies_its_prefix_with_its_prerequisite_before_any_credential() {
    let (_directory, root) = installation();
    let locations = locations(&root);
    record_calls(&locations.voile.join("current"));
    let config = configuration(&root);
    install(
        &config,
        &pe(),
        Some(prerequisite(&["dx"], "Redist\\setup.exe")),
    );
    fs::create_dir_all(config.install.join("Redist")).unwrap();
    fs::write(config.install.join("Redist/setup.exe"), "").unwrap();
    let windows = wine::Installation::of(
        &locations,
        config.profile_game(),
        config.platform,
        &config.install,
    )
    .unwrap()
    .unwrap();

    let wine = ready(&windows, &config, &locations).unwrap();

    assert_eq!(wine.prefix, root.join("library/Prefixes/epic-hash-Windows"));
    assert!(config.runs_in(Some(&wine)));
    assert_eq!(
        calls(&root.join("library/Prefixes")),
        [
            "wineboot --init".to_owned(),
            format!(
                "{} /quiet /norestart",
                config.install.join("Redist/setup.exe").display()
            ),
        ]
    );
}
