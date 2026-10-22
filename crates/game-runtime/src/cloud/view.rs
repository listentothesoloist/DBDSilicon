//! The Cloud view a Cloud sync compares against, whatever the storefront:
//! archived bytes stand in for listed saves they match, the rest is fetched,
//! the view holds only while the storefront still lists the same Cloud, and
//! an upload ends with the view its storefront confirmed.
use super::{
    Control,
    sync::{self, Failure as _},
};
use cloud_vault::{AccountIdentity, FileVersion, Manifest, RemoteView, Vault};
use std::{
    collections::{BTreeMap, BTreeSet},
    marker::PhantomData,
};

#[cfg(test)]
mod tests;

/// One save a storefront's Cloud lists.
pub(crate) struct Listed<'a> {
    pub name: &'a str,
    pub size: u64,
    pub platforms: u32,
}

/// A complete listing of one game's Cloud, as its storefront gives it.
pub(crate) trait Listing {
    fn revision(&self) -> String;
    /// The saves Obelisk manages, in listing order. The rest is never read
    /// into the view or sent as deleted.
    fn saves(&self) -> Vec<Listed<'_>>;
    /// Whether `bytes` are exactly the listed save `name`, by the storefront's
    /// own identity check.
    fn holds(&self, name: &str, bytes: &[u8]) -> bool;
}

/// A storefront's Cloud for one game, account and attempt: protocol work
/// only. It stops with the sync's `Control`, held itself or by the connection
/// it was built over, so no call takes a cancellation.
#[allow(async_fn_in_trait)]
pub(crate) trait Remote {
    type Failure: sync::Failure;
    type Listing: Listing;
    /// Names the storefront's uploads in the Cloud archive's journal. On
    /// disk: never change it.
    const JOURNAL: &'static str;

    async fn list(&mut self) -> Result<Self::Listing, Self::Failure>;
    /// True while the Cloud still lists exactly `listing`.
    async fn unchanged(&mut self, listing: &Self::Listing) -> Result<bool, Self::Failure>;
    /// Reads these saves of `listing`, each checked against it, and hands each
    /// over once read; a storefront may deliver them together. Runs exactly
    /// once per read, after reuse, even with no names: a storefront whose
    /// commit resends files Obelisk does not manage reads them here.
    async fn fetch(
        &mut self,
        listing: &Self::Listing,
        names: &[&str],
        receive: impl FnMut(&str, Vec<u8>) -> Result<(), cloud_vault::Error>,
    ) -> Result<(), Self::Failure>;
    /// Keeps a verified view and the listing that confirmed it.
    fn adopt(
        &mut self,
        listing: Self::Listing,
        view: &RemoteView,
        vault: &Vault,
    ) -> Result<(), Self::Failure>;
    /// The view the storefront's record kept from an earlier sync.
    fn adopted(&self) -> Option<&Manifest> {
        None
    }
    /// Commits `change`. Only `Change::send` makes a `Committed`.
    async fn commit(
        &mut self,
        change: Change<'_, Self::Failure>,
    ) -> Result<Committed<Self::Listing>, Self::Failure>;
}

/// What an upload sends: local saves over the view they replace.
pub(crate) struct Change<'a, F> {
    vault: &'a mut Vault,
    account: AccountIdentity,
    local: &'a Manifest,
    view: &'a RemoteView,
    journal: &'static str,
    control: &'a Control,
    failure: PhantomData<F>,
}

impl<'a, F: sync::Failure> Change<'a, F> {
    pub(crate) fn vault(&self) -> &Vault {
        self.vault
    }
    pub(crate) fn local(&self) -> &'a Manifest {
        self.local
    }
    pub(crate) fn view(&self) -> &'a RemoteView {
        self.view
    }
    /// Saves that are new or differ from the view, in name order.
    pub(crate) fn uploads(&self) -> impl Iterator<Item = (&'a str, &'a FileVersion)> + use<'a, F> {
        let view = self.view;
        self.local
            .0
            .iter()
            .filter(move |(name, file)| view.files.0.get(*name) != Some(file))
            .map(|(name, file)| (name.as_str(), file))
    }
    /// Names in the view that the local saves lack, in name order.
    pub(crate) fn deletes(&self) -> impl Iterator<Item = &'a str> + use<'a, F> {
        let local = self.local;
        self.view
            .files
            .0
            .keys()
            .filter(move |name| !local.0.contains_key(*name))
            .map(String::as_str)
    }

    /// Sends this change as one upload. `journal` keeps what settling a lost
    /// reply needs, after the vault queued the upload and before it may
    /// commit; `send` returns the complete listing that confirmed it.
    pub(crate) async fn send<L: Listing>(
        self,
        journal: impl FnOnce() -> Result<(), F>,
        send: impl AsyncFnOnce() -> Result<L, F>,
    ) -> Result<Committed<L>, F> {
        let Self {
            vault,
            account,
            local,
            view,
            journal: adapter,
            control,
            ..
        } = self;
        vault
            .upload(account, local, view, adapter, journal, async |_| {
                let listing = send().await?;
                // Every listed save carries the identity the commit gave it,
                // so the managed ones are the local versions just sent or
                // kept. Nothing is downloaded again.
                let mut files = Manifest::default();
                for save in listing.saves() {
                    let version = local.0.get(save.name).ok_or_else(F::changed)?;
                    files.0.insert(save.name.to_owned(), version.clone());
                }
                let confirmed = RemoteView {
                    revision: listing.revision(),
                    files,
                };
                control.check()?;
                Ok((
                    confirmed.clone(),
                    Committed {
                        listing,
                        view: confirmed,
                    },
                ))
            })
            .await
    }
}

