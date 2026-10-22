use super::*;
use crate::cloud::{
    CloudError, CloudFile, CloudListing, CloudQuota, UploadTransaction, deadline,
    preferences::ROAMING_CONFIG_APP, upload, wire,
};
use futures_util::TryStreamExt;

#[cfg(debug_assertions)]
mod interruption_probe;

const UPLOAD_DEADLINE: u64 = 120;

impl SteamConnection {
    /// Read only the account's remote roaming configuration. Never consult a
    /// local Steam installation or change preferences as an ownership shortcut.
    pub async fn cloud_preferences(
        &self,
        http: &reqwest::Client,
        app_id: u32,
    ) -> Result<crate::cloud::CloudPreferences, CloudError> {
        self.cloud_preferences_cached(http, app_id, &mut Default::default())
            .await
    }

    pub async fn cloud_preferences_cached(
        &self,
        http: &reqwest::Client,
        app_id: u32,
        cache: &mut crate::cloud::CloudPreferencesCache,
    ) -> Result<crate::cloud::CloudPreferences, CloudError> {
        let before = self.cloud_listing(ROAMING_CONFIG_APP).await?;
        let preferences = cache
            .read(app_id, &before, async |file| {
                self.cloud_download(http, ROAMING_CONFIG_APP, file).await
            })
            .await?;
        if self.cloud_listing(ROAMING_CONFIG_APP).await? != before {
            return Err(CloudError::Changed);
        }
        Ok(preferences)
    }
    /// Low-level, journaled transport. The caller must hold the coordinator's
    /// upload permit and exclude a native synchronizer for this account/store.
    /// The guard is rechecked before every request; UI closure is not revocation.
    /// Returns the complete listing that confirmed the batch.
    pub async fn cloud_upload(
        &self,
        http: &reqwest::Client,
        transaction: &mut UploadTransaction,
        authorized: impl Fn() -> bool,
    ) -> Result<CloudListing, CloudError> {
        let mut transport = self.upload_transport(http, transaction, &authorized)?;
        deadline(
            UPLOAD_DEADLINE,
            upload::execute(&mut transport, transaction, &authorized),
        )
        .await
    }

    /// Obtain a positive closure receipt without resending file mutations.
    /// Ambiguous old file requests remain blocked; the caller must read a new
    /// complete remote view after this method returns before reconciliation.
    pub async fn cloud_settle_upload(
        &self,
        http: &reqwest::Client,
        transaction: &mut UploadTransaction,
        authorized: impl Fn() -> bool,
    ) -> Result<(), CloudError> {
        let mut transport = self.upload_transport(http, transaction, &authorized)?;
        deadline(
            UPLOAD_DEADLINE,
            upload::settle(&mut transport, transaction, &authorized),
        )
        .await
    }

