//! Each game's Cloud record, whatever the storefront: the work a Cloud sync
//! leaves and the removal fence, and what Obelisk does with the record outside
//! a sync: the uninstall archive, the removal, and the pending discovery behind
//! the Cloud indicators. A storefront plugs in its saved record as a `Record`,
//! which its Cloud sync adapter holds during a sync.
use super::sync::{self, Failure as _, Pending, Work};
use crate::{cloud_status, files::safe_storage_path};
use cloud_vault::{AccountIdentity, Bindings, Manifest, StoreScope, Vault};
use serde::de::DeserializeOwned;
use std::{collections::BTreeSet, fs, path::Path};

#[cfg(test)]
pub(crate) mod tests;

/// Where a game is installed and keeps its saves, as its storefront configures it.
pub(crate) trait Configuration: Clone {
    /// The Cloud archive.
    fn root(&self) -> &Path;
    fn install(&self) -> &Path;
    /// Set for a Windows build: its saves live in this Wine prefix.
    fn prefix(&self) -> Option<&Path>;
}

/// A storefront's saved Cloud record of one game: the configuration that holds
/// its saves, its Cloud rules and the work a Cloud sync left.
pub(crate) trait Record: Sized + DeserializeOwned {
    type Failure: sync::Failure;
    type Configuration: Configuration;
    /// Steam's SteamID, Epic's account id.
    type Account: ?Sized;
    /// The record's file in the game's directory of the Cloud archive.
    const FILE: &'static str;

    /// The account as the vault checks it.
    fn identity(account: &Self::Account) -> AccountIdentity;
    /// The game's scope, once `configuration` names only safe paths for `account`.
    fn scope(
        configuration: &Self::Configuration,
        account: &Self::Account,
    ) -> Result<StoreScope, Self::Failure>;
    /// Under the vault's lock. `None` until the game's Cloud rules were known once.
    fn load(
        configuration: &Self::Configuration,
        account: &Self::Account,
    ) -> Result<Option<Self>, Self::Failure>;
    fn save(&self) -> Result<(), Self::Failure>;
    /// A record read without the lock names one account and holds valid manifests.
    fn check(&self) -> Result<(), Self::Failure>;
    fn account(&self) -> &Self::Account;
    fn configuration(&self) -> &Self::Configuration;
    fn configuration_mut(&mut self) -> &mut Self::Configuration;
    fn work(&self) -> Work;
    fn set_work(&mut self, work: Work);
    /// An upload whose outcome is unknown waits to be settled.
    fn uncertain(&self) -> bool {
        false
    }
    /// Where each save lives: the local set, and `remote`'s names. `create`
    /// makes their directories inside an installation that exists. Nothing
    /// is saved: the caller saves the record once it holds what it needs.
    fn bindings(
        &mut self,
        vault: &Vault,
        remote: Option<&Manifest>,
        create: bool,
    ) -> Result<Bindings, Self::Failure>;
}

/// Whether the installation is gone. Anything but a folder there is a storage
/// failure.
pub(crate) fn absent(install: &Path) -> cloud_vault::Result<bool> {
    safe_storage_path(install)?;
    match fs::symlink_metadata(install) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(true),
        Ok(metadata) if metadata.is_dir() => Ok(false),
        Ok(_) => Err(cloud_vault::Error::Invalid),
        Err(error) => Err(error.into()),
    }
}

/// Obelisk holds this game's saves, so its Cloud cannot silently vanish.
/// `recorded` says whether the game has a record.
pub(crate) fn holds_saves(vault: &Vault, recorded: bool) -> bool {
    recorded
        || vault.pending().is_some()
        || vault.game_session().is_some()
        || vault.local().is_some()
        || vault.baseline().is_some()
}

/// Captures the saves `bindings` name. A save whose folder is gone is left
/// out: a capture never recreates a folder. Behind the removal fence, each
/// save missing now keeps its archived version.
pub(crate) fn capture(
    vault: &mut Vault,
    mut bindings: Bindings,
    fenced: bool,
) -> cloud_vault::Result<Manifest> {
    bindings.retain(|_, file| file.path.parent().is_some_and(Path::is_dir));
    if fenced {
        vault.capture_preserving_missing(&bindings)
    } else {
        vault.capture(&bindings)
    }
}

/// Adopts the configuration the game runs with now. A Windows game whose
/// engine moved it to another Wine prefix saves there, still without the
/// saves of the old one. As after a reinstall, the removal fence goes up: a
/// save missing there keeps its archived version and is restored, never sent
/// as deleted.
pub(crate) fn rebind<R: Record>(record: &mut R, configuration: R::Configuration) {
    if record.configuration().prefix() != configuration.prefix() {
        fence(record);
    }
    *record.configuration_mut() = configuration;
}

/// From now on, a save missing locally may be one an uninstall removed.
fn fence<R: Record>(record: &mut R) {
    let mut work = record.work();
    work.installation_removed = true;
    record.set_work(work);
}

/// The game's vault, which holds its storefront's lock, and its record.
pub(crate) fn locked<R: Record>(
    configuration: &R::Configuration,
    account: &R::Account,
) -> Result<(Vault, Option<R>), R::Failure> {
    let scope = R::scope(configuration, account)?;
    let vault = Vault::open(configuration.root(), scope)?;
    let record = R::load(configuration, account)?;
    Ok((vault, record))
}

