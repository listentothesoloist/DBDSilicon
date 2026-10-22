//! The Cloud view through its interface, against an in-memory storefront
//! Cloud: archive reuse, one fetch per read, the confirming listing,
//! cancellation, the changed files and the confirmed upload.
use super::*;
use crate::cloud::{Stop, sync::Kind};
use cloud_vault::{Preferences, Scope, StoreScope, UploadState};
use sha1::{Digest, Sha1};
use std::{cell::RefCell, path::PathBuf};

const SCOPE: Scope = Scope {
    steam_id: 0x0110_0001_0000_002a,
    app_id: 42,
    cloud_app_id: 42,
};

#[derive(Debug, PartialEq, Eq)]
enum Failed {
    Stopped(Stop),
    Vault(String),
    Changed,
    Offline,
    Conflict,
    Recovery,
    Unsupported,
}
impl From<Stop> for Failed {
    fn from(stop: Stop) -> Self {
        Self::Stopped(stop)
    }
}
impl From<cloud_vault::Error> for Failed {
    fn from(error: cloud_vault::Error) -> Self {
        Self::Vault(error.to_string())
    }
}
impl sync::Failure for Failed {
    fn kind(&self) -> Kind {
        match self {
            Self::Stopped(_) => Kind::Cancelled,
            Self::Changed | Self::Offline => Kind::Transient,
            Self::Conflict => Kind::Conflict,
            Self::Unsupported => Kind::Unavailable,
            Self::Vault(_) | Self::Recovery => Kind::Blocking,
        }
    }
    fn key(&self) -> String {
        format!("{self:?}")
    }
    fn timed_out() -> Self {
        Self::Offline
    }
    fn conflict() -> Self {
        Self::Conflict
    }
    fn recovery() -> Self {
        Self::Recovery
    }
    fn unsupported() -> Self {
        Self::Unsupported
    }
    fn changed() -> Self {
        Self::Changed
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct FakeListing {
    revision: u64,
    files: BTreeMap<String, (u64, Vec<u8>, u32)>,
}
impl Listing for FakeListing {
    fn revision(&self) -> String {
        format!("r{}", self.revision)
    }
    fn saves(&self) -> Vec<Listed<'_>> {
        self.files
            .iter()
            .map(|(name, (size, _, platforms))| Listed {
                name,
                size: *size,
                platforms: *platforms,
            })
            .collect()
    }
    fn holds(&self, name: &str, bytes: &[u8]) -> bool {
        self.files.get(name).is_some_and(|(size, sha1, _)| {
            *size == bytes.len() as u64 && Sha1::digest(bytes).as_slice() == sha1
        })
    }
}

fn listing(files: &BTreeMap<String, (Vec<u8>, u32)>, revision: u64) -> FakeListing {
    FakeListing {
        revision,
        files: files
            .iter()
            .map(|(name, (bytes, platforms))| {
                let identity = (bytes.len() as u64, Sha1::digest(bytes).to_vec(), *platforms);
                (name.clone(), identity)
            })
            .collect(),
    }
}

/// One game's Cloud in memory, with counters for what a view asked of it.
struct FakeRemote {
    root: PathBuf,
    files: BTreeMap<String, (Vec<u8>, u32)>,
    revision: u64,
    lists: usize,
    fetches: usize,
    fetched: Vec<String>,
    /// Another device publishes this between a listing and its confirmation.
    race: Option<(String, Vec<u8>)>,
    /// What the storefront's record keeps of the last view.
    record: Option<Manifest>,
    adopted: Option<FakeListing>,
    /// A commit's listing also names this save.
    stray: Option<String>,
    /// A fetch leaves this save out yet succeeds.
    skipped: Option<String>,
    /// The vault's upload state when the journal ran, then when sending.
    states: RefCell<Vec<Option<UploadState>>>,
}

impl FakeRemote {
    fn new(root: PathBuf) -> Self {
        Self {
            root,
            files: BTreeMap::new(),
            revision: 0,
            lists: 0,
            fetches: 0,
            fetched: Vec::new(),
            race: None,
            record: None,
            adopted: None,
            stray: None,
            skipped: None,
            states: RefCell::new(Vec::new()),
        }
    }
    fn publish(&mut self, name: &str, bytes: &[u8], platforms: u32) {
        self.files.insert(name.into(), (bytes.to_vec(), platforms));
        self.revision += 1;
    }
}

fn state(root: &std::path::Path) -> Option<UploadState> {
    cloud_vault::peek(root, SCOPE).unwrap().pending
}

impl Remote for FakeRemote {
    type Failure = Failed;
    type Listing = FakeListing;
    const JOURNAL: &'static str = "fake-cloud";

