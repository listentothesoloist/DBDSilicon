use super::*;
use serde_json::json;

fn build_manifest(value: serde_json::Value) -> BuildManifest {
    serde_json::from_value(value).unwrap()
}

/// The Witcher's Mac build manifest as GOG serves it, trimmed.
fn witcher() -> BuildManifest {
    build_manifest(json!({
        "version": 2,
        "baseProductId": "1207658924",
        "platform": "osx",
        "products": [{"name": "The Witcher: Enhanced Edition", "productId": "1207658924",
                      "temp_executable": "", "temp_arguments": ""}],
        "buildId": "57325240911024496",
        "installDirectory": "The Witcher Enhanced Edition",
        "depots": [
            {"manifest": "24b59fcf4a04b94f6cd03dbae1fe5f3f", "size": 15_104_925_994_u64,
             "compressedSize": 10_318_796_398_u64, "productId": "1207658924", "languages": ["en-US"]},
            {"manifest": "47554788d74dcc53009bfd8b171324e6", "size": 996, "compressedSize": 330,
             "productId": "1207658924", "languages": ["en-US"], "isGogDepot": true},
        ],
    }))
}

#[test]
fn the_current_build_is_the_newest_of_the_default_branch() {
    let builds: Builds = serde_json::from_value(json!({"items": [
        {"build_id": "3", "branch": "beta", "generation": 2, "version_name": "beta", "link": "https://a/3"},
        {"build_id": "2", "branch": null, "generation": 2, "version_name": "1.2", "link": "https://a/2"},
        {"build_id": "1", "branch": null, "generation": 2, "version_name": "1.1", "link": "https://a/1"},
    ]}))
    .unwrap();
    let current = builds.current().unwrap();
    assert_eq!(current.version_name, "1.2");
    assert_eq!(
        current.build_id, "2",
        "an installation compares its build to it"
    );
    let legacy: Builds = serde_json::from_value(json!({"items": [
        {"build_id": "1", "branch": null, "generation": 1, "legacy_build_id": 7, "link": "https://a/1"},
    ]}))
    .unwrap();
    assert!(
        legacy.current().is_none(),
        "a generation 1 build is not installed"
    );
}

#[test]
fn a_mac_build_that_names_its_bundle_installs_inside_it() {
    assert_eq!(witcher().content_root().unwrap(), "");
    let mut stardew = witcher();
    stardew.install_directory = "Stardew Valley.app".into();
    assert_eq!(stardew.content_root().unwrap(), "Stardew Valley.app");
    stardew.install_directory = "../Escape.app".into();
    assert!(stardew.content_root().is_err());
}

#[test]
fn depots_follow_the_language_then_english() {
    let manifest = build_manifest(json!({
        "baseProductId": "1",
        "depots": [
            {"manifest": "a", "productId": "1", "languages": ["*"], "size": 1},
            {"manifest": "en", "productId": "1", "languages": ["en-US"], "size": 10},
            {"manifest": "fr", "productId": "1", "languages": ["fr-FR"], "size": 100},
            {"manifest": "dlc", "productId": "2", "languages": ["*"], "size": 1000},
            {"manifest": "other", "productId": "3", "languages": ["*"], "size": 10000},
        ],
    }));
    let owned = HashSet::from(["1", "2"]);
    let names = |language: &str| -> Vec<&str> {
        depots(&manifest, &owned, language)
            .iter()
            .map(|depot| depot.manifest.as_str())
            .collect()
    };
    assert_eq!(names("fr-FR"), ["a", "fr", "dlc"]);
    assert_eq!(
        names("fr"),
        ["a", "fr", "dlc"],
        "a bare language matches its region"
    );
    assert_eq!(
        names("de-DE"),
        ["a", "en", "dlc"],
        "English stands in for a missing language"
    );
    assert_eq!(names("en-US"), ["a", "en", "dlc"]);
}

#[test]
fn galaxy_paths_split_the_hash() {
    assert_eq!(
        galaxy_path("24b59fcf4a04b94f6cd03dbae1fe5f3f").unwrap(),
        "24/b5/24b59fcf4a04b94f6cd03dbae1fe5f3f"
    );
    assert_eq!(
        galaxy_path("24/b5/24b59fcf4a04b94f6cd03dbae1fe5f3f").unwrap(),
        "24/b5/24b59fcf4a04b94f6cd03dbae1fe5f3f"
    );
    for hash in [
        "",
        "../b5/24b59fcf4a04b94f6cd03dbae1fe5f3f",
        "zz",
        "aa/bb/24b59fcf4a04b94f6cd03dbae1fe5f3f",
    ] {
        assert!(galaxy_path(hash).is_err(), "{hash}");
    }
}

fn chunk(md5: &str) -> serde_json::Value {
    json!({"md5": md5, "size": 4, "compressedMd5": md5, "compressedSize": 12})
}

fn items(value: serde_json::Value) -> Vec<Item> {
    serde_json::from_value::<DepotManifest>(json!({"depot": {"items": value}, "version": 2}))
        .unwrap()
        .depot
        .items
}

