use super::super::paths::Paths;
use super::*;
use crate::cloud::view::{self, Listed};
use cloud_vault::RemoteView;
use std::collections::BTreeSet;
use steam_kit::cloud::{CloudError, CloudFile, CloudMetadata, CloudPreferences, CloudUpload};

/// One authenticated Steam Cloud connection, bound to one app.
pub(crate) trait CloudConnection {
    async fn listing(&mut self) -> Result<CloudListing, Failure>;
    /// Reads `files` in one bounded window, each verified against its listed
    /// hash before `receive` gets it.
    async fn download_files(
        &mut self,
        files: &[CloudFile],
        receive: impl FnMut(&CloudFile, Vec<u8>) -> Result<(), CloudError>,
    ) -> Result<(), Failure>;
    /// The app's Cloud rules, or `None` when the game has no Steam Cloud.
    async fn cloud_metadata(&mut self) -> Result<Option<CloudMetadata>, Failure>;
    async fn preferences(
        &mut self,
        cache: &mut CloudPreferencesCache,
    ) -> Result<CloudPreferences, CloudError>;
    async fn settle_upload(
        &mut self,
        transaction: &mut UploadTransaction,
    ) -> Result<(), CloudError>;
    /// Commits the transaction and returns the complete listing that
    /// confirmed it.
    async fn upload(
        &mut self,
        transaction: &mut UploadTransaction,
    ) -> Result<CloudListing, CloudError>;
}

/// Where a session's Cloud connections come from: Steam's CM in production,
/// an in-memory store in tests.
pub(crate) trait CloudRemote {
    type Connection: CloudConnection;
    async fn connect(
        &self,
        credentials: &Credentials,
        app_id: u32,
        control: Arc<Control>,
    ) -> Result<Self::Connection, Failure>;
}

pub(crate) struct SteamCm {
    http: reqwest::Client,
}

impl SteamCm {
    pub(crate) fn new() -> Result<Self, Failure> {
        Ok(Self {
            http: steam_kit::client::http_client().map_err(|_| Failure::Network)?,
        })
    }
}

impl CloudRemote for SteamCm {
    type Connection = CmConnection;
    async fn connect(
        &self,
        credentials: &Credentials,
        app_id: u32,
        control: Arc<Control>,
    ) -> Result<CmConnection, Failure> {
        let mut cm = SteamConnection::connect(self.http.clone()).await?;
        cm.log_on(
            &credentials.account_name,
            credentials.steam_id,
            &credentials.refresh_token,
            steam_kit::cm::ChatMode::Disabled,
        )
        .await?;
        Ok(CmConnection {
            cm,
            http: self.http.clone(),
            app_id,
            control,
        })
    }
}

pub(crate) struct CmConnection {
    cm: SteamConnection,
    http: reqwest::Client,
    app_id: u32,
    control: Arc<Control>,
}

impl CmConnection {
    /// steam-kit's guard, checked before each Cloud request: a cancelled or
    /// revoked sync sends nothing more. Its journaled uploads take this guard
    /// rather than a token, so every call here stops the same way.
    fn authorized(&self) -> impl Fn() -> bool + '_ {
        let control = &self.control;
        move || control.check().is_ok()
    }
}

impl CloudConnection for CmConnection {
    async fn download_files(
        &mut self,
        files: &[CloudFile],
        receive: impl FnMut(&CloudFile, Vec<u8>) -> Result<(), CloudError>,
    ) -> Result<(), Failure> {
        Ok(self
            .cm
            .cloud_download_files(&self.http, self.app_id, files, self.authorized(), receive)
            .await?)
    }
    async fn listing(&mut self) -> Result<CloudListing, Failure> {
        Ok(self.cm.cloud_listing(self.app_id).await?)
    }
    async fn cloud_metadata(&mut self) -> Result<Option<CloudMetadata>, Failure> {
        Ok(self.cm.app_info(self.app_id).await?.cloud)
    }
    async fn preferences(
        &mut self,
        cache: &mut CloudPreferencesCache,
    ) -> Result<CloudPreferences, CloudError> {
        self.cm
            .cloud_preferences_cached(&self.http, self.app_id, cache)
            .await
    }
    async fn settle_upload(
        &mut self,
        transaction: &mut UploadTransaction,
    ) -> Result<(), CloudError> {
        self.cm
            .cloud_settle_upload(&self.http, transaction, self.authorized())
            .await
    }
    async fn upload(
        &mut self,
        transaction: &mut UploadTransaction,
    ) -> Result<CloudListing, CloudError> {
        self.cm
            .cloud_upload(&self.http, transaction, self.authorized())
            .await
    }
}