    async fn list(&mut self) -> Result<FakeListing, Failed> {
        self.lists += 1;
        Ok(listing(&self.files, self.revision))
    }
    async fn unchanged(&mut self, listed: &FakeListing) -> Result<bool, Failed> {
        if let Some((name, bytes)) = self.race.take() {
            self.publish(&name, &bytes, 2);
        }
        Ok(listing(&self.files, self.revision) == *listed)
    }
    async fn fetch(
        &mut self,
        _: &FakeListing,
        names: &[&str],
        mut receive: impl FnMut(&str, Vec<u8>) -> Result<(), cloud_vault::Error>,
    ) -> Result<(), Failed> {
        self.fetches += 1;
        for name in names {
            if self.skipped.as_deref() == Some(*name) {
                continue;
            }
            self.fetched.push((*name).to_owned());
            receive(name, self.files[*name].0.clone())?;
        }
        Ok(())
    }
    fn adopt(&mut self, listed: FakeListing, view: &RemoteView, _: &Vault) -> Result<(), Failed> {
        self.record = Some(view.files.clone());
        self.adopted = Some(listed);
        Ok(())
    }
    fn adopted(&self) -> Option<&Manifest> {
        self.record.as_ref()
    }
    async fn commit(
        &mut self,
        change: Change<'_, Failed>,
    ) -> Result<Committed<FakeListing>, Failed> {
        let mut uploads = Vec::new();
        for (name, file) in change.uploads() {
            uploads.push((
                name.to_owned(),
                change.vault().read_content(file)?,
                file.platforms,
            ));
        }
        let deletes: Vec<String> = change.deletes().map(str::to_owned).collect();
        let (root, states) = (&self.root, &self.states);
        let (files, revision, stray) = (&mut self.files, &mut self.revision, &self.stray);
        change
            .send(
                || {
                    states.borrow_mut().push(state(root));
                    Ok(())
                },
                async || {
                    states.borrow_mut().push(state(root));
                    for (name, bytes, platforms) in uploads {
                        files.insert(name, (bytes, platforms));
                    }
                    for name in deletes {
                        files.remove(&name);
                    }
                    if let Some(stray) = stray {
                        files.insert(stray.clone(), (b"stray".to_vec(), 2));
                    }
                    *revision += 1;
                    Ok(listing(files, *revision))
                },
            )
            .await
    }
}

struct Fixture {
    _temporary: tempfile::TempDir,
    root: PathBuf,
    vault: Vault,
    control: Control,
}

fn fixture() -> Fixture {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().canonicalize().unwrap().join("cloud");
    let mut vault = Vault::open(&root, SCOPE).unwrap();
    vault.set_preferences(Preferences {
        account_enabled: Some(true),
        game_enabled: Some(true),
    });
    Fixture {
        _temporary: temporary,
        root,
        vault,
        control: Control::new(|_| {}),
    }
}

impl Fixture {
    fn remote(&self) -> FakeRemote {
        FakeRemote::new(self.root.clone())
    }
    fn staged(&self, files: &[(&str, &[u8])]) -> Manifest {
        Manifest(
            files
                .iter()
                .map(|(name, bytes)| ((*name).to_owned(), self.vault.stage(bytes, 2).unwrap()))
                .collect(),
        )
    }
    async fn send(
        &mut self,
        remote: &mut FakeRemote,
        local: &Manifest,
        view: &RemoteView,
    ) -> Result<(), Failed> {
        send(
            remote,
            &mut self.vault,
            SCOPE.steam_id.into(),
            local,
            view,
            &self.control,
        )
        .await
    }
}

#[tokio::test]
async fn unchanged_saves_reuse_archived_bytes_across_reads_and_restarts() {
    let game = fixture();
    let mut remote = game.remote();
    remote.publish("slot.sav", b"cloud v1", 7);
    let mut views = Views::default();

    let first = views
        .read(&mut remote, &game.vault, &game.control)
        .await
        .unwrap();
    views
        .read(&mut remote, &game.vault, &game.control)
        .await
        .unwrap();
    assert_eq!(remote.fetched, ["slot.sav"]);
    assert_eq!(remote.lists, 2);
    assert_eq!(first.files.0["slot.sav"].platforms, 7);
    assert_eq!(first.revision, "r1");

    // After a restart only the storefront's record still holds the view.
    let again = Views::default()
        .read(&mut remote, &game.vault, &game.control)
        .await
        .unwrap();
    assert_eq!(remote.fetched, ["slot.sav"]);
    assert_eq!(again, first);
}

#[tokio::test]
async fn stale_bytes_never_stand_in_for_a_listed_save() {
    let game = fixture();
    let mut remote = game.remote();
    remote.publish("slot.sav", b"cloud v1", 2);
    let mut views = Views::default();
    let first = views
        .read(&mut remote, &game.vault, &game.control)
        .await
        .unwrap();
    let blob = StoreScope::from(SCOPE)
        .directory(&game.root)
        .unwrap()
        .join("blobs")
        .join(first.files.0["slot.sav"].hash.as_str());
    std::fs::remove_file(&blob).unwrap();
    views
        .read(&mut remote, &game.vault, &game.control)
        .await
        .unwrap();
    assert_eq!(remote.fetched.len(), 2);

    // The same size with other bytes fails the listed hash.
    remote.publish("slot.sav", b"cloud v2", 2);
    let second = views
        .read(&mut remote, &game.vault, &game.control)
        .await
        .unwrap();
    assert_eq!(remote.fetched.len(), 3);
    assert_eq!(
        game.vault
            .read_content(&second.files.0["slot.sav"])
            .unwrap(),
        b"cloud v2"
    );
    remote.publish("slot.sav", b"cloud v3, longer", 2);
    let third = views
        .read(&mut remote, &game.vault, &game.control)
        .await
        .unwrap();
    assert_eq!(remote.fetched.len(), 4);
    assert_eq!(
        game.vault.read_content(&third.files.0["slot.sav"]).unwrap(),
        b"cloud v3, longer"
    );
}

#[tokio::test]
async fn fetch_runs_once_per_read_with_only_the_missing_saves_and_stages_their_listed_platforms() {
    let game = fixture();
    let mut remote = game.remote();
    remote.publish("a.sav", b"first", 7);
    remote.publish("b.sav", b"second", 1);
    let mut views = Views::default();

    let view = views
        .read(&mut remote, &game.vault, &game.control)
        .await
        .unwrap();
    assert_eq!(
        (remote.fetches, remote.fetched.clone()),
        (1, vec!["a.sav".into(), "b.sav".into()])
    );
    assert_eq!(view.files.0["a.sav"].platforms, 7);
    assert_eq!(view.files.0["b.sav"].platforms, 1);
    assert_eq!(
        game.vault.read_content(&view.files.0["b.sav"]).unwrap(),
        b"second"
    );

    remote.publish("c.sav", b"third", 2);
    views
        .read(&mut remote, &game.vault, &game.control)
        .await
        .unwrap();
    assert_eq!(remote.fetches, 2);
    assert_eq!(remote.fetched[2..], ["c.sav"]);

    // Nothing missing still asks once, with no names.
    views
        .read(&mut remote, &game.vault, &game.control)
        .await
        .unwrap();
    assert_eq!(remote.fetches, 3);
    assert_eq!(remote.fetched.len(), 3);
}

#[tokio::test]
async fn a_fetch_that_leaves_a_save_out_refuses_the_view() {
    let game = fixture();
    let mut remote = game.remote();
    remote.publish("a.sav", b"first", 2);
    remote.publish("b.sav", b"second", 2);
    remote.skipped = Some("b.sav".into());

    assert_eq!(
        Views::default()
            .read(&mut remote, &game.vault, &game.control)
            .await,
        Err(Failed::Vault(cloud_vault::Error::Integrity.to_string()))
    );
    assert!(remote.adopted.is_none());
}

#[tokio::test]
async fn a_listing_that_changes_while_reading_refuses_the_view() {
    let game = fixture();
    let mut remote = game.remote();
    remote.publish("slot.sav", b"cloud v1", 2);
    remote.race = Some(("slot.sav".into(), b"cloud v2".to_vec()));
    let mut views = Views::default();

    assert_eq!(
        views.read(&mut remote, &game.vault, &game.control).await,
        Err(Failed::Changed)
    );
    assert!(remote.adopted.is_none());

    let view = views
        .read(&mut remote, &game.vault, &game.control)
        .await
        .unwrap();
    assert_eq!(
        game.vault.read_content(&view.files.0["slot.sav"]).unwrap(),
        b"cloud v2"
    );
    assert_eq!(remote.adopted.unwrap().revision, 2);
}

#[tokio::test]
async fn an_empty_listing_forgets_what_the_last_view_read() {
    let game = fixture();
    let mut remote = game.remote();
    remote.publish("slot.sav", b"cloud v1", 2);
    let mut views = Views::default();
    views
        .read(&mut remote, &game.vault, &game.control)
        .await
        .unwrap();
    remote.files.clear();
    remote.revision += 1;

    let view = views
        .read(&mut remote, &game.vault, &game.control)
        .await
        .unwrap();

    assert!(view.files.0.is_empty());
    assert!(views.downloaded.0.is_empty());
    assert_eq!(remote.record, Some(Manifest::default()));
}

#[tokio::test]
async fn cancellation_and_revocation_stop_the_read_before_any_fetch() {
    let game = fixture();
    let mut remote = game.remote();
    remote.publish("slot.sav", b"cloud v1", 2);
    for (stop, expected) in [
        (Control::cancel as fn(&Control), Stop::Cancelled),
        (Control::revoke, Stop::Revoked),
    ] {
        let control = Control::new(|_| {});
        stop(&control);
        assert_eq!(
            Views::default()
                .read(&mut remote, &game.vault, &control)
                .await,
            Err(Failed::Stopped(expected))
        );
    }
    assert_eq!(remote.fetches, 0);
    assert!(remote.adopted.is_none());
}

#[test]
fn change_lists_uploads_for_new_or_different_versions_and_deletes_for_missing_names() {
    let mut game = fixture();
    let local = game.staged(&[("a.sav", b"same"), ("b.sav", b"edited"), ("c.sav", b"new")]);
    let view = RemoteView {
        revision: "r1".into(),
        files: game.staged(&[("a.sav", b"same"), ("b.sav", b"older"), ("d.sav", b"gone")]),
    };
    let change = Change::<Failed> {
        vault: &mut game.vault,
        account: SCOPE.steam_id.into(),
        local: &local,
        view: &view,
        journal: FakeRemote::JOURNAL,
        control: &game.control,
        failure: PhantomData,
    };

    let uploads: Vec<_> = change.uploads().map(|(name, _)| name).collect();
    let deletes: Vec<_> = change.deletes().collect();

    assert_eq!(uploads, ["b.sav", "c.sav"]);
    assert_eq!(deletes, ["d.sav"]);
}

#[tokio::test]
async fn an_upload_confirms_from_the_committed_listing_and_moves_the_baseline() {
    let mut game = fixture();
    let mut remote = game.remote();
    let view = Views::default()
        .read(&mut remote, &game.vault, &game.control)
        .await
        .unwrap();
    let local = game.staged(&[("a.sav", b"first"), ("b.sav", b"second")]);

    game.send(&mut remote, &local, &view).await.unwrap();

    assert_eq!(remote.files["b.sav"].0, b"second");
    let confirmed = game.vault.baseline().unwrap();
    assert_eq!(confirmed.files, local);
    assert_eq!(confirmed.revision, "r1");
    assert!(game.vault.pending().is_none());
    assert_eq!(remote.adopted.unwrap().revision, 1);
    assert_eq!(remote.record, Some(local));
    // Nothing is read back.
    assert_eq!(remote.fetches, 1);
}

#[tokio::test]
async fn a_committed_listing_naming_an_unsent_save_leaves_the_upload_pending() {
    let mut game = fixture();
    let mut remote = game.remote();
    let view = Views::default()
        .read(&mut remote, &game.vault, &game.control)
        .await
        .unwrap();
    let local = game.staged(&[("a.sav", b"first")]);
    remote.stray = Some("stray.sav".into());

    assert_eq!(
        game.send(&mut remote, &local, &view).await,
        Err(Failed::Changed)
    );

    assert!(game.vault.baseline().is_none());
    assert!(
        game.vault
            .pending()
            .is_some_and(|pending| pending.may_have_committed)
    );
    assert_eq!(remote.adopted.unwrap().revision, 0);
}

#[tokio::test]
async fn the_journal_runs_after_the_upload_is_queued_and_before_it_may_commit() {
    let mut game = fixture();
    let mut remote = game.remote();
    let view = Views::default()
        .read(&mut remote, &game.vault, &game.control)
        .await
        .unwrap();
    let local = game.staged(&[("a.sav", b"first")]);

    game.send(&mut remote, &local, &view).await.unwrap();

    assert_eq!(
        remote.states.into_inner(),
        [
            Some(UploadState::Queued),
            Some(UploadState::MayHaveCommitted)
        ]
    );
}

#[tokio::test]
async fn forgetting_drops_the_reuse_cache() {
    let game = fixture();
    let mut remote = game.remote();
    remote.publish("slot.sav", b"cloud v1", 2);
    let mut views = Views::default();
    for _ in 0..2 {
        views
            .read(&mut remote, &game.vault, &game.control)
            .await
            .unwrap();
        // Only this operation's reads may stand in.
        remote.record = None;
    }
    assert_eq!(remote.fetched.len(), 1);

    views.forget();
    views
        .read(&mut remote, &game.vault, &game.control)
        .await
        .unwrap();

    assert_eq!(remote.fetched.len(), 2);
}