    /// Shared admission for journaled upload operations: the session must be
    /// logged on as the transaction's account.
    fn upload_transport<'a, F: Fn() -> bool>(
        &'a self,
        http: &'a reqwest::Client,
        transaction: &UploadTransaction,
        authorized: &'a F,
    ) -> Result<UploadTransport<'a, F>, CloudError> {
        self.ensure_logged_on()?;
        if transaction.steam_id() != self.steam_id {
            return Err(CloudError::Revoked);
        }
        Ok(UploadTransport {
            connection: self,
            http,
            authorized,
        })
    }

    pub async fn cloud_listing(&self, app_id: u32) -> Result<CloudListing, CloudError> {
        if app_id == 0 {
            return Err(CloudError::Invalid("app id"));
        }
        let response: wire::ListingResponse = self
            .service(
                "Cloud.GetAppFileChangelist#1",
                &wire::ListingRequest {
                    app_id,
                    synced_change_number: Some(0),
                },
            )
            .await?;
        crate::cloud::listing(self.steam_id, app_id, response)
    }

    pub async fn cloud_quota(&self, app_id: u32) -> Result<CloudQuota, CloudError> {
        if app_id == 0 {
            return Err(CloudError::Invalid("app id"));
        }
        let response: wire::QuotaResponse = self
            .service(
                "Cloud.ClientGetAppQuotaUsage#1",
                &wire::AppRequest { app_id },
            )
            .await?;
        Ok(CloudQuota {
            existing_files: response.existing_files,
            existing_bytes: response.existing_bytes,
            max_files: response.max_files,
            max_bytes: response.max_bytes,
        })
    }

    pub async fn cloud_download(
        &self,
        http: &reqwest::Client,
        app_id: u32,
        file: &CloudFile,
    ) -> Result<Vec<u8>, CloudError> {
        let response = self.download_request(app_id, file).await?;
        crate::cloud::download(http, app_id, file, response).await
    }

    /// Bounded concurrent reads, CM requests included, on this account's CM
    /// session. The caller owns the listing checks before and after the set.
    pub async fn cloud_download_files(
        &self,
        http: &reqwest::Client,
        app_id: u32,
        files: &[CloudFile],
        authorized: impl Fn() -> bool,
        receive: impl FnMut(&CloudFile, Vec<u8>) -> Result<(), CloudError>,
    ) -> Result<(), CloudError> {
        self.ensure_logged_on()?;
        crate::cloud::downloads::download_files(
            files,
            |file| async move { self.download_request(app_id, &file).await },
            |file, response| {
                let http = http.clone();
                async move { crate::cloud::download(&http, app_id, &file, response).await }
            },
            &authorized,
            receive,
        )
        .await
    }

    /// The CM's answer for one file's body. Persist-state files are never read.
    async fn download_request(
        &self,
        app_id: u32,
        file: &CloudFile,
    ) -> Result<wire::DownloadResponse, CloudError> {
        if file.persist_state != 0 {
            return Err(CloudError::Invalid("persist state"));
        }
        Ok(self
            .service(
                "Cloud.ClientFileDownload#1",
                &wire::DownloadRequest {
                    app_id,
                    name: file.name.clone(),
                },
            )
            .await?)
    }
}

struct UploadTransport<'a, F> {
    connection: &'a SteamConnection,
    http: &'a reqwest::Client,
    authorized: &'a F,
}

impl<F: Fn() -> bool> UploadTransport<'_, F> {
    async fn send_commit(
        &self,
        request: &wire::CommitUploadRequest,
    ) -> Result<wire::CommitUploadResponse, CloudError> {
        if !(self.authorized)() {
            return Err(CloudError::Revoked);
        }
        self.connection.ensure_logged_on()?;
        let packet = self
            .connection
            .request(
                SERVICE_REQUEST,
                request,
                Some("Cloud.ClientCommitFileUpload#1"),
                SERVICE_RESPONSE,
            )
            .await?
            .next()
            .await?;
        commit_response(packet.header.result, &packet.body, request.succeeded)
    }

    async fn send_block(&self, block: &wire::UploadBlock, bytes: &[u8]) -> Result<(), CloudError> {
        let request = crate::cloud::upload::block_request(block, bytes)?;
        if !(self.authorized)() {
            return Err(CloudError::Revoked);
        }
        let response = self
            .http
            .execute(request)
            .await
            .map_err(|_| CloudError::Network)?;
        if !response.status().is_success() {
            return Err(CloudError::Network);
        }
        crate::client::read_bounded(response, 1024 * 1024)
            .await
            .map_err(|_| CloudError::Network)?;
        Ok(())
    }
}