/// A Steam Cloud listing, and which of its files Obelisk manages.
pub(crate) struct SteamListing {
    listing: CloudListing,
    /// Indices of the files Obelisk manages.
    managed: Vec<usize>,
}

impl SteamListing {
    /// `paths.existing_file` decides which files Obelisk manages; it also
    /// leaves out persist-state files.
    pub(super) fn managed(listing: CloudListing, paths: &Paths) -> Result<Self, Failure> {
        let mut managed = Vec::new();
        for (index, file) in listing.files.iter().enumerate() {
            if paths.existing_file(file)?.is_some() {
                managed.push(index);
            }
        }
        Ok(Self { listing, managed })
    }

    fn managed_files(&self) -> impl Iterator<Item = &CloudFile> {
        self.managed.iter().map(|&index| &self.listing.files[index])
    }
}

impl view::Listing for SteamListing {
    fn revision(&self) -> String {
        self.listing.change_number.to_string()
    }
    fn saves(&self) -> Vec<Listed<'_>> {
        self.managed_files()
            .map(|file| Listed {
                name: &file.name,
                size: u64::from(file.size),
                platforms: file.platforms,
            })
            .collect()
    }
    fn holds(&self, name: &str, bytes: &[u8]) -> bool {
        self.managed_files()
            .find(|file| file.name == name)
            .is_some_and(|file| file.verify(bytes).is_ok())
    }
}

/// Steam Cloud for one app and account, over one CM connection: the game's
/// Cloud record keeps the listing and view it adopts.
pub(crate) struct SteamRemote<'a, C> {
    pub(super) cm: &'a mut C,
    pub(super) context: &'a mut Context,
    pub(super) config: &'a Config,
    pub(super) steam_id: u64,
}

impl<C: CloudConnection> view::Remote for SteamRemote<'_, C> {
    type Failure = Failure;
    type Listing = SteamListing;
    const JOURNAL: &'static str = "steam-cm-autocloud";

    async fn list(&mut self) -> Result<SteamListing, Failure> {
        let listing = self.cm.listing().await?;
        if listing.steam_id != self.steam_id || listing.app_id != self.config.app_id {
            return Err(Failure::Authentication);
        }
        SteamListing::managed(listing, &self.context.paths()?)
    }
    async fn unchanged(&mut self, listing: &SteamListing) -> Result<bool, Failure> {
        Ok(self.cm.listing().await? == listing.listing)
    }
    async fn fetch(
        &mut self,
        listing: &SteamListing,
        names: &[&str],
        mut receive: impl FnMut(&str, Vec<u8>) -> Result<(), cloud_vault::Error>,
    ) -> Result<(), Failure> {
        let names: BTreeSet<&str> = names.iter().copied().collect();
        let files: Vec<_> = listing
            .managed_files()
            .filter(|file| names.contains(file.name.as_str()))
            .cloned()
            .collect();
        self.cm
            .download_files(&files, |file, bytes| {
                receive(&file.name, bytes).map_err(|_| CloudError::Storage)
            })
            .await
    }
    fn adopt(
        &mut self,
        listing: SteamListing,
        view: &RemoteView,
        vault: &Vault,
    ) -> Result<(), Failure> {
        self.context.listing = listing.listing;
        self.context.remote = view.clone();
        self.context.refresh_bindings(vault.local())?;
        self.context.save()
    }
    fn adopted(&self) -> Option<&Manifest> {
        Some(&self.context.remote.files)
    }
    async fn commit(
        &mut self,
        change: view::Change<'_, Failure>,
    ) -> Result<view::Committed<SteamListing>, Failure> {
        let mut uploads = Vec::new();
        for (name, file) in change.uploads() {
            uploads.push(CloudUpload {
                name: name.to_owned(),
                bytes: change.vault().read_content(file)?,
                timestamp: crate::unix_now(),
                platforms: file.platforms,
            });
        }
        let deletes = change.deletes().map(str::to_owned).collect();
        // Full listing preserves foreign-platform entries. Only the managed
        // Mac subset supplies upload/deletion operations and the Vault baseline.
        let mut transaction = UploadTransaction::create(
            &store::directory(self.config, self.steam_id)?,
            self.context.listing.clone(),
            uploads,
            deletes,
        )?;
        let paths = self.context.paths()?;
        let directory = transaction.directory().to_path_buf();
        let (context, cm) = (&mut *self.context, &mut *self.cm);
        let committed = change
            .send(
                || {
                    // Settling a lost receipt later needs this transaction.
                    context.transaction = Some(directory);
                    context.save()
                },
                async || SteamListing::managed(cm.upload(&mut transaction).await?, &paths),
            )
            .await?;
        self.context.transaction = None;
        Ok(committed)
    }
}

#[cfg(test)]
mod tests;
