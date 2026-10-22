//! Each game's Cloud record through its interface, once for every storefront,
//! on the Cloud sync's harnesses: the uninstall archive, the removal, pending
//! discovery and the indicators.
use super::*;
use crate::cloud::sync::tests::{
    Harness, Of, RecordOf, indicator, listed, pending, played_offline, synced, uninstall,
    uploading, vault, work,
};
use crate::cloud_status::SavedState;
use crate::supervisor::client::no_other_session;
use cloud_vault::{EpicScope, Scope};

/// Beside `scope`: another game of the same account, the same game of another
/// account, and a game of the other storefront.
fn neighbours(scope: &StoreScope) -> [StoreScope; 3] {
    let steam = |steam_id, app_id| {
        StoreScope::from(Scope {
            steam_id,
            app_id,
            cloud_app_id: app_id,
        })
    };
    match scope {
        StoreScope::Steam(scope) => [
            steam(scope.steam_id, 7),
            steam(scope.steam_id + 1, scope.app_id),
            EpicScope {
                account_id: "0123456789abcdef0123456789abcdef".into(),
                namespace: "namespace".into(),
                app_name: "game".into(),
            }
            .into(),
        ],
        StoreScope::Epic(scope) => [
            EpicScope {
                app_name: "other-game".into(),
                ..scope.clone()
            }
            .into(),
            EpicScope {
                account_id: "abcdef0123456789abcdef0123456789".into(),
                ..scope.clone()
            }
            .into(),
            steam(0x0110_0001_0000_0001, 10),
        ],
    }
}

/// The account `scope` belongs to.
fn account(scope: &StoreScope) -> AccountIdentity {
    match scope {
        StoreScope::Steam(Scope { steam_id, .. }) => (*steam_id).into(),
        StoreScope::Epic(scope) => scope.account_id.as_str().into(),
    }
}

/// The games of `account` whose Cloud indicator shows: those `requested`,
/// and those pending discovery finds.
fn shown<H: Harness>(
    game: &H,
    account: &AccountIdentity,
    requested: &[StoreScope],
) -> Vec<StoreScope> {
    indicators::<RecordOf<H>>(&game.root(), account, requested)
        .unwrap()
        .into_iter()
        .map(|(scope, _)| scope)
        .collect()
}

/// The removal of `game`'s installation, recorded.
fn record_removal<H: Harness>(game: &H) -> Result<(), Of<H>> {
    removed::<RecordOf<H>>(&game.configuration(), game.account_id())
}

pub(crate) async fn pending_discovery_includes_uploads_the_vault_still_holds<H: Harness>() {
    let game = H::game();
    synced(&game, b"old").await;
    let mut vault = vault(&game);
    vault.set_preferences(cloud_vault::Preferences {
        account_enabled: Some(true),
        game_enabled: Some(true),
    });
    let files = Manifest([(game.slot().into(), vault.stage(b"exit", 2).unwrap())].into());
    let authorization = vault.authorize(game.account()).unwrap();
    let before = vault.baseline().unwrap().clone();
    vault
        .queue_upload(&files, &before, "fixture", authorization)
        .unwrap();
    drop(vault);

    assert_eq!(pending(&game), None);
    assert!(listed(&game));
}

pub(crate) async fn indicators_follow_installed_and_uninstalled_games_without_locking<
    H: Harness,
>() {
    let game = H::game();
    assert_eq!(
        indicator(&game, true).unwrap().state,
        SavedState::NeverSynced
    );
    synced(&game, b"old").await;
    let shown = indicator(&game, true).unwrap();
    assert_eq!(shown.state, SavedState::Synced);
    assert!(shown.synced_at.is_some());

    played_offline(&game, b"played offline").await;
    let held = game.open();
    let shown = indicator(&game, true).unwrap();
    assert_eq!(shown.state, SavedState::Pending);
    assert!(shown.pending_upload);
    drop(held);

    uninstall(&game).await;
    let shown = indicator(&game, false).expect("uninstalled games with pending work are listed");
    assert!(shown.installation_removed);
    assert!(shown.pending_upload);
}

pub(crate) async fn a_windows_game_whose_engine_moved_its_prefix_keeps_its_cloud_saves<
    H: Harness,
>() {
    let mut game = H::game();
    let rosetta = game.root().with_file_name("Prefixes/game");
    game.run_in(&rosetta);
    synced(&game, b"progress").await;
    let (old, uploads) = (game.save(), game.uploads());

    // The Linux engine runs the game in a prefix of its own, still empty.
    let linux = game.root().with_file_name("Prefixes/game-linux/prefix");
    game.run_in(&linux);
    let mut sync = game.open();
    sync.prepare(&no_other_session).await.unwrap();

    assert_eq!(fs::read(game.save()).unwrap(), b"progress");
    sync.started().unwrap();
    sync.finished(true).await;
    assert_eq!(game.cloud().as_deref(), Some(&b"progress"[..]));
    assert_eq!(game.uploads(), uploads);
    assert_eq!(pending(&game), None);
    assert_eq!(fs::read(old).unwrap(), b"progress");
}