/// Sends `local` over `view` and completes the vault's upload with the view
/// its storefront confirmed. It reads nothing, so it needs no `Views`.
pub(crate) async fn send<R: Remote>(
    remote: &mut R,
    vault: &mut Vault,
    account: AccountIdentity,
    local: &Manifest,
    view: &RemoteView,
    control: &Control,
) -> Result<(), R::Failure> {
    let change = Change {
        vault,
        account,
        local,
        view,
        journal: R::JOURNAL,
        control,
        failure: PhantomData,
    };
    let committed = remote.commit(change).await?;
    remote.adopt(committed.listing, &committed.view, vault)
}

/// A listing that confirmed an upload, with its view.
pub(crate) struct Committed<L> {
    listing: L,
    view: RemoteView,
}

/// The Cloud views of one Cloud sync, and the bytes its reads staged.
#[derive(Default)]
pub(crate) struct Views {
    /// Files this operation staged, reused by later reads in it.
    downloaded: Manifest,
}

impl Views {
    /// A verified complete view of the game's Cloud saves.
    pub(crate) async fn read<R: Remote>(
        &mut self,
        remote: &mut R,
        vault: &Vault,
        control: &Control,
    ) -> Result<RemoteView, R::Failure> {
        let listing = remote.list().await?;
        let saves = listing.saves();
        let mut files = Manifest::default();
        let mut missing = BTreeMap::new();
        for save in &saves {
            control.check()?;
            match self.archived(remote.adopted(), vault, &listing, save) {
                Some(version) => {
                    files.0.insert(save.name.to_owned(), version);
                }
                None => {
                    missing.insert(save.name, save.platforms);
                }
            }
        }
        let names: Vec<&str> = saves
            .iter()
            .map(|save| save.name)
            .filter(|name| missing.contains_key(name))
            .collect();
        let downloaded = &mut self.downloaded;
        remote
            .fetch(&listing, &names, |name, bytes| {
                let platforms = missing.remove(name).ok_or(cloud_vault::Error::Integrity)?;
                let version = vault.stage(&bytes, platforms)?;
                downloaded.0.insert(name.to_owned(), version.clone());
                files.0.insert(name.to_owned(), version);
                Ok(())
            })
            .await?;
        // A save the fetch left out would read as deleted in the Cloud.
        if !missing.is_empty() {
            return Err(cloud_vault::Error::Integrity.into());
        }
        // Even an entirely cached view is valid only while the remote listing
        // stays unchanged. No cache may authorize a stale conflict decision.
        if !remote.unchanged(&listing).await? {
            return Err(R::Failure::changed());
        }
        control.check()?;
        self.downloaded = files.clone();
        let view = RemoteView {
            revision: listing.revision(),
            files,
        };
        view.validate()?;
        drop(saves);
        remote.adopt(listing, &view, vault)?;
        Ok(view)
    }

    /// Between operations, before garbage collection: staged bytes stop being
    /// reusable.
    pub(crate) fn forget(&mut self) {
        self.downloaded = Manifest::default();
    }

    fn archived(
        &self,
        adopted: Option<&Manifest>,
        vault: &Vault,
        listing: &impl Listing,
        save: &Listed<'_>,
    ) -> Option<FileVersion> {
        // A storefront's record survives restart. Its listing can have been
        // refreshed before remote bytes, so never trust that pairing: verify
        // the actual blob against the newly listed hash and size on every
        // cache hit.
        let mut checked = BTreeSet::new();
        [
            Some(&self.downloaded),
            adopted,
            vault.baseline().map(|b| &b.files),
            vault.local(),
        ]
        .into_iter()
        .flatten()
        .filter_map(|files| files.0.get(save.name))
        .filter(|cached| cached.size == save.size)
        .filter(|cached| checked.insert((&cached.hash, cached.size)))
        .find_map(|cached| {
            let bytes = vault.read_content(cached).ok()?;
            listing.holds(save.name, &bytes).then(|| FileVersion {
                platforms: save.platforms,
                ..cached.clone()
            })
        })
    }
}