impl<F: Fn() -> bool> crate::cloud::upload::Transport for UploadTransport<'_, F> {
    async fn commit(
        &mut self,
        request: &wire::CommitUploadRequest,
    ) -> Result<wire::CommitUploadResponse, CloudError> {
        self.send_commit(request).await
    }
    async fn commits(
        &mut self,
        requests: &[wire::CommitUploadRequest],
    ) -> Result<Vec<wire::CommitUploadResponse>, CloudError> {
        let this = &*self;
        // Collected first, as in `calls`.
        let commits: Vec<_> = requests
            .iter()
            .map(|request| this.send_commit(request))
            .collect();
        futures_util::stream::iter(commits)
            .buffered(upload::PARALLEL_FILES)
            .try_collect()
            .await
    }
    async fn call<R: Message + Default>(
        &mut self,
        name: &str,
        request: &impl Message,
    ) -> Result<R, CloudError> {
        if !(self.authorized)() {
            return Err(CloudError::Revoked);
        }
        Ok(self.connection.service(name, request).await?)
    }
    async fn calls<R: Message + Default>(
        &mut self,
        name: &str,
        requests: &[impl Message],
    ) -> Result<Vec<R>, CloudError> {
        let (connection, authorized) = (self.connection, self.authorized);
        // Collected first: a stream mapping through a closure leaves the
        // future without a provable Send, which UniFFI's exports need.
        let requests: Vec<_> = requests
            .iter()
            .map(|request| async move {
                if !authorized() {
                    return Err(CloudError::Revoked);
                }
                Ok(connection.service(name, request).await?)
            })
            .collect();
        futures_util::stream::iter(requests)
            .buffered(upload::PARALLEL_FILES)
            .try_collect()
            .await
    }
    async fn block(&mut self, block: &wire::UploadBlock, bytes: &[u8]) -> Result<(), CloudError> {
        self.send_block(block, bytes).await
    }
    async fn bodies(&mut self, files: &[(&[wire::UploadBlock], &[u8])]) -> Result<(), CloudError> {
        let this = &*self;
        // Collected first, as in `calls`.
        let files: Vec<_> = files
            .iter()
            .map(|(blocks, bytes)| async move {
                for block in *blocks {
                    this.send_block(block, bytes).await?;
                }
                Ok::<_, CloudError>(())
            })
            .collect();
        futures_util::stream::iter(files)
            .buffer_unordered(upload::PARALLEL_FILES)
            .try_collect::<Vec<()>>()
            .await?;
        Ok(())
    }
    async fn listing(&mut self, app: u32) -> Result<CloudListing, CloudError> {
        if !(self.authorized)() {
            return Err(CloudError::Revoked);
        }
        self.connection.cloud_listing(app).await
    }
}

fn commit_response(
    result: Option<i32>,
    body: &[u8],
    succeeded: bool,
) -> Result<wire::CommitUploadResponse, CloudError> {
    // Steam reports a failed HTTP transfer with EResult::Fail and an explicit
    // file_committed=false body. That is the requested negative receipt, not
    // upload success. Never apply this exception to a positive commit, an
    // absent/malformed receipt, a missing envelope result or another rejection.
    if !succeeded && result == Some(2) {
        let response =
            wire::CommitUploadResponse::decode(body).map_err(|e| CloudError::Cm(e.into()))?;
        if response.committed == Some(false) {
            return Ok(response);
        }
    }
    check_result(result, "backend.steam.cm.action.contentRequest")?;
    wire::CommitUploadResponse::decode(body).map_err(|e| CloudError::Cm(e.into()))
}

#[cfg(test)]
mod upload_receipt_tests {
    use super::*;

    #[test]
    fn failed_transfer_accepts_only_the_explicit_negative_file_receipt() {
        // Exact nonsensitive body/result observed in the authenticated restart
        // probe: EResult::Fail with file_committed explicitly false.
        let receipt = commit_response(Some(2), &[0x08, 0x00], false).unwrap();
        assert_eq!(receipt.committed, Some(false));
        for (result, body, succeeded) in [
            (Some(2), vec![0x08, 0x00], true),
            (Some(2), vec![0x08, 0x01], false),
            (Some(2), vec![], false),
            (Some(10), vec![0x08, 0x00], false),
            (None, vec![0x08, 0x00], false),
            (Some(2), vec![0x80], false),
        ] {
            assert!(commit_response(result, &body, succeeded).is_err());
        }
    }
}

#[cfg(test)]
mod parallel_tests {
    use super::*;
    use crate::cloud::upload::Transport;

