//! EGS Cloud behind the Cloud sync: the shared suites over an in-memory Epic
//! Cloud, and what only Epic has: its listing filters, size limits, the files
//! it republishes and changed Cloud rules.
use super::fake::{Call, FakeCloud};
use super::remote::EpicListing;
use super::store::Pending;
use super::*;
use crate::cloud::record::tests::record_suite;
use crate::cloud::sync::{
    Work,
    tests::{Harness, cloud_suite, synced},
};
use crate::cloud::view::{Listing as _, Views};
use crate::supervisor::client::no_other_session;
use cloud_vault::{AccountIdentity, StoreScope};
use std::{
    fs,
    path::{Path, PathBuf},
};

const ACCOUNT: &str = "0123456789abcdef0123456789abcdef";

/// The game's saves: in the Mac home, or in a Windows build's profile.
pub(crate) fn rules() -> CloudRules {
    CloudRules {
        cloud_save_folder: Some("{AppData}/Game".into()),
        cloud_save_folder_mac: Some("{Home}/Saves".into()),
        include: vec!["*.sav".into()],
        exclude: Vec::new(),
    }
}

pub(crate) struct EpicGame {
    _temporary: tempfile::TempDir,
    pub config: Configuration,
    pub cloud: Arc<FakeCloud>,
    pub control: Arc<Control>,
}

impl Harness for EpicGame {
    type Store = Adapter<FakeCloud>;
    const NO_RULES: UnavailableReason = UnavailableReason::Unsupported;

    fn game() -> Self {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().to_path_buf();
        let home = root.join("home");
        let config = Configuration {
            root: root.join("archive"),
            install: home.join("Game"),
            session_directory: root.join("sessions"),
            helper: root.join("helper"),
            home,
            namespace: "namespace".into(),
            catalog_item_id: "catalog".into(),
            app_name: "game".into(),
            platform: Platform::Mac,
            language: "en".into(),
            title: String::new(),
            prefix: None,
        };
        for directory in [
            config.home.join("Saves"),
            config.install.clone(),
            config.session_directory.clone(),
        ] {
            fs::create_dir_all(directory).unwrap();
        }
        Self {
            control: Arc::new(Control::new(|_| {})),
            cloud: FakeCloud::new(ACCOUNT, Some(rules())),
            config,
            _temporary: temporary,
        }
    }
    fn control(&self) -> &Arc<Control> {
        &self.control
    }
    fn open(&self) -> Session<FakeCloud> {
        open(
            self.config.clone(),
            ACCOUNT.into(),
            Arc::clone(&self.cloud),
            Arc::clone(&self.control),
            uuid::Uuid::new_v4().to_string(),
        )
        .unwrap()
    }
    fn save(&self) -> PathBuf {
        match &self.config.prefix {
            Some(prefix) => crate::wine::profile(prefix).join("AppData/Local/Game/slot.sav"),
            None => self.config.home.join("Saves").join("slot.sav"),
        }
    }
    fn slot(&self) -> &'static str {
        "slot.sav"
    }
    fn install(&self) -> PathBuf {
        self.config.install.clone()
    }
    fn publish(&self, bytes: &[u8]) {
        self.cloud.publish(&[("slot.sav", bytes)]);
    }
    fn cloud(&self) -> Option<Vec<u8>> {
        self.cloud.files().get("slot.sav").cloned()
    }
    fn uploads(&self) -> usize {
        self.cloud.uploads()
    }
    fn fail_next(&self, transient: bool) {
        self.cloud.fail_next(
            Call::List,
            if transient {
                EpicError::Network
            } else {
                EpicError::UnsupportedContent
            },
        );
    }
    fn failure_key(&self) -> String {
        Error::Epic(EpicError::UnsupportedContent).to_string()
    }
    fn set_rules(&self, available: bool) {
        self.cloud.set_rules(available.then(rules));
    }
    fn run_in(&mut self, prefix: &Path) {
        self.config.platform = Platform::Windows;
        self.config.prefix = Some(prefix.to_owned());
        fs::create_dir_all(Harness::save(self).parent().unwrap()).unwrap();
    }
    async fn archive(&self) -> Result<()> {
        archive_with(self.config.clone(), self.cloud.as_ref()).await
    }
    async fn maintain(&self, alive: &SessionAlive<'_>) -> Result<()> {
        let control = Arc::clone(&self.control);
        maintain_with(self.config.clone(), Arc::clone(&self.cloud), control, alive).await
    }
    fn configuration(&self) -> Configuration {
        self.config.clone()
    }
    fn account_id(&self) -> &str {
        ACCOUNT
    }
    fn root(&self) -> PathBuf {
        self.config.root.clone()
    }
    fn scope(&self) -> StoreScope {
        self.config.scope(ACCOUNT).into()
    }
    fn account(&self) -> AccountIdentity {
        ACCOUNT.into()
    }
}

