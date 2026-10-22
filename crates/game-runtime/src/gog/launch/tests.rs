use super::*;
use crate::wine::tests::{installation, pe, target};
use std::{collections::HashMap, fs, path::Path};

#[test]
fn a_windows_build_on_the_linux_engine_takes_its_store_title_in_the_dock() {
    let (_directory, root) = installation();
    let install = root.join("gog-1207658924-windows");
    fs::create_dir_all(install.join("System")).unwrap();
    fs::write(install.join("System/witcher.exe"), pe()).unwrap();
    let prefix = root.join("Prefixes/gog-1207658924-windows-linux/prefix");
    let config = Configuration {
        home: root.join("home"),
        install: install.clone(),
        session_directory: root.join("GOGSessions"),
        helper: root.join("SteamNative/obelisk-steam-host"),
        product_id: "1207658924".into(),
        platform: obelisk_core::Platform::Windows,
        title: "The Witcher".into(),
        prefix: Some(prefix.clone()),
    };
    let wine = crate::wine::Config {
        prefix,
        settings: crate::launch_settings::Settings {
            engine: crate::wine::Engine::Linux,
            ..Default::default()
        },
        ..crate::wine::tests::config(&root)
    };

    let (target, architecture) = windows_target(
        &target(&install.join("System/witcher.exe"), &[]),
        &config,
        &wine,
        &HashMap::new(),
    )
    .unwrap();

    assert_eq!(architecture, Architecture::Arm64);
    assert_eq!(target.executable, Path::new("/bin/bash"));
    assert_eq!(
        target.environment[crate::wine::linux_engine::APP_NAME],
        "The Witcher"
    );
}