pub(crate) async fn an_uninstall_after_the_engine_moved_the_prefix_keeps_the_archived_saves<
    H: Harness,
>() {
    let mut game = H::game();
    let rosetta = game.root().with_file_name("Prefixes/game");
    game.run_in(&rosetta);
    synced(&game, b"progress").await;
    // The Linux engine readied a prefix of its own, still empty, and the
    // player uninstalls the game before it ever runs there.
    let linux = game.root().with_file_name("Prefixes/game-linux/prefix");
    game.run_in(&linux);

    uninstall(&game).await;

    let vault = vault(&game);
    let archived = vault.local().unwrap();
    assert_eq!(
        vault.read_content(&archived.0[game.slot()]).unwrap(),
        b"progress"
    );
    let work = work(&game).unwrap();
    assert!(work.installation_removed);
    assert_eq!(work.pending, None);
}

pub(crate) async fn an_uninstall_archives_the_saves_of_a_game_never_synced<H: Harness>() {
    let game = H::game();
    fs::write(game.save(), b"progress").unwrap();
    assert_eq!(work(&game), None);

    uninstall(&game).await;

    let vault = vault(&game);
    let archived = vault.local().unwrap();
    assert_eq!(
        vault.read_content(&archived.0[game.slot()]).unwrap(),
        b"progress"
    );
    assert!(vault.pending().is_none());
    let work = work(&game).unwrap();
    assert!(work.installation_removed);
    assert_eq!(work.pending, None, "an uninstall creates no work");
    assert!(!listed(&game));
    assert!(!game.install().exists());
}

pub(crate) async fn an_uninstall_keeps_the_exit_save_and_the_upload_still_waiting<H: Harness>() {
    let game = H::game();
    synced(&game, b"old").await;
    played_offline(&game, b"exit save").await;
    let exit = uploading(pending(&game)).unwrap();
    // The exit save's upload began, and its outcome is unknown.
    let mut held = vault(&game);
    held.set_preferences(cloud_vault::Preferences {
        account_enabled: Some(true),
        game_enabled: Some(true),
    });
    let before = held.baseline().unwrap().clone();
    let authorization = held.authorize(game.account()).unwrap();
    let upload = held
        .queue_upload(&exit, &before, "fixture", authorization)
        .unwrap();
    held.begin_upload(&upload, authorization, &before).unwrap();
    drop(held);
    fs::write(game.save(), b"edited before uninstall").unwrap();

    game.archive().await.unwrap();
    game.archive().await.unwrap();
    fs::remove_dir_all(game.install()).unwrap();
    record_removal(&game).unwrap();
    record_removal(&game).unwrap();

    let work = work(&game).unwrap();
    assert!(work.installation_removed);
    assert_eq!(uploading(work.pending), Some(exit.clone()));
    let vault = vault(&game);
    let waiting = vault.pending().unwrap();
    assert_eq!(waiting.id, upload);
    assert_eq!(waiting.desired, exit);
    assert!(waiting.may_have_committed);
    let archived = vault.local().unwrap();
    assert_eq!(
        vault.read_content(&archived.0[game.slot()]).unwrap(),
        b"edited before uninstall"
    );
    assert_eq!(
        vault.read_content(&exit.0[game.slot()]).unwrap(),
        b"exit save"
    );
    assert!(listed(&game));
}

pub(crate) async fn an_interrupted_uninstall_keeps_the_saves_it_already_removed<H: Harness>() {
    let game = H::game();
    let other = game.save().with_file_name("other.sav");
    let other_slot = game.slot().replace("slot.sav", "other.sav");
    fs::write(game.save(), b"original slot").unwrap();
    fs::write(&other, b"original other").unwrap();
    game.archive().await.unwrap();
    // The removal deleted one save, then failed with the installation and
    // another save, edited meanwhile, still there.
    fs::remove_file(game.save()).unwrap();
    fs::write(&other, b"edited survivor").unwrap();

    let refused = record_removal(&game).unwrap_err();
    game.archive().await.unwrap();

    assert_eq!(refused.key(), Of::<H>::recovery().key());
    let work = work(&game).unwrap();
    assert!(work.installation_removed);
    assert_eq!(work.pending, None);
    assert!(!listed(&game));
    let vault = vault(&game);
    assert!(vault.pending().is_none());
    let archived = vault.local().unwrap();
    assert_eq!(archived.0.len(), 2);
    assert_eq!(
        vault.read_content(&archived.0[game.slot()]).unwrap(),
        b"original slot"
    );
    assert_eq!(
        vault
            .read_content(&archived.0[other_slot.as_str()])
            .unwrap(),
        b"edited survivor"
    );
    assert!(!game.save().exists());
    assert_eq!(fs::read(&other).unwrap(), b"edited survivor");
}