mod suite {
    super::cloud_suite!(super::EpicGame);
    super::record_suite!(super::EpicGame);
}

fn fixture() -> (tempfile::TempDir, Vault, Context) {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path();
    let home = root.join("home");
    let install = home.join("Game");
    let session_directory = root.join("sessions");
    for directory in [
        home.join("Saves"),
        install.clone(),
        session_directory.clone(),
    ] {
        fs::create_dir_all(directory).unwrap();
    }
    let config = Configuration {
        root: root.join("archive"),
        home,
        install,
        session_directory,
        helper: root.join("helper"),
        namespace: "namespace".into(),
        catalog_item_id: "catalog".into(),
        app_name: "game".into(),
        platform: Platform::Mac,
        language: "en".into(),
        title: String::new(),
        prefix: None,
    };
    let mut vault = Vault::open(&config.root, config.scope(ACCOUNT)).unwrap();
    vault.set_preferences(Preferences {
        account_enabled: Some(true),
        game_enabled: Some(true),
    });
    let context = Context {
        config,
        account_id: ACCOUNT.into(),
        rules: CloudRules {
            cloud_save_folder: None,
            cloud_save_folder_mac: Some("{Home}/Saves".into()),
            include: vec!["*.sav".into()],
            exclude: vec!["excluded.sav".into()],
        },
        pending: None,
        installation_removed: false,
    };
    context.save().unwrap();
    (temporary, vault, context)
}
fn staged(vault: &Vault, name: &str, bytes: &[u8]) -> Manifest {
    Manifest([(name.to_owned(), vault.stage(bytes, 2).unwrap())].into())
}

#[test]
fn uploads_preserve_excluded_remote_bytes_and_apply_managed_deletions() {
    let (_temporary, vault, context) = fixture();
    let remote: BTreeMap<String, Vec<u8>> = [
        ("deleted.sav".into(), b"old save".to_vec()),
        ("excluded.sav".into(), b"other platform".to_vec()),
        ("settings.ini".into(), b"foreign config".to_vec()),
    ]
    .into();
    let listing = epic_kit::cloud::CloudListing::of("original".into(), ACCOUNT.into(), &remote);
    let managed = EpicListing::new(listing, &context.rules).unwrap();
    let saves: Vec<_> = managed.saves().into_iter().map(|save| save.name).collect();
    assert_eq!(saves, ["deleted.sav"]);
    let local = staged(&vault, "new.sav", b"new bytes");
    let upload = upload_files(&vault, &context.rules, remote.clone(), &local).unwrap();
    assert_eq!(upload["excluded.sav"], b"other platform");
    assert_eq!(upload["settings.ini"], b"foreign config");
    assert_eq!(upload["new.sav"], b"new bytes");
    assert!(!upload.contains_key("deleted.sav"));
    let excluded = staged(&vault, "excluded.sav", b"wrong local selection");
    assert!(matches!(
        upload_files(&vault, &context.rules, remote, &excluded),
        Err(Error::Unsupported)
    ));
}