    #[tokio::test]
    async fn a_batch_keeps_ten_file_requests_in_flight_and_answers_in_order() {
        let (connection, mut server) = super::super::test_server::connection(1, true).await;
        let http = reqwest::Client::new();
        let mut transport = UploadTransport {
            connection: &connection,
            http: &http,
            authorized: &|| true,
        };
        let deletes: Vec<_> = (0..12)
            .map(|index| wire::DeleteRequest {
                app_id: 7,
                name: format!("save-{index}"),
                explicit: true,
                batch_id: 1,
            })
            .collect();
        // Each answer echoes its request, so the order shows.
        let answer = async {
            let mut sent = Vec::new();
            for _ in 0..upload::PARALLEL_FILES {
                sent.push(server.request(SERVICE_REQUEST).await);
            }
            assert!(server.quiet().await, "an eleventh request waits");
            for packet in sent.iter().rev() {
                let request = wire::DeleteRequest::decode(packet.body.clone()).unwrap();
                server.reply(packet, SERVICE_RESPONSE, &request).await;
            }
            for _ in upload::PARALLEL_FILES..deletes.len() {
                let packet = server.request(SERVICE_REQUEST).await;
                let request = wire::DeleteRequest::decode(packet.body.clone()).unwrap();
                server.reply(&packet, SERVICE_RESPONSE, &request).await;
            }
        };
        let (answers, ()) = tokio::join!(
            transport.calls::<wire::DeleteRequest>("Cloud.ClientDeleteFile#1", &deletes),
            answer
        );
        let names: Vec<_> = answers.unwrap().into_iter().map(|a| a.name).collect();
        let sent: Vec<_> = deletes.into_iter().map(|d| d.name).collect();
        assert_eq!(names, sent);
    }
}

#[cfg(test)]
mod parallel_commit_tests {
    use super::*;
    use crate::cloud::upload::Transport;

    #[tokio::test]
    async fn commits_keep_ten_in_flight() {
        let (connection, mut server) = super::super::test_server::connection(1, true).await;
        let http = reqwest::Client::new();
        let mut transport = UploadTransport {
            connection: &connection,
            http: &http,
            authorized: &|| true,
        };
        let commits: Vec<_> = (0..11)
            .map(|index| wire::CommitUploadRequest {
                succeeded: true,
                app_id: 7,
                sha: vec![index; 20],
                name: format!("save-{index}"),
            })
            .collect();
        let receipt = wire::CommitUploadResponse {
            committed: Some(true),
        };
        let answer = async {
            let mut sent = Vec::new();
            for _ in 0..upload::PARALLEL_FILES {
                sent.push(server.request(SERVICE_REQUEST).await);
            }
            assert!(server.quiet().await, "an eleventh commit waits");
            for packet in &sent {
                server.reply(packet, SERVICE_RESPONSE, &receipt).await;
            }
            let last = server.request(SERVICE_REQUEST).await;
            server.reply(&last, SERVICE_RESPONSE, &receipt).await;
        };
        let (receipts, ()) = tokio::join!(transport.commits(&commits), answer);
        assert_eq!(receipts.unwrap(), vec![receipt; 11]);
    }
}

#[cfg(test)]
mod download_tests {
    use super::*;

    #[tokio::test]
    async fn single_and_windowed_reads_share_the_persist_state_refusal() {
        let (connection, _server) = super::super::test_server::connection(1, true).await;
        let http = reqwest::Client::new();
        let file = CloudFile {
            name: "state.vdf".into(),
            sha1: "0".repeat(40),
            size: 4,
            timestamp: 1,
            persist_state: 1,
            platforms: 7,
            reupload_requested: false,
        };
        assert!(matches!(
            connection.cloud_download(&http, 7, &file).await,
            Err(CloudError::Invalid("persist state"))
        ));
        assert!(matches!(
            connection
                .cloud_download_files(
                    &http,
                    7,
                    &[file],
                    || true,
                    |_, _| { panic!("a persist-state file is never read") }
                )
                .await,
            Err(CloudError::Invalid("persist state"))
        ));
    }
}
