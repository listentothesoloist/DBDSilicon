use super::*;
use crate::wine::tests::{calls, installation, locations, pe, record_calls};
use std::fs;

#[test]
fn a_windows_build_readies_its_prefix_with_its_prerequisites() {
    let (_directory, root) = installation();
    let locations = locations(&root);
    record_calls(&locations.voile.join("current"));
    let install = locations.library.join("Games/GOG/gog-1207658924-windows");
    let redistributable = install.join("__redist/MSVC2017/VC_redist.x86.exe");
    fs::create_dir_all(install.join("System")).unwrap();
    fs::create_dir_all(redistributable.parent().unwrap()).unwrap();
    fs::write(install.join("System/witcher.exe"), pe()).unwrap();
    fs::write(&redistributable, "").unwrap();
    let info = serde_json::json!({"playTasks": [
        {"category": "game", "isPrimary": true, "path": "System\\witcher.exe", "type": "FileTask"},
    ]});
    fs::write(
        install.join("goggame-1207658924.info"),
        serde_json::to_vec(&info).unwrap(),
    )
    .unwrap();
    let receipt = gog_kit::client::Installation {
        product_id: "1207658924".into(),
        user_id: "48628349957132247".into(),
        directory: install.to_string_lossy().into_owned(),
        platform: obelisk_core::Platform::Windows,
        total_bytes: 1,
        build_id: "1".into(),
        version_name: "1.0".into(),
        language: "en-US".into(),
        content_root: String::new(),
        script_interpreter: false,
        products: Vec::new(),
        dependencies: serde_json::from_value(serde_json::json!([{
            "id": "MSVC2017",
            "path": "__redist/MSVC2017/VC_redist.x86.exe",
            "arguments": "/install /quiet",
        }]))
        .unwrap(),
    };
    fs::write(
        install.join(obelisk_core::install::RECEIPT_FILE),
        serde_json::to_vec(&receipt).unwrap(),
    )
    .unwrap();
    let config = Configuration {
        home: locations.home.clone(),
        install: install.clone(),
        session_directory: locations.gog_sessions.clone(),
        helper: locations.helper.clone(),
        product_id: "1207658924".into(),
        platform: obelisk_core::Platform::Windows,
        title: "The Witcher".into(),
        prefix: Some(locations.wine_prefixes.join("gog-1207658924-windows")),
    };
    let windows = crate::installations::Receipt::<crate::gog::Gog> {
        product: crate::gog::Product {
            product_id: "1207658924".into(),
        },
        name: "The Witcher".into(),
        directory: install.to_string_lossy().into_owned(),
        platform: obelisk_core::Platform::Windows,
        total_bytes: 1,
        needs_verification: false,
    }
    .windows(&locations)
    .unwrap()
    .unwrap();

    let wine = ready(&windows, &config, &locations).unwrap();

    // The supervisor starts the game its configuration names: the one
    // readied.
    let started = config.game();
    assert_eq!(
        wine::Installation::of(
            &locations,
            config.profile_game(),
            config.platform,
            started.install
        )
        .unwrap(),
        Some(windows)
    );
    assert!(config.runs_in(Some(&wine)));
    assert_eq!(
        calls(&locations.wine_prefixes),
        [
            "wineboot --init".to_owned(),
            format!("{} /install /quiet", redistributable.display()),
        ]
    );
}