#[tokio::test]
async fn a_second_sync_with_equal_saves_fetches_nothing() {
    let game = EpicGame::game();
    synced(&game, b"progress").await;
    let first = game.cloud.fetched();
    game.open().prepare(&no_other_session).await.unwrap();
    assert_eq!(game.cloud.fetched(), first);

    // A newer Cloud save is read once, then stands in from the archive.
    game.cloud.publish(&[("slot.sav", b"cloud progress")]);
    game.open().prepare(&no_other_session).await.unwrap();
    game.open().prepare(&no_other_session).await.unwrap();

    assert_eq!(game.cloud.fetched()[first.len()..], ["slot.sav"]);
    assert_eq!(fs::read(Harness::save(&game)).unwrap(), b"cloud progress");
}

#[tokio::test]
async fn a_cloud_that_changes_during_a_read_refuses_the_view_as_a_conflict() {
    let game = EpicGame::game();
    game.cloud.publish(&[("slot.sav", b"progress")]);
    game.cloud
        .publish_during_read(&[("slot.sav", b"other device")]);
    let mut adapter = Adapter {
        config: game.config.clone(),
        account: ACCOUNT.into(),
        api: Arc::clone(&game.cloud),
        context: None,
        kept: None,
    };
    let (mut vault, _) = record::locked::<Context>(&game.config, ACCOUNT).unwrap();
    adapter.connect(&game.control).await.unwrap();
    adapter.rules(&mut (), &mut vault, true).await.unwrap();

    let read = Views::default()
        .read(
            &mut adapter.remote(&mut (), &game.control).unwrap(),
            &vault,
            &game.control,
        )
        .await;

    assert!(matches!(read, Err(Error::Epic(EpicError::CloudConflict))));
    assert!(adapter.kept.is_none());
}

#[tokio::test]
async fn unmanaged_cloud_files_are_read_with_the_view_and_republished() {
    let game = EpicGame::game();
    fs::write(Harness::save(&game), b"progress").unwrap();
    game.cloud
        .publish(&[("slot.sav", b"progress"), ("settings.ini", b"other device")]);
    let mut sync = game.open();
    sync.prepare(&no_other_session).await.unwrap();
    let first = game.cloud.fetched();
    assert!(first.iter().any(|name| name == "settings.ini"));
    sync.started().unwrap();
    fs::write(Harness::save(&game), b"new progress").unwrap();
    sync.finished(true).await;

    assert_eq!(game.cloud.uploads(), 1);
    assert_eq!(
        game.cloud.files(),
        BTreeMap::from([
            ("settings.ini".into(), b"other device".to_vec()),
            ("slot.sav".into(), b"new progress".to_vec()),
        ])
    );
    // The exit sync read them again under its own listing.
    assert_eq!(game.cloud.fetched()[first.len()..], ["settings.ini"]);
}

#[tokio::test]
async fn changed_epic_cloud_rules_need_a_dedicated_conversion() {
    let game = EpicGame::game();
    fs::write(Harness::save(&game), b"progress").unwrap();
    game.cloud.publish(&[("slot.sav", b"progress")]);
    game.open().prepare(&no_other_session).await.unwrap();
    let mut changed = rules();
    changed.include.push("*.cfg".into());
    game.cloud.set_rules(Some(changed));

    let refused = maintain_with(
        game.config.clone(),
        Arc::clone(&game.cloud),
        Arc::clone(&game.control),
        &no_other_session,
    )
    .await;

    assert!(matches!(refused, Err(Error::Unsupported)));
    assert_eq!(game.cloud.uploads(), 0);
}

#[test]
fn save_name_scan_refuses_links_before_adopting_or_reading_bytes() {
    let (_temporary, vault, context) = fixture();
    let outside = context.config.home.join("private.sav");
    fs::write(&outside, b"private bytes").unwrap();
    std::os::unix::fs::symlink(&outside, context.config.home.join("Saves/slot.sav")).unwrap();
    assert!(matches!(
        store::bindings(&context, &vault, None, true),
        Err(Error::Storage)
    ));
    assert!(vault.local().is_none());
    assert_eq!(fs::read(outside).unwrap(), b"private bytes");
}

