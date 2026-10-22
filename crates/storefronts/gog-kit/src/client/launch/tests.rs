use super::*;
use crate::client::download::Dependency;
use crate::manifest::Product;
use serde_json::json;
use std::fs;

fn installation(directory: &Path, platform: Platform, content_root: &str) -> Installation {
    Installation {
        product_id: "1207658924".into(),
        user_id: "48628349957132247".into(),
        directory: directory.to_string_lossy().into_owned(),
        platform,
        total_bytes: 1,
        build_id: "57325240911024496".into(),
        version_name: "1.5 (gog-3)".into(),
        language: "fr-FR".into(),
        content_root: content_root.into(),
        script_interpreter: true,
        products: vec![Product {
            product_id: "1207658924".into(),
            name: "The Witcher".into(),
            temp_executable: String::new(),
            temp_arguments: String::new(),
        }],
        dependencies: vec![Dependency {
            id: "MSVC2017".into(),
            path: "__redist/MSVC2017/VC_redist.x86.exe".into(),
            arguments: "/install /quiet".into(),
        }],
    }
}

fn write(path: &Path, bytes: &[u8]) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, bytes).unwrap();
}

fn info(tasks: serde_json::Value) -> Vec<u8> {
    serde_json::to_vec(
        &json!({"gameId": "1207658924", "rootGameId": "1207658924", "playTasks": tasks}),
    )
    .unwrap()
}

#[test]
fn gog_spells_its_builds_as_obelisk_and_its_content_system_do() {
    assert_eq!(crate::client::content_platform(Platform::Mac), "osx");
    assert_eq!(
        crate::client::content_platform(Platform::Windows),
        "windows"
    );
    let folder = tempfile::tempdir().unwrap();
    let written = serde_json::to_value(installation(folder.path(), Platform::Mac, "")).unwrap();
    assert_eq!(written["platform"], "macos");
    let read: Installation = serde_json::from_value(written).unwrap();
    assert_eq!(read.platform, Platform::Mac);
    assert_eq!(
        crate::client::installation_directory_name("1", Platform::Windows),
        "gog-1-windows"
    );
}

#[test]
fn a_windows_build_plays_its_primary_task() {
    let folder = tempfile::tempdir().unwrap();
    let root = folder.path().join("gog-1207658924-windows");
    write(&root.join("System/witcher.exe"), b"MZ");
    write(&root.join("launcher.exe"), b"MZ");
    write(
        &root.join("goggame-1207658924.info"),
        &info(json!([
            {"category": "launcher", "path": "launcher.exe", "type": "FileTask"},
            {"category": "game", "isPrimary": true, "path": "System\\witcher.exe",
             "arguments": "-skipintro \"-name with space\"", "type": "FileTask"},
            {"category": "document", "link": "https://www.gog.com", "type": "URLTask"},
        ])),
    );
    write(
        &root.join(obelisk_core::install::RECEIPT_FILE),
        &serde_json::to_vec(&installation(&root, Platform::Windows, "")).unwrap(),
    );
    let launch = installed_launch("1207658924", Platform::Windows, &root).unwrap();
    let root = root.canonicalize().unwrap();
    assert_eq!(
        Path::new(&launch.target.executable),
        root.join("System/witcher.exe")
    );
    assert_eq!(launch.target.arguments, ["-skipintro", "-name with space"]);
    assert_eq!(
        Path::new(&launch.target.working_directory),
        root.join("System")
    );

    let prerequisites = launch.prerequisites(|path| format!("Z:{}", path.display()));
    assert_eq!(prerequisites.len(), 2);
    let isi = &prerequisites[0];
    assert_eq!(isi.program, root.join("__redist/ISI/scriptinterpreter.exe"));
    assert!(isi.arguments.starts_with(&format!(
        "/VERYSILENT \"/DIR=Z:{}\" /Language=French",
        root.display()
    )));
    assert!(isi.arguments.contains("/ProductId=1207658924"));
    assert!(isi.arguments.contains("/versionName=\"1.5 (gog-3)\""));
    assert!(isi.arguments.ends_with(&format!(
        "\"/supportDir=Z:{}\"",
        root.join("gog-support/1207658924").display()
    )));
    assert_eq!(prerequisites[1].id, "MSVC2017");
    assert_eq!(
        prerequisites[1].program,
        root.join("__redist/MSVC2017/VC_redist.x86.exe")
    );
}

#[test]
fn the_played_program_is_read_from_an_info_file_before_installing() {
    assert_eq!(
        info_files("1207658924"),
        [
            "goggame-1207658924.info",
            "Contents/Resources/goggame-1207658924.info"
        ]
    );
    let tasks = info(json!([
        {"category": "tool", "path": "Tools\\editor", "type": "FileTask"},
        {"category": "game", "path": "Contents\\MacOS\\Game", "type": "FileTask"},
    ]));
    assert_eq!(
        played_program(&tasks).as_deref(),
        Some("Contents/MacOS/Game")
    );
    assert_eq!(played_program(b"not json"), None);
}

#[test]
fn a_mac_build_reads_its_bundles_resources() {
    let folder = tempfile::tempdir().unwrap();
    let root = folder.path().join("gog-1207658924-macos");
    let bundle = root.join("Game.app");
    write(&bundle.join("Contents/MacOS/Game"), b"\xcf\xfa\xed\xfe");
    write(
        &bundle.join("Contents/Resources/goggame-1207658924.info"),
        &info(json!([{"category": "game", "path": "Contents/MacOS/Game", "type": "FileTask"}])),
    );
    let mut installed = installation(&root, Platform::Mac, "Game.app");
    installed.dependencies.clear();
    write(
        &root.join(obelisk_core::install::RECEIPT_FILE),
        &serde_json::to_vec(&installed).unwrap(),
    );
    let launch = installed_launch("1207658924", Platform::Mac, &root).unwrap();
    assert_eq!(
        Path::new(&launch.target.executable),
        bundle.canonicalize().unwrap().join("Contents/MacOS/Game")
    );
    assert!(
        launch
            .prerequisites(|path| path.display().to_string())
            .is_empty()
    );
}

#[test]
fn an_installation_without_its_receipt_or_task_does_not_launch() {
    let folder = tempfile::tempdir().unwrap();
    let root = folder.path().join("gog-1207658924-windows");
    fs::create_dir_all(&root).unwrap();
    let refused = |result: Result<InstalledLaunch, GogError>, key: &str| match result {
        Err(GogError::Service { message }) => assert_eq!(message, key),
        _ => panic!("expected {key}"),
    };
    refused(
        installed_launch("1207658924", Platform::Windows, &root),
        "backend.gog.launch.incompleteInstallation",
    );
    write(
        &root.join(obelisk_core::install::RECEIPT_FILE),
        &serde_json::to_vec(&installation(&root, Platform::Windows, "")).unwrap(),
    );
    refused(
        installed_launch("1207658924", Platform::Mac, &root),
        "backend.gog.launch.incompleteInstallation",
    );
    refused(
        installed_launch("1207658924", Platform::Windows, &root),
        "backend.gog.launch.noPlayTask",
    );
    write(
        &root.join("goggame-1207658924.info"),
        &info(json!([{"category": "game", "path": "..\\outside.exe", "type": "FileTask"}])),
    );
    refused(
        installed_launch("1207658924", Platform::Windows, &root),
        "backend.gog.launch.pathOutsideGame",
    );
}