/// Before an uninstall removes the game's files: captures the saves under
/// the installation's `configuration`, then raises the removal fence. It
/// never creates work: an exit save or an upload still waiting stays as it
/// is. After a partial uninstall, or once the saves moved to another Wine
/// prefix, a save missing now keeps its archived version.
pub(crate) fn archive<R: Record>(
    vault: &mut Vault,
    mut record: R,
    configuration: R::Configuration,
) -> Result<(), R::Failure> {
    let gone = absent(configuration.install())?;
    rebind(&mut record, configuration);
    let fenced = gone || record.work().installation_removed;
    let bindings = record.bindings(vault, None, false)?;
    capture(vault, bindings, fenced)?;
    // Persist before removal starts: recursive deletion can fail partway.
    fence(&mut record);
    record.save()
}

/// Records a completed uninstall: a save missing from now on is never read
/// as deleted. Exit saves and uploads still waiting stay valid.
pub(crate) fn removed<R: Record>(
    configuration: &R::Configuration,
    account: &R::Account,
) -> Result<(), R::Failure> {
    let (_vault, record) = locked::<R>(configuration, account)?;
    let Some(mut record) = record else {
        return Ok(());
    };
    if !absent(configuration.install())? {
        return Err(R::Failure::recovery());
    }
    fence(&mut record);
    record.save()
}

/// The Cloud indicators of `games`, plus every other game of `account` with
/// pending work, such as an uninstalled one. Nothing here takes a lock: a game
/// or maintenance may be writing these journals meanwhile.
pub(crate) fn indicators<R: Record>(
    root: &Path,
    account: &AccountIdentity,
    games: &[StoreScope],
) -> Result<Vec<(StoreScope, cloud_status::Indicator)>, R::Failure> {
    let mut wanted: BTreeSet<StoreScope> = games.iter().cloned().collect();
    wanted.extend(pending::<R>(root, account)?);
    let mut indicators = Vec::new();
    for scope in wanted {
        match indicator::<R>(root, &scope) {
            Ok(indicator) => indicators.push((scope, indicator)),
            Err(error) => eprintln!("Cloud indicator of {scope:?} unavailable: {}", error.key()),
        }
    }
    Ok(indicators)
}

fn indicator<R: Record>(
    root: &Path,
    scope: &StoreScope,
) -> Result<cloud_status::Indicator, R::Failure> {
    let vault = cloud_vault::peek(root, scope.clone())?;
    // No record: the game has no Cloud rules or was never checked.
    let record = saved::<R>(root, scope)?;
    let journal = journal(
        record.as_ref().map(R::work).as_ref(),
        record.as_ref().is_some_and(R::uncertain),
    );
    let status = cloud_status::read(root, scope)?;
    Ok(cloud_status::indicator(scope, &vault, &journal, &status))
}

/// What Cloud maintenance of an uninstalled game syncs: the record it left.
pub(crate) fn saved<R: Record>(root: &Path, scope: &StoreScope) -> Result<Option<R>, R::Failure> {
    safe_storage_path(root)?;
    read(root, &scope.directory(root)?)
}

/// The games of `account` whose Cloud work still waits. One unreadable record
/// never hides the others' work.
fn pending<R: Record>(
    root: &Path,
    account: &AccountIdentity,
) -> Result<Vec<StoreScope>, R::Failure> {
    safe_storage_path(root)?;
    let directory = account.directory(root)?;
    safe_storage_path(&directory)?;
    if !directory.exists() {
        return Ok(Vec::new());
    }
    let mut games = Vec::new();
    for entry in fs::read_dir(directory).map_err(cloud_vault::Error::from)? {
        let entry = entry.map_err(cloud_vault::Error::from)?;
        if !entry
            .file_type()
            .map_err(cloud_vault::Error::from)?
            .is_dir()
        {
            continue;
        }
        match waiting::<R>(root, &entry.path()) {
            Ok(Some(scope)) => games.push(scope),
            Ok(None) => {}
            Err(error) => eprintln!(
                "Cloud record of {} unreadable: {}",
                entry.file_name().to_string_lossy(),
                error.key()
            ),
        }
    }
    Ok(games)
}

/// The game whose record lives in `directory`, if its work still waits.
fn waiting<R: Record>(root: &Path, directory: &Path) -> Result<Option<StoreScope>, R::Failure> {
    let Some(record) = read::<R>(root, directory)? else {
        return Ok(None);
    };
    let scope = R::scope(record.configuration(), record.account())?;
    // Vault work outlives the record's: an uncertain upload or an
    // interrupted replacement still needs this game's next sync.
    let vault = cloud_vault::peek(root, scope.clone())?;
    Ok(
        (record.work().pending.is_some() || vault.pending.is_some() || vault.applying)
            .then_some(scope),
    )
}

/// A record read without the vault's lock, from the game directory it must
/// name: one copied from another account, game or archive is refused.
fn read<R: Record>(root: &Path, directory: &Path) -> Result<Option<R>, R::Failure> {
    let path = directory.join(R::FILE);
    safe_storage_path(&path)?;
    let Some(record) = cloud_vault::journal::read::<R>(&path)? else {
        return Ok(None);
    };
    record.check()?;
    if record.configuration().root() != root
        || R::scope(record.configuration(), record.account())?.directory(root)? != directory
    {
        return Err(cloud_vault::Error::Invalid.into());
    }
    Ok(Some(record))
}

/// What a game's Cloud indicator reads from its record's work.
fn journal(work: Option<&Work>, uncertain: bool) -> cloud_status::Journal {
    let pending = work.and_then(|work| work.pending.as_ref());
    cloud_status::Journal {
        pending: matches!(pending, Some(Pending::Capture | Pending::Upload { .. })),
        uncertain,
        applying: matches!(pending, Some(Pending::Applying { .. })),
        installation_removed: work.is_some_and(|work| work.installation_removed),
    }
}