#[test]
fn unsupported_mac_rules_allow_only_a_new_unmanaged_game_to_start_without_cloud() {
    let (_temporary, _vault, mut context) = fixture();
    let mut rules = context.rules.clone();
    rules.cloud_save_folder_mac = Some("{AppData}/Game".into());
    context.config.root = context.config.root.with_file_name("fresh-archive");
    let mut vault = Vault::open(&context.config.root, context.config.scope(ACCOUNT)).unwrap();
    assert!(!supports_rules(&context.config, ACCOUNT, &rules, &vault, false).unwrap());
    assert!(matches!(
        supports_rules(&context.config, ACCOUNT, &rules, &vault, true),
        Err(Error::Epic(EpicError::UnsupportedContent))
    ));
    vault
        .game_started(&uuid::Uuid::new_v4().to_string())
        .unwrap();
    assert!(matches!(
        supports_rules(&context.config, ACCOUNT, &rules, &vault, false),
        Err(Error::Epic(EpicError::UnsupportedContent))
    ));
}

#[test]
fn an_epic_record_the_previous_version_saved_keeps_loading() {
    let (_temporary, _vault, context) = fixture();
    let config = &context.config;
    let saved = serde_json::json!({
        "config": {
            "root": config.root,
            "home": config.home,
            "install": config.install,
            "session_directory": config.session_directory,
            "helper": config.helper,
            "namespace": "namespace",
            "catalog_item_id": "catalog",
            "app_name": "game",
            "platform": "Mac",
            "language": "en",
        },
        "rules": {
            "cloud_save_folder": null,
            "cloud_save_folder_mac": "{Home}/Saves",
            "include": ["*.sav"],
            "exclude": ["excluded.sav"],
        },
        "account_id": ACCOUNT,
        "pending": {"phase": "Upload", "desired": {}},
        "installation_removed": true,
    });
    cloud_vault::journal::write(&Context::path(config, ACCOUNT).unwrap(), &saved).unwrap();

    let loaded = Context::load(config, ACCOUNT).unwrap().unwrap();

    assert_eq!(
        loaded.work(),
        Work {
            pending: Some(Pending::Upload {
                desired: Manifest::default(),
            }),
            installation_removed: true,
        }
    );
}

#[test]
fn an_epic_cloud_record_keeps_epic_s_spelling() {
    let (_temporary, _vault, mut context) = fixture();
    let written = serde_json::to_value(&context).unwrap();
    assert_eq!(written["config"]["platform"], "Mac");
    let read: Context = serde_json::from_value(written).unwrap();
    assert_eq!(read.config.platform, Platform::Mac);

    context.config.platform = Platform::Windows;
    context.config.prefix = Some(context.config.root.join("prefix"));
    let written = serde_json::to_value(&context).unwrap();
    assert_eq!(written["config"]["platform"], "Windows");
}

#[test]
fn a_game_without_a_cloud_record_records_its_removal_even_behind_a_symbolic_link() {
    let game = EpicGame::game();
    // The Cloud archive refuses a path through a symbolic link, but this
    // game never gave it saves to keep.
    let linked = game.config.home.with_file_name("linked");
    std::os::unix::fs::symlink(&game.config.home, &linked).unwrap();
    let config = Configuration {
        install: linked.join("Game"),
        ..game.config.clone()
    };
    fs::remove_dir_all(&game.config.install).unwrap();

    record::removed::<Context>(&config, ACCOUNT).unwrap();

    assert!(Context::load(&config, ACCOUNT).unwrap().is_none());
}

#[test]
fn archive_errors_name_the_storefront_that_holds_the_saves() {
    assert!(matches!(Error::from(cloud_vault::Error::Busy), Error::Busy));
    let shared = Error::from(cloud_vault::Error::OtherStoreBusy);
    assert_eq!(shared.to_string(), "backend.cloud.sharedSavesInUse");
    assert_eq!(
        Error::from(cloud_vault::Error::Stale).to_string(),
        "backend.epic.cloud.storage"
    );
}