#[test]
fn files_land_under_the_content_root_and_support_files_apart() {
    let md5 = "efd1e52118fbc32baf1a429b0b80fcad";
    let mut installed = Installed::default();
    installed
        .add(
            "Game.app",
            "1",
            items(json!([
                {"type": "DepotFile", "path": "Contents\\MacOS\\Game", "chunks": [chunk(md5)], "flags": ["executable"]},
                {"type": "DepotFile", "path": "setup.exe", "chunks": [chunk(md5)], "flags": ["support"]},
                {"type": "DepotDirectory", "path": "Contents/Resources"},
                {"type": "DepotLink", "path": "Contents/Frameworks/X.framework/X",
                 "target": "Contents/Frameworks/X.framework/Versions/A/X"},
            ])),
        )
        .unwrap();
    installed
        .add(
            "Game.app",
            DEPENDENCIES,
            items(json!([
                {"type": "DepotFile", "path": "__redist/ISI/scriptinterpreter.exe", "chunks": [chunk(md5)]},
                {"type": "DepotFile", "path": "ignored.txt", "chunks": [chunk(md5)], "flags": ["support"]},
            ])),
        )
        .unwrap();
    let paths: Vec<_> = installed
        .files
        .iter()
        .map(|file| file.path.as_str())
        .collect();
    assert_eq!(
        paths,
        [
            "Game.app/Contents/MacOS/Game",
            "gog-support/1/setup.exe",
            "Game.app/__redist/ISI/scriptinterpreter.exe",
        ]
    );
    assert_eq!(installed.files[2].source, DEPENDENCIES);
    assert_eq!(installed.directories, ["Game.app/Contents/Resources"]);
    assert_eq!(
        installed.links[0].target,
        "Game.app/Contents/Frameworks/X.framework/Versions/A/X"
    );
    installed.validate().unwrap();
    assert_eq!(installed.total_bytes(), 12);
}

#[test]
fn a_later_depot_replaces_a_file_of_the_same_path() {
    let mut installed = Installed::default();
    installed
        .add(
            "",
            "1",
            items(json!([{"type": "DepotFile", "path": "Data/A.pak", "chunks": []}])),
        )
        .unwrap();
    installed
        .add(
            "",
            "2",
            items(json!([{"type": "DepotFile", "path": "data\\a.pak", "chunks": []}])),
        )
        .unwrap();
    assert_eq!(installed.files.len(), 1);
    assert_eq!(installed.files[0].source, "2");
}

#[test]
fn validation_refuses_what_no_installation_may_hold() {
    let md5 = "efd1e52118fbc32baf1a429b0b80fcad";
    let refused = [
        json!([{"type": "DepotFile", "path": "../escape", "chunks": []}]),
        json!([{"type": "DepotFile", "path": ".obelisk-install.json", "chunks": []}]),
        json!([{"type": "DepotFile", "path": "a", "chunks": []},
               {"type": "DepotFile", "path": "a/b", "chunks": []}]),
        json!([{"type": "DepotFile", "path": "a", "chunks": [{"md5": "x", "size": 1, "compressedMd5": md5, "compressedSize": 1}]}]),
        json!([{"type": "DepotFile", "path": "a", "chunks": [{"md5": md5, "size": 0, "compressedMd5": md5, "compressedSize": 1}]}]),
        json!([{"type": "DepotLink", "path": "link", "target": "../../outside"}]),
        json!([{"type": "DepotLink", "path": "Versions/Current", "target": "Versions/A"},
               {"type": "DepotFile", "path": "Versions/Current/X", "chunks": []}]),
    ];
    for value in refused {
        let mut installed = Installed::default();
        let added = installed.add("", "1", items(value.clone()));
        assert!(added.is_err() || installed.validate().is_err(), "{value}");
    }
    let mut unknown = Installed::default();
    assert!(
        unknown
            .add("", "1", items(json!([{"type": "DepotPatch", "path": "a"}])))
            .is_err()
    );
}

#[test]
fn links_become_relative_to_their_folder() {
    assert_eq!(
        relative_target("a/b/link", "a/c/target"),
        "../../a/c/target"
    );
    assert_eq!(relative_target("link", "target"), "target");
}

#[test]
fn net_framework_xna_and_installer_packages_stay_out_of_wine() {
    let depot = |id: &str, path: &str| DependencyDepot {
        dependency_id: id.into(),
        manifest: String::new(),
        executable: DependencyExecutable {
            path: path.into(),
            arguments: String::new(),
        },
    };
    assert!(depot("MSVC2017_x64", "__redist/MSVC2017_x64/VC_redist.x64.exe").installs_in_wine());
    assert!(depot("DirectX", "__redist/DirectX/DXSETUP.exe").installs_in_wine());
    assert!(depot("DOSBox074", "").installs_in_wine());
    assert!(
        depot(
            "dotNetDesktopRuntime60_x64",
            "__redist/x/windowsdesktop-runtime.exe"
        )
        .installs_in_wine()
    );
    assert!(!depot("dotNet4", "__redist/dotNet4/dotNetFx40_Full_x86_x64.exe").installs_in_wine());
    assert!(!depot("XNA_40", "__redist/XNA_40/xnafx40_redist.msi").installs_in_wine());
    assert!(!depot("PHYSXLEGACY", "__redist/PHYSXLEGACY/PhysX.msi").installs_in_wine());
}
