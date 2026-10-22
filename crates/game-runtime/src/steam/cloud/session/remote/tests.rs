//! Steam's listing as a Cloud view reads it: only this account's app, and of
//! it only the files Obelisk manages, each held to Steam's own hash. The
//! game's Cloud record keeps the view, so a restart reads nothing again.
use super::*;
use crate::cloud::sync::tests::Harness as _;
use crate::cloud::view::{Listing as _, Remote as _, Views};
use crate::steam::cloud::session::fake::{SLOT, save_rule};
use crate::steam::cloud::session::tests::{ACCOUNT, SteamGame};
use sha1::{Digest, Sha1};

const NAME: &str = "%GameInstall%slot.sav";

fn file(name: &str, bytes: &[u8], platforms: u32) -> CloudFile {
    CloudFile {
        name: name.into(),
        sha1: Sha1::digest(bytes)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect(),
        size: bytes.len() as u32,
        timestamp: 1,
        persist_state: 0,
        platforms,
        reupload_requested: false,
    }
}

fn listing(files: Vec<CloudFile>) -> CloudListing {
    CloudListing {
        steam_id: ACCOUNT,
        app_id: 42,
        change_number: 7,
        files,
    }
}

/// A connection that only lists.
struct ListingOnly(CloudListing);
impl CloudConnection for ListingOnly {
    async fn listing(&mut self) -> Result<CloudListing, Failure> {
        Ok(self.0.clone())
    }
    async fn download_files(
        &mut self,
        _: &[CloudFile],
        _: impl FnMut(&CloudFile, Vec<u8>) -> Result<(), CloudError>,
    ) -> Result<(), Failure> {
        unreachable!("a listing reads no file")
    }
    async fn cloud_metadata(&mut self) -> Result<Option<CloudMetadata>, Failure> {
        unreachable!("a listing reads no rules")
    }
    async fn preferences(
        &mut self,
        _: &mut CloudPreferencesCache,
    ) -> Result<CloudPreferences, CloudError> {
        unreachable!("a listing reads no preferences")
    }
    async fn settle_upload(&mut self, _: &mut UploadTransaction) -> Result<(), CloudError> {
        unreachable!("a listing settles nothing")
    }
    async fn upload(&mut self, _: &mut UploadTransaction) -> Result<CloudListing, CloudError> {
        unreachable!("a listing uploads nothing")
    }
}

/// The game's listing as its Cloud view reads it.
async fn list(game: &SteamGame, listing: CloudListing) -> Result<SteamListing, Failure> {
    let mut context = Context::new(&game.config, ACCOUNT, save_rule());
    let mut cm = ListingOnly(listing);
    let mut remote = SteamRemote {
        cm: &mut cm,
        context: &mut context,
        config: &game.config,
        steam_id: ACCOUNT,
    };
    remote.list().await
}

#[tokio::test]
async fn a_listing_for_another_account_or_app_is_refused() {
    let game = SteamGame::game();
    let mut foreign = listing(vec![file(NAME, b"save", 2)]);
    foreign.steam_id += 1;
    assert!(matches!(
        list(&game, foreign).await,
        Err(Failure::Authentication)
    ));
    let mut foreign = listing(vec![file(NAME, b"save", 2)]);
    foreign.app_id += 1;
    assert!(matches!(
        list(&game, foreign).await,
        Err(Failure::Authentication)
    ));
}

#[tokio::test]
async fn persist_state_and_foreign_platform_files_are_left_out_of_the_saves() {
    let game = SteamGame::game();
    let mut state = file("%GameInstall%state.sav", b"state", 2);
    state.persist_state = 1;
    let listed = list(
        &game,
        listing(vec![
            file(NAME, b"save", 2),
            state,
            file("%GameInstall%windows.sav", b"windows", 1),
        ]),
    )
    .await
    .unwrap();

    assert_eq!(listed.revision(), "7");
    let saves: Vec<_> = listed
        .saves()
        .into_iter()
        .map(|save| (save.name, save.size, save.platforms))
        .collect();
    assert_eq!(saves, [(NAME, 4, 2)]);
}

#[tokio::test]
async fn a_listed_save_holds_only_the_bytes_its_hash_names() {
    let game = SteamGame::game();
    let mut state = file("%GameInstall%state.sav", b"state", 2);
    state.persist_state = 1;
    let listed = list(&game, listing(vec![file(NAME, b"save", 2), state]))
        .await
        .unwrap();

    assert!(listed.holds(NAME, b"save"));
    assert!(!listed.holds(NAME, b"sav!"));
    assert!(!listed.holds(NAME, b"saves"));
    assert!(!listed.holds("%GameInstall%state.sav", b"state"));
    assert!(!listed.holds("%GameInstall%other.sav", b"save"));
}

#[tokio::test]
async fn unchanged_saves_are_read_once_across_a_restart_through_the_cloud_record() {
    let game = SteamGame::game();
    game.cloud.publish_for(SLOT, b"cloud v1", 7);
    Context::new(&game.config, ACCOUNT, save_rule())
        .save()
        .unwrap();
    let read = async || {
        let (mut adapter, vault) = game.adapter();
        // No baseline: only the record can stand in after a restart.
        assert!(vault.baseline().is_none());
        let mut cm = adapter.connect(&game.control).await.unwrap();
        Views::default()
            .read(
                &mut adapter.remote(&mut cm, &game.control).unwrap(),
                &vault,
                &game.control,
            )
            .await
            .unwrap()
    };

    let first = read().await;
    let again = read().await;

    assert_eq!(game.cloud.downloads(), 1);
    assert_eq!(again, first);
    assert_eq!(again.files.0[SLOT].platforms, 7);
}