pub(crate) async fn a_failed_uninstall_archive_leaves_the_record_as_it_was<H: Harness>() {
    let mut game = H::game();
    let rosetta = game.root().with_file_name("Prefixes/game");
    game.run_in(&rosetta);
    synced(&game, b"progress").await;
    let record = game
        .scope()
        .directory(&game.root())
        .unwrap()
        .join(RecordOf::<H>::FILE);
    let before = fs::read(&record).unwrap();
    // A Game session that crashed still holds the saves, and the engine has
    // moved the game to a prefix of its own since.
    vault(&game)
        .game_started(&uuid::Uuid::new_v4().to_string())
        .unwrap();
    let linux = game.root().with_file_name("Prefixes/game-linux/prefix");
    game.run_in(&linux);

    let refused = game.archive().await.unwrap_err();

    assert_eq!(refused.key(), Of::<H>::from(cloud_vault::Error::Busy).key());
    assert_eq!(fs::read(&record).unwrap(), before);
}

pub(crate) async fn an_uninstall_needs_only_its_storefronts_lock<H: Harness>() {
    let game = H::game();
    synced(&game, b"progress").await;
    let [same_storefront, _, other_storefront] = neighbours(&game.scope());
    let held = Vault::open(&game.root(), other_storefront).unwrap();

    game.archive().await.unwrap();
    fs::remove_dir_all(game.install()).unwrap();
    record_removal(&game).unwrap();

    assert!(work(&game).unwrap().installation_removed);
    let busy = Of::<H>::from(cloud_vault::Error::Busy).key();
    let other_game = Vault::open(&game.root(), same_storefront).unwrap();
    assert_eq!(game.archive().await.unwrap_err().key(), busy);
    assert_eq!(record_removal(&game).unwrap_err().key(), busy);
    drop((held, other_game));
}

pub(crate) async fn an_unreadable_record_neither_hides_nor_fakes_another_games_pending_work<
    H: Harness,
>() {
    let game = H::game();
    synced(&game, b"old").await;
    played_offline(&game, b"exit save").await;
    let [other_game, _, _] = neighbours(&game.scope());
    let directory = other_game.directory(&game.root()).unwrap();
    fs::create_dir_all(&directory).unwrap();
    let broken = directory.join(RecordOf::<H>::FILE);
    fs::write(&broken, b"not valid JSON").unwrap();
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&broken, fs::Permissions::from_mode(0o600)).unwrap();
    }

    let discovered = shown(&game, &game.account(), &[]);

    assert_eq!(discovered, [game.scope()]);
    assert_eq!(
        shown(&game, &game.account(), &[other_game]),
        [game.scope()],
        "an unreadable record shows no indicator"
    );
}

pub(crate) async fn a_record_copied_into_another_account_or_game_is_never_read_as_its_own<
    H: Harness,
>() {
    let game = H::game();
    synced(&game, b"old").await;
    played_offline(&game, b"exit save").await;
    let own = game
        .scope()
        .directory(&game.root())
        .unwrap()
        .join(RecordOf::<H>::FILE);
    let [other_game, other_account, _] = neighbours(&game.scope());

    for copy in [&other_game, &other_account] {
        let directory = copy.directory(&game.root()).unwrap();
        fs::create_dir_all(&directory).unwrap();
        fs::copy(&own, directory.join(RecordOf::<H>::FILE)).unwrap();
        assert!(saved::<RecordOf<H>>(&game.root(), copy).is_err());
    }

    assert_eq!(shown(&game, &game.account(), &[]), [game.scope()]);
    assert!(shown(&game, &account(&other_account), &[]).is_empty());
}

/// Instantiates every Cloud record scenario for one storefront's harness.
macro_rules! record_suite {
    ($harness:ty) => {
        $crate::cloud::record::tests::record_suite!(
            $harness;
            pending_discovery_includes_uploads_the_vault_still_holds,
            indicators_follow_installed_and_uninstalled_games_without_locking,
            a_windows_game_whose_engine_moved_its_prefix_keeps_its_cloud_saves,
            an_uninstall_after_the_engine_moved_the_prefix_keeps_the_archived_saves,
            an_uninstall_archives_the_saves_of_a_game_never_synced,
            an_uninstall_keeps_the_exit_save_and_the_upload_still_waiting,
            an_interrupted_uninstall_keeps_the_saves_it_already_removed,
            a_failed_uninstall_archive_leaves_the_record_as_it_was,
            an_uninstall_needs_only_its_storefronts_lock,
            an_unreadable_record_neither_hides_nor_fakes_another_games_pending_work,
            a_record_copied_into_another_account_or_game_is_never_read_as_its_own,
        );
    };
    ($harness:ty; $($scenario:ident),* $(,)?) => {
        $(
            #[tokio::test]
            async fn $scenario() {
                $crate::cloud::record::tests::$scenario::<$harness>().await
            }
        )*
    };
}
pub(crate) use record_suite;
