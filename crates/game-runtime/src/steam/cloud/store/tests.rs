use super::*;
use crate::cloud::record::Record as _;
use steam_kit::cloud::CloudFile;

const ACCOUNT: u64 = 0x0110_0001_0000_002a;
const APP: u32 = 42;
const SLOT: &str = "%GameInstall%saves/slot.sav";

struct Fixture {
    _temporary: tempfile::TempDir,
    config: Config,
}

impl Fixture {
    fn new() -> Self {
        let temporary = tempfile::tempdir().unwrap();
        let directory = temporary.path().canonicalize().unwrap();
        let config = Config {
            root: directory.join("cloud"),
            home: directory.join("home"),
            install: directory.join("game"),
            app_id: APP,
            prefix: None,
        };
        for path in [&config.root, &config.home, &config.install] {
            crate::files::private_directory(path).unwrap();
        }
        Self {
            _temporary: temporary,
            config,
        }
    }

    fn save(&self, name: &str, bytes: &[u8]) {
        let path = self.config.install.join("saves").join(name);
        crate::files::private_directory(path.parent().unwrap()).unwrap();
        fs::write(path, bytes).unwrap();
    }

    fn vault(&self) -> Vault {
        Vault::open(&self.config.root, scope(&self.config, ACCOUNT)).unwrap()
    }

    fn context(&self) -> Context {
        let mut context = Context::new(&self.config, ACCOUNT, metadata());
        context.listing = listing();
        context
    }
}

/// One AutoCloud `savefiles` rule rooted at the install directory.
fn save_rule(path: &str, pattern: &str, recursive: &str) -> CloudMetadata {
    CloudMetadata::Object(vec![(
        "savefiles".into(),
        CloudMetadata::Object(vec![(
            "0".into(),
            CloudMetadata::Object(
                [
                    ("root", "GameInstall"),
                    ("path", path),
                    ("pattern", pattern),
                    ("recursive", recursive),
                ]
                .into_iter()
                .map(|(key, value)| (key.into(), CloudMetadata::Value(value.into())))
                .collect(),
            ),
        )]),
    )])
}

fn metadata() -> CloudMetadata {
    save_rule("saves", "*.sav", "1")
}

fn listing() -> CloudListing {
    CloudListing {
        steam_id: ACCOUNT,
        app_id: APP,
        change_number: 7,
        files: Vec::new(),
    }
}

#[test]
fn a_record_the_previous_version_saved_keeps_loading() {
    let fixture = Fixture::new();
    let config = &fixture.config;
    let path = directory(config, ACCOUNT).unwrap().join("context.json");
    let saved = serde_json::json!({
        "config": {
            "root": config.root,
            "home": config.home,
            "install": config.install,
            "app_id": APP,
        },
        "account_id": ACCOUNT,
        "metadata": serde_json::to_value(metadata()).unwrap(),
        "listing": serde_json::to_value(listing()).unwrap(),
        "remote": {"revision": "8", "files": {}},
        "bindings": {},
        "needs_work": true,
        "desired": {},
        "transaction": null,
        "installation_removed": false,
        "applying": {"remote": {"revision": "8", "files": {}}, "project": true},
    });
    write(&path, &saved).unwrap();

    let mut context = Context::load(config, &ACCOUNT).unwrap().unwrap();

    assert!(matches!(
        context.work().pending,
        Some(Pending::Applying { project: true, .. })
    ));
    context.set_work(Work {
        pending: Some(Pending::Upload {
            desired: Manifest::default(),
        }),
        installation_removed: false,
    });
    context.save().unwrap();
    let mut written: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    assert_eq!(written["needs_work"], true);
    assert_eq!(written["desired"], serde_json::json!({}));
    assert!(written.get("applying").is_none());
    // The removal fence is never guessed from a record that lacks it.
    written
        .as_object_mut()
        .unwrap()
        .remove("installation_removed");
    assert!(serde_json::from_value::<Context>(written).is_err());
}

#[test]
fn archived_binding_cannot_reclassify_a_now_foreign_only_identity() {
    let fixture = Fixture::new();
    fixture.save("slot.sav", b"local progress");
    let mut context = fixture.context();
    let mut vault = fixture.vault();
    context.refresh_bindings(vault.local()).unwrap();
    vault.capture(&context.bindings).unwrap();
    context.listing.files.push(CloudFile {
        name: SLOT.into(),
        sha1: "00".repeat(20),
        size: 0,
        timestamp: 0,
        platforms: 1,
        persist_state: 0,
        reupload_requested: false,
    });

    context.refresh_bindings(vault.local()).unwrap();

    assert!(
        context.bindings.is_empty(),
        "Archived Mac bindings must not re-adopt a foreign-only identity"
    );
    assert_eq!(
        fs::read(fixture.config.install.join("saves/slot.sav")).unwrap(),
        b"local progress"
    );
}

#[test]
fn refreshing_remote_and_local_bindings_never_recreates_removed_install() {
    let fixture = Fixture::new();
    fixture.save("slot.sav", b"archived save");
    let mut context = fixture.context();
    let mut vault = fixture.vault();
    context.refresh_bindings(vault.local()).unwrap();
    vault.capture(&context.bindings).unwrap();
    context.listing.files.push(CloudFile {
        name: "%GameInstall%saves/nested/remote.sav".into(),
        sha1: "00".repeat(20),
        size: 0,
        timestamp: 0,
        platforms: 7,
        persist_state: 0,
        reupload_requested: false,
    });
    fs::remove_dir_all(&fixture.config.install).unwrap();

    context.refresh_bindings(vault.local()).unwrap();

    assert_eq!(context.bindings.len(), 2);
    assert_eq!(
        context.bindings[SLOT].path,
        fixture.config.install.join("saves/slot.sav")
    );
    assert_eq!(
        context.bindings["%GameInstall%saves/nested/remote.sav"].path,
        fixture.config.install.join("saves/nested/remote.sav")
    );
    assert!(!fixture.config.install.exists());
    assert!(matches!(context.ensure_parents(), Err(Failure::Recovery)));
    assert!(!fixture.config.install.exists());
}
