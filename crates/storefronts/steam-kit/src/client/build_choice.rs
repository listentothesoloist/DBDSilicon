//! Steam's answers to the build choice: the builds the account's licenses
//! grant, and a Mac build's depot manifests, read to find the program and
//! Steam API library a launch would start.
use std::{collections::HashMap, sync::Arc};

use obelisk_core::{
    build_choice::{GameBuilds, Inspection, Offered},
    mac_build,
    platform::Platform,
};

use super::{
    Client, ClientError, DepotAuthorization, MacBuild, installable_builds, select_depots,
    shared_sources,
};
use crate::{
    cm::{AppInfo, ContentLicenses},
    content::{CachedManifest, CancelToken, ContentClient, DepotFiles, DepotSource},
};

/// The account's licenses and a game's PICS metadata, from which both of
/// its builds are selected.
#[derive(Clone)]
pub(super) struct Licensed {
    pub(super) owned: Arc<ContentLicenses>,
    pub(super) app: Arc<AppInfo>,
}

impl Licensed {
    /// The builds offered, of the Mac build and the Windows build, each
    /// when the account's licenses select depots of it.
    pub(super) fn offered(self, mac: bool, windows: bool) -> Offered<Licensed> {
        Offered {
            mac: mac.then(|| self.clone()),
            windows: windows.then_some(self),
        }
    }
}

/// A game the account owns, with the depots of `language`.
pub(super) struct OwnedApp<'a> {
    pub(super) client: &'a Client,
    pub(super) app_id: u32,
    pub(super) language: &'a str,
}

impl GameBuilds for OwnedApp<'_> {
    type Build = Licensed;
    type Key = MacBuild;
    type Kept = HashMap<(u32, u64), CachedManifest>;
    type Error = ClientError;

    async fn builds(&self) -> Result<Offered<Licensed>, ClientError> {
        let (owned, app) = self.client.owned_app(self.app_id).await?;
        let (mac, windows) = installable_builds(&app, &owned, self.language);
        Ok(Licensed { owned, app }.offered(mac, windows))
    }

    /// A build without a Mac launch configuration has no program to read,
    /// so it keeps no verdict.
    async fn key(&self, mac: &Licensed) -> Result<Option<MacBuild>, ClientError> {
        if crate::launch::configuration(&mac.app, Platform::Mac).is_none() {
            return Ok(None);
        }
        self.client
            .mac_build(&mac.app, &mac.owned, self.language)
            .await
            .map(Some)
    }

    /// Whether the build's program and its Steam API library hold arm64
    /// slices, as a launch would start them. It reads their first bytes from
    /// the build's depots.
    async fn inspect(
        &self,
        mac: &Licensed,
        cancel: &CancelToken,
    ) -> Result<Inspection<Self::Kept>, ClientError> {
        let Some(executable) = crate::launch::configuration(&mac.app, Platform::Mac)
            .map(|config| config.executable.clone())
        else {
            return Ok(Inspection::NotNative);
        };
        let (client, app) = (self.client, &*mac.app);
        let selected = select_depots(app, &mac.owned, Platform::Mac, self.language);
        let sources = client
            .cached_app_infos(&shared_sources(app, &selected))
            .await?;
        let servers = client.content_servers(app.app_id).await?;
        let mut depots = Vec::new();
        for depot in &selected {
            if let Some(prepared) = client.prepare_depot(app, depot, &sources, cancel).await? {
                let request_code = client
                    .cm
                    .manifest_request_code(
                        prepared.depot_id,
                        prepared.authorizing_app,
                        prepared.manifest_id,
                    )
                    .await?;
                depots.push((prepared, request_code));
            }
        }
        let authorizations: Vec<_> = depots
            .iter()
            .map(|(depot, _)| DepotAuthorization {
                cm: &client.cm,
                app_id: depot.authorizing_app,
                depot_id: depot.depot_id,
            })
            .collect();
        let content = ContentClient::new(client.http.clone());
        let mut files = Vec::new();
        let mut listing = mac_build::Listing::default();
        for ((depot, request_code), authorization) in depots.iter().zip(&authorizations) {
            let source = DepotSource {
                depot_id: depot.depot_id,
                manifest_id: depot.manifest_id,
                request_code: *request_code,
                depot_key: depot.key,
                servers: &servers,
                authorization: Some(authorization),
            };
            let depot_files = content.depot_files(source, cancel).await?;
            depot_files.list(&mut listing, files.len());
            files.push(depot_files);
        }
        let read = |(depot, file): (usize, usize), bytes: u64| {
            let files: &Vec<_> = &files;
            files[depot].read(file, bytes, cancel)
        };
        let native =
            mac_build::runs_natively(&listing, &executable, &["libsteam_api.dylib"], read).await?;
        Ok(match native {
            true => Inspection::Native(files.into_iter().map(DepotFiles::into_manifest).collect()),
            false => Inspection::NotNative,
        })
    }
}
