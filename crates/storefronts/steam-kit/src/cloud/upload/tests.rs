use super::*;

#[test]
fn upload_platform_wire_defaults_do_not_turn_none_into_all() {
    // Tag 7 is absent only for Valve's schema default (all platforms).
    let none = wire::BeginUploadRequest {
        platforms: 0,
        ..Default::default()
    }
    .encode_to_vec();
    assert_eq!(none, [0x38, 0x00]);
    assert_eq!(
        wire::BeginUploadRequest::decode(none.as_slice())
            .unwrap()
            .platforms,
        0
    );
    assert_eq!(
        wire::BeginUploadRequest::decode(&[][..]).unwrap().platforms,
        u32::MAX
    );
}

fn file(name: &str, bytes: &[u8]) -> CloudFile {
    CloudFile {
        name: name.into(),
        sha1: crate::content::hex(&Sha1::digest(bytes)),
        size: bytes.len() as u32,
        timestamp: 1,
        persist_state: 0,
        platforms: 7,
        reupload_requested: false,
    }
}
fn listing(files: Vec<CloudFile>) -> CloudListing {
    CloudListing {
        steam_id: 0x0110000100000001,
        app_id: 42,
        change_number: 2,
        files,
    }
}
fn fixture() -> (tempfile::TempDir, UploadTransaction, Fake) {
    let root = tempfile::tempdir().unwrap();
    let parent = root.path().canonicalize().unwrap();
    let before = listing(vec![file("keep", b"same"), file("delete", b"old")]);
    let job = UploadTransaction::create(
        &parent,
        before.clone(),
        vec![CloudUpload {
            name: "new".into(),
            bytes: b"new-data".to_vec(),
            timestamp: 4,
            platforms: 7,
        }],
        vec!["delete".into()],
    )
    .unwrap();
    (
        root,
        job,
        Fake {
            remote: before,
            bodies: BTreeMap::from([
                ("keep".into(), b"same".to_vec()),
                ("delete".into(), b"old".to_vec()),
            ]),
            calls: vec![],
            fault: None,
            commit: true,
            blocks: BTreeMap::new(),
            pending: BTreeMap::new(),
            batch: (vec!["new".into()], vec!["delete".into()]),
            concurrent_edit: false,
            pause_block: false,
            move_journal: None,
            abort_receipt: Some(false),
        },
    )
}

#[test]
fn opening_requires_current_upload_schema_and_recovery_information() {
    let (_root, job, _server) = fixture();
    let directory = job.directory().to_owned();
    let path = directory.join("state.json");
    let current = serde_json::to_value(&job.state).unwrap();
    drop(job);
    assert!(UploadTransaction::open(&directory).is_ok());
    for case in 0..3 {
        let mut old = current.clone();
        match case {
            0 => old["schema"] = 1.into(),
            1 => old["schema"] = 2.into(),
            2 => {
                old.as_object_mut().unwrap().remove("unanswered_mutation");
            }
            _ => unreachable!(),
        }
        fs::write(&path, serde_json::to_vec(&old).unwrap()).unwrap();
        let before = fs::read(&path).unwrap();
        assert!(UploadTransaction::open(&directory).is_err());
        assert_eq!(fs::read(&path).unwrap(), before);
    }
}

struct Fake {
    remote: CloudListing,
    bodies: BTreeMap<String, Vec<u8>>,
    calls: Vec<String>,
    fault: Option<&'static str>,
    commit: bool,
    /// The bytes each file's blocks carried, by file name.
    blocks: BTreeMap<String, Vec<u8>>,
    pending: BTreeMap<String, wire::BeginUploadRequest>,
    /// The uploads and deletions the batch must announce.
    batch: (Vec<String>, Vec<String>),
    /// Someone else's file appears once the batch completed.
    concurrent_edit: bool,
    pause_block: bool,
    move_journal: Option<(PathBuf, PathBuf)>,
    abort_receipt: Option<bool>,
}
impl Fake {
    fn response<R: Message + Default>(
        &mut self,
        name: &str,
        response: &impl Message,
    ) -> Result<R, CloudError> {
        if self.fault == Some(name) {
            self.fault = None;
            return Err(CloudError::Network);
        }
        Ok(R::decode(response.encode_to_vec().as_slice()).unwrap())
    }
}
impl Transport for Fake {
    async fn call<R: Message + Default>(
        &mut self,
        name: &str,
        request: &impl Message,
    ) -> Result<R, CloudError> {
        self.calls.push(name.into());
        let data = request.encode_to_vec();
        match name {
            "Cloud.BeginAppUploadBatch#1" => {
                let req = wire::BeginBatchRequest::decode(data.as_slice()).unwrap();
                assert_eq!(req.uploads, self.batch.0);
                assert_eq!(req.deletes, self.batch.1);
                if let Some((from, to)) = self.move_journal.take() {
                    fs::rename(from, to).unwrap();
                }
                self.response(
                    name,
                    &wire::BeginBatchResponse {
                        batch_id: Some(123),
                        change_number: Some(3),
                    },
                )
            }
            "Cloud.ClientBeginFileUpload#1" => {
                let req = wire::BeginUploadRequest::decode(data.as_slice()).unwrap();
                if self.pending.contains_key(&req.name) {
                    return Err(CloudError::Cm(crate::cm::CmError::Rejected {
                        action: "fixture file busy",
                        result: 10,
                    }));
                }
                assert!(!req.can_encrypt);
                assert_eq!(req.file_size, req.raw_size);
                // Two parts, each block's path naming its file.
                let split = req.file_size.min(3);
                let block = |path: String, offset: u32, length: u32| wire::UploadBlock {
                    https: true,
                    host: "steamcloud-test.storage.googleapis.com".into(),
                    path,
                    method: 4,
                    offset: offset.into(),
                    length,
                    ..Default::default()
                };
                let plan = wire::BeginUploadResponse {
                    encrypted: false,
                    blocks: vec![
                        block(format!("/{}/part1?secret=never-log", req.name), 0, split),
                        block(format!("/{}/part2", req.name), split, req.file_size - split),
                    ],
                };
                self.pending.insert(req.name.clone(), req);
                self.response(name, &plan)
            }
            "Cloud.ClientCommitFileUpload#1" => {
                let req = wire::CommitUploadRequest::decode(data.as_slice()).unwrap();
                let pending = self.pending.remove(&req.name).unwrap();
                assert_eq!(req.sha, pending.sha);
                let sent = self.blocks.remove(&req.name).unwrap_or_default();
                if !req.succeeded {
                    self.calls.push("abort-file".into());
                }
                if self.commit && req.succeeded {
                    let bytes = sent;
                    assert_eq!(
                        crate::content::hex(&Sha1::digest(&bytes)),
                        crate::content::hex(&req.sha)
                    );
                    self.remote.files.retain(|file| file.name != req.name);
                    self.remote.files.push(file(&req.name, &bytes));
                    self.bodies.insert(req.name, bytes);
                }
                self.response(
                    name,
                    &wire::CommitUploadResponse {
                        committed: if req.succeeded {
                            Some(self.commit)
                        } else {
                            self.abort_receipt
                        },
                    },
                )
            }
            "Cloud.ClientDeleteFile#1" => {
                let req = wire::DeleteRequest::decode(data.as_slice()).unwrap();
                assert!(req.explicit);
                assert_eq!(req.batch_id, 123);
                self.remote.files.retain(|file| file.name != req.name);
                self.bodies.remove(&req.name);
                self.response(name, &wire::EmptyResponse {})
            }
            "Cloud.CompleteAppUploadBatchBlocking#1" => {
                let req = wire::CompleteBatchRequest::decode(data.as_slice()).unwrap();
                self.calls.push(format!("complete-result-{}", req.result));
                self.remote.change_number = 3;
                self.response(name, &wire::EmptyResponse {})
            }
            _ => panic!("unexpected test RPC"),
        }
    }
    async fn block(&mut self, block: &wire::UploadBlock, bytes: &[u8]) -> Result<(), CloudError> {
        self.calls.push("block".into());
        if self.fault == Some("block") {
            self.fault = None;
            return Err(CloudError::Network);
        }
        let request = block_request(block, bytes)?;
        let name = block.path.split('/').nth(1).unwrap().to_owned();
        self.blocks
            .entry(name)
            .or_default()
            .extend_from_slice(request.body().unwrap().as_bytes().unwrap());
        if self.pause_block {
            std::future::pending::<()>().await;
        }
        Ok(())
    }
    async fn listing(&mut self, _: u32) -> Result<CloudListing, CloudError> {
        self.calls.push("listing".into());
        if self.concurrent_edit && self.remote.change_number == 3 {
            self.remote
                .files
                .push(file("concurrent", b"someone else's edit"));
            self.concurrent_edit = false;
        }
        Ok(self.remote.clone())
    }
}

#[tokio::test]
async fn interrupted_http_recovery_releases_file_before_a_fresh_upload() {
    let (root, mut job, mut server) = fixture();
    server.pause_block = true;
    assert!(
        tokio::time::timeout(
            std::time::Duration::from_millis(20),
            execute(&mut server, &mut job, &|| true),
        )
        .await
        .is_err()
    );
    let path = job.directory().to_owned();
    drop(job);
    let mut job = UploadTransaction::open(&path).unwrap();
    server.pause_block = false;
    server.calls.clear();
    settle(&mut server, &mut job, &|| true).await.unwrap();
    assert!(job.is_settled());
    let mut fresh = UploadTransaction::create(
        &root.path().canonicalize().unwrap(),
        server.remote.clone(),
        vec![CloudUpload {
            name: "new".into(),
            bytes: b"new-data".to_vec(),
            timestamp: 4,
            platforms: 7,
        }],
        vec!["delete".into()],
    )
    .unwrap();
    execute(&mut server, &mut fresh, &|| true)
        .await
        .expect("settlement must release the staged file");
    assert_eq!(server.bodies["new"], b"new-data");
    assert_eq!(server.bodies["keep"], b"same");
    assert_eq!(
        &server.calls[..4],
        [
            "Cloud.ClientCommitFileUpload#1",
            "abort-file",
            "Cloud.CompleteAppUploadBatchBlocking#1",
            "complete-result-2"
        ]
    );
}

#[tokio::test]
async fn http_failure_reports_failed_file_before_batch_closure() {
    let (_root, mut job, mut server) = fixture();
    server.fault = Some("block");
    assert!(matches!(
        execute(&mut server, &mut job, &|| true).await,
        Err(CloudError::Network)
    ));
    assert!(job.is_settled());
    assert!(server.pending.is_empty());
    assert_eq!(
        &server.calls[server.calls.len() - 4..],
        [
            "Cloud.ClientCommitFileUpload#1",
            "abort-file",
            "Cloud.CompleteAppUploadBatchBlocking#1",
            "complete-result-2"
        ]
    );
    assert!(!server.bodies.contains_key("new"));
    assert_eq!(server.bodies["delete"], b"old");
}

#[tokio::test]
async fn lost_or_malformed_abort_receipt_never_allows_replay_or_settlement() {
    for (lost, receipt) in [(true, Some(false)), (false, None), (false, Some(true))] {
        let (_root, mut job, mut server) = fixture();
        server.pause_block = true;
        assert!(
            tokio::time::timeout(
                std::time::Duration::from_millis(20),
                execute(&mut server, &mut job, &|| true)
            )
            .await
            .is_err()
        );
        let path = job.directory().to_owned();
        drop(job);
        let mut job = UploadTransaction::open(&path).unwrap();
        server.abort_receipt = receipt;
        if lost {
            server.fault = Some("Cloud.ClientCommitFileUpload#1");
        }
        assert!(settle(&mut server, &mut job, &|| true).await.is_err());
        assert!(!job.is_settled());
        drop(job);
        let mut job = UploadTransaction::open(&path).unwrap();
        server.calls.clear();
        assert!(matches!(
            settle(&mut server, &mut job, &|| true).await,
            Err(CloudError::Uncertain)
        ));
        assert!(server.calls.is_empty());
        assert_eq!(server.bodies["keep"], b"same");
        assert!(!server.bodies.contains_key("new"));
    }
}

#[tokio::test]
async fn multipart_upload_requires_file_and_batch_acknowledgement_and_a_matching_listing() {
    let (_root, mut job, mut server) = fixture();
    let actual = execute(&mut server, &mut job, &|| true).await.unwrap();
    assert_eq!(actual.change_number, 3);
    assert_eq!(job.phase(), &UploadPhase::Verified);
    assert_eq!(server.bodies.get("new").unwrap(), b"new-data");
    assert_eq!(server.bodies.get("keep").unwrap(), b"same");
    assert!(!server.bodies.contains_key("delete"));
    assert_eq!(
        server.calls,
        [
            "listing",
            "Cloud.BeginAppUploadBatch#1",
            "Cloud.ClientBeginFileUpload#1",
            "block",
            "block",
            "Cloud.ClientCommitFileUpload#1",
            "Cloud.ClientDeleteFile#1",
            "Cloud.CompleteAppUploadBatchBlocking#1",
            "complete-result-1",
            "listing"
        ]
    );
    let directory = job.directory().to_owned();
    drop(job);
    let mut reopened = UploadTransaction::open(&directory).unwrap();
    server.calls.clear();
    execute(&mut server, &mut reopened, &|| true).await.unwrap();
    assert_eq!(server.calls, ["listing"]);
    let journal = fs::read_to_string(directory.join("state.json")).unwrap();
    assert!(!journal.contains("never-log") && !journal.contains("https:"));
}

#[tokio::test]
async fn interruption_at_each_remote_mutation_never_restarts_an_uncertain_transaction() {
    for fault in [
        "Cloud.BeginAppUploadBatch#1",
        "Cloud.ClientBeginFileUpload#1",
        "block",
        "Cloud.ClientCommitFileUpload#1",
        "Cloud.ClientDeleteFile#1",
        "Cloud.CompleteAppUploadBatchBlocking#1",
    ] {
        let (_root, mut job, mut server) = fixture();
        server.fault = Some(fault);
        assert!(
            execute(&mut server, &mut job, &|| true).await.is_err(),
            "{fault}"
        );
        let directory = job.directory().to_owned();
        drop(job);
        let mut reopened = UploadTransaction::open(&directory).unwrap();
        server.calls.clear();
        assert!(
            matches!(
                execute(&mut server, &mut reopened, &|| true).await,
                Err(CloudError::Uncertain)
            ),
            "{fault}"
        );
        assert!(server.calls.is_empty(), "{fault}");
        assert_eq!(server.bodies["keep"], b"same");
    }
}

#[tokio::test]
async fn dropping_an_active_transfer_preserves_uncertainty_and_never_commits() {
    let (_root, mut job, mut server) = fixture();
    server.pause_block = true;
    let cancelled = tokio::time::timeout(
        std::time::Duration::from_millis(20),
        execute(&mut server, &mut job, &|| true),
    )
    .await;
    assert!(cancelled.is_err());
    assert_eq!(server.blocks.len(), 1);
    assert!(!server.bodies.contains_key("new"));
    assert!(
        !server
            .calls
            .iter()
            .any(|call| call == "Cloud.ClientCommitFileUpload#1")
    );
    let directory = job.directory().to_owned();
    drop(job);
    let mut reopened = UploadTransaction::open(&directory).unwrap();
    server.calls.clear();
    assert!(matches!(
        execute(&mut server, &mut reopened, &|| true).await,
        Err(CloudError::Uncertain)
    ));
    assert!(server.calls.is_empty());
}

#[tokio::test]
async fn journal_failure_after_batch_admission_prevents_all_file_mutations() {
    let (_root, mut job, mut server) = fixture();
    let original = job.directory().to_owned();
    let moved = original.with_extension("moved");
    server.move_journal = Some((original.clone(), moved.clone()));
    assert!(matches!(
        execute(&mut server, &mut job, &|| true).await,
        Err(CloudError::Storage)
    ));
    assert!(
        !server
            .calls
            .iter()
            .any(|call| call == "Cloud.ClientBeginFileUpload#1")
    );
    fs::rename(&moved, &original).unwrap();
    server.calls.clear();
    assert!(matches!(
        execute(&mut server, &mut job, &|| true).await,
        Err(CloudError::Storage)
    ));
    assert!(server.calls.is_empty());
    drop(job);
    let mut reopened = UploadTransaction::open(&original).unwrap();
    assert!(matches!(
        execute(&mut server, &mut reopened, &|| true).await,
        Err(CloudError::Uncertain)
    ));
    assert!(server.calls.is_empty());
}

#[tokio::test]
async fn failed_commit_closes_batch_as_failure_and_does_not_delete_other_files() {
    let (_root, mut job, mut server) = fixture();
    server.commit = false;
    assert!(matches!(
        execute(&mut server, &mut job, &|| true).await,
        Err(CloudError::NotCommitted)
    ));
    assert!(server.calls.contains(&"complete-result-2".into()));
    assert!(!server.calls.contains(&"complete-result-1".into()));
    assert!(server.bodies.contains_key("delete"));
}

#[tokio::test]
async fn negative_commit_and_acknowledged_closure_are_durable_without_resending_files() {
    let (_root, mut job, mut server) = fixture();
    server.commit = false;
    assert!(matches!(
        execute(&mut server, &mut job, &|| true).await,
        Err(CloudError::NotCommitted)
    ));
    let path = job.directory().to_owned();
    drop(job);
    let mut job = UploadTransaction::open(&path).unwrap();
    // A negative file result and positive batch closure leave no unanswered
    // mutation. The existing transport discards that closure acknowledgement.
    assert_eq!(
        serde_json::to_value(job.phase()).unwrap(),
        serde_json::json!({"Closed": {"batch_id": 123}})
    );
    assert_eq!(server.bodies["keep"], b"same");
    assert!(server.bodies.contains_key("delete"));
    assert!(!server.bodies.contains_key("new"));
    assert!(job.is_settled());
    server.calls.clear();
    settle(&mut server, &mut job, &|| true).await.unwrap();
    assert!(server.calls.is_empty());
}

#[tokio::test]
async fn lost_closure_reply_can_be_settled_without_repeating_acknowledged_file_mutations() {
    let (_root, mut job, mut server) = fixture();
    server.fault = Some("Cloud.CompleteAppUploadBatchBlocking#1");
    assert!(matches!(
        execute(&mut server, &mut job, &|| true).await,
        Err(CloudError::Network)
    ));
    assert!(!job.is_settled());
    let path = job.directory().to_owned();
    drop(job);
    let mut job = UploadTransaction::open(&path).unwrap();
    server.calls.clear();
    settle(&mut server, &mut job, &|| true).await.unwrap();
    assert!(job.is_settled());
    assert_eq!(
        server.calls,
        [
            "Cloud.CompleteAppUploadBatchBlocking#1",
            "complete-result-2"
        ]
    );
    assert_eq!(server.bodies["new"], b"new-data");
    assert!(!server.bodies.contains_key("delete"));
    drop(job);
    assert!(UploadTransaction::open(&path).unwrap().is_settled());
}

#[tokio::test]
async fn closure_ack_never_settles_a_file_request_with_a_lost_response() {
    for fault in [
        "Cloud.BeginAppUploadBatch#1",
        "Cloud.ClientBeginFileUpload#1",
        "Cloud.ClientCommitFileUpload#1",
        "Cloud.ClientDeleteFile#1",
    ] {
        let (_root, mut job, mut server) = fixture();
        server.fault = Some(fault);
        assert!(execute(&mut server, &mut job, &|| true).await.is_err());
        assert!(!server.calls.contains(&"abort-file".into()), "{fault}");
        let path = job.directory().to_owned();
        drop(job);
        let mut job = UploadTransaction::open(&path).unwrap();
        assert!(!job.is_settled(), "{fault}");
        server.calls.clear();
        assert!(
            matches!(
                settle(&mut server, &mut job, &|| true).await,
                Err(CloudError::Uncertain)
            ),
            "{fault}"
        );
        assert!(server.calls.is_empty(), "{fault}");
    }
}

#[tokio::test]
async fn stopped_http_body_before_commit_can_close_but_revocation_or_lost_ack_cannot_settle() {
    let (_root, mut job, mut server) = fixture();
    server.pause_block = true;
    assert!(
        tokio::time::timeout(
            std::time::Duration::from_millis(20),
            execute(&mut server, &mut job, &|| true),
        )
        .await
        .is_err()
    );
    let path = job.directory().to_owned();
    drop(job);
    let mut job = UploadTransaction::open(&path).unwrap();
    server.calls.clear();
    assert!(matches!(
        settle(&mut server, &mut job, &|| false).await,
        Err(CloudError::Revoked)
    ));
    assert!(server.calls.is_empty());
    server.fault = Some("Cloud.CompleteAppUploadBatchBlocking#1");
    assert!(settle(&mut server, &mut job, &|| true).await.is_err());
    assert!(!job.is_settled());
    server.calls.clear();
    settle(&mut server, &mut job, &|| true).await.unwrap();
    assert!(job.is_settled());
    assert_eq!(
        server.calls,
        [
            "Cloud.CompleteAppUploadBatchBlocking#1",
            "complete-result-2"
        ]
    );
    assert!(!server.bodies.contains_key("new"));
    assert!(server.bodies.contains_key("delete"));
}

#[tokio::test]
async fn stale_remote_view_or_revocation_prevents_mutation_and_a_concurrent_edit_is_never_confirmed()
 {
    let (_root, mut job, mut server) = fixture();
    assert!(matches!(
        execute(&mut server, &mut job, &|| false).await,
        Err(CloudError::Revoked)
    ));
    assert!(server.calls.is_empty());
    server.remote.change_number += 1;
    assert!(matches!(
        execute(&mut server, &mut job, &|| true).await,
        Err(CloudError::Changed)
    ));
    assert_eq!(server.calls, ["listing"]);
    assert_eq!(job.phase(), &UploadPhase::Prepared);
    let (_root, mut job, mut server) = fixture();
    server.concurrent_edit = true;
    assert!(matches!(
        execute(&mut server, &mut job, &|| true).await,
        Err(CloudError::Changed)
    ));
    assert_eq!(job.phase(), &UploadPhase::Acknowledged);
}

#[tokio::test]
async fn authorization_is_checked_between_requests_and_bad_archives_never_reach_network() {
    use std::cell::Cell;
    let (_root, mut job, mut server) = fixture();
    let calls = Cell::new(0);
    let guard = || {
        calls.set(calls.get() + 1);
        calls.get() < 5
    };
    assert!(matches!(
        execute(&mut server, &mut job, &guard).await,
        Err(CloudError::Revoked)
    ));
    assert!(
        !server
            .calls
            .contains(&"Cloud.ClientCommitFileUpload#1".into())
    );
    let (_root, mut job, mut server) = fixture();
    fs::write(job.directory().join("file-0.bin"), b"corrupt").unwrap();
    assert!(matches!(
        execute(&mut server, &mut job, &|| true).await,
        Err(CloudError::Integrity)
    ));
    assert_eq!(server.calls, ["listing"]);
}

#[test]
fn upload_plans_honor_explicit_bodies_offsets_methods_and_authorities() {
    let mut block = wire::UploadBlock {
        https: true,
        host: "steamcloud-test.storage.googleapis.com".into(),
        path: "/data?sig=secret".into(),
        method: 4,
        offset: 2,
        length: 3,
        ..Default::default()
    };
    assert_eq!(
        block_request(&block, b"abcdef")
            .unwrap()
            .body()
            .unwrap()
            .as_bytes()
            .unwrap(),
        b"cde"
    );
    block.offset = u64::MAX;
    assert!(block_request(&block, b"abcdef").is_err());
    block.body = Some(b"completion document".to_vec());
    block.method = 3;
    assert_eq!(
        block_request(&block, b"abcdef")
            .unwrap()
            .body()
            .unwrap()
            .as_bytes()
            .unwrap(),
        b"completion document"
    );
    block.body = Some(vec![]);
    block.method = 1;
    assert!(block_request(&block, b"abcdef").is_ok());
    block.body = None;
    assert!(block_request(&block, b"abcdef").is_err());
    block.offset = 0;
    block.method = 4;
    block.headers = vec![wire::Header {
        name: "Host".into(),
        value: "evil.invalid".into(),
    }];
    assert!(block_request(&block, b"abcdef").is_err());
    block.headers.clear();
    block.https = false;
    assert!(block_request(&block, b"abcdef").is_err());
    block.https = true;
    block.host = "localhost".into();
    assert!(block_request(&block, b"abcdef").is_err());
}

#[test]
fn journal_lock_and_immutable_content_survive_reopen_and_reject_links() {
    let (_root, job, _) = fixture();
    let directory = job.directory().to_owned();
    assert!(UploadTransaction::open(&directory).is_err());
    drop(job);
    let reopened = UploadTransaction::open(&directory).unwrap();
    drop(reopened);
    fs::hard_link(directory.join("file-0.bin"), directory.join("alias.bin")).unwrap();
    assert!(UploadTransaction::open(&directory).is_err());
}

/// A batch of `uploads` new files and `deletes` deletions, beside a kept file.
fn batch_fixture(uploads: usize, deletes: usize) -> (tempfile::TempDir, UploadTransaction, Fake) {
    let (root, job, mut server) = fixture();
    drop(job);
    let parent = root.path().canonicalize().unwrap();
    let doomed: Vec<String> = (0..deletes).map(|i| format!("old-{i:02}")).collect();
    let mut before = listing(vec![file("keep", b"same")]);
    before
        .files
        .extend(doomed.iter().map(|name| file(name, name.as_bytes())));
    let new: Vec<_> = (0..uploads)
        .map(|i| CloudUpload {
            name: format!("file-{i:02}"),
            bytes: format!("data-{i}").into_bytes(),
            timestamp: 4,
            platforms: 7,
        })
        .collect();
    server.batch = (new.iter().map(|u| u.name.clone()).collect(), doomed.clone());
    server.remote = before.clone();
    server.bodies = doomed
        .iter()
        .map(|name| (name.clone(), name.as_bytes().to_vec()))
        .chain([("keep".to_owned(), b"same".to_vec())])
        .collect();
    let job = UploadTransaction::create(&parent, before, new, doomed).unwrap();
    (root, job, server)
}

#[tokio::test]
async fn files_go_in_waves_of_ten_before_the_deletions() {
    let (_root, mut job, mut server) = batch_fixture(12, 3);
    execute(&mut server, &mut job, &|| true).await.unwrap();
    assert_eq!(job.phase(), &UploadPhase::Verified);
    for i in 0..12 {
        assert_eq!(
            server.bodies[&format!("file-{i:02}")],
            format!("data-{i}").as_bytes()
        );
    }
    assert!(!server.bodies.keys().any(|name| name.starts_with("old-")));
    let wave = |files: usize| {
        [
            vec!["Cloud.ClientBeginFileUpload#1"; files],
            vec!["block"; 2 * files],
            vec!["Cloud.ClientCommitFileUpload#1"; files],
        ]
        .concat()
    };
    let expected = [
        vec!["listing", "Cloud.BeginAppUploadBatch#1"],
        wave(PARALLEL_FILES),
        wave(2),
        vec!["Cloud.ClientDeleteFile#1"; 3],
        vec![
            "Cloud.CompleteAppUploadBatchBlocking#1",
            "complete-result-1",
            "listing",
        ],
    ]
    .concat();
    assert_eq!(server.calls, expected);
}

#[tokio::test]
async fn a_failed_body_reports_every_file_of_its_wave_before_closing() {
    let (_root, mut job, mut server) = batch_fixture(3, 1);
    server.fault = Some("block");
    assert!(matches!(
        execute(&mut server, &mut job, &|| true).await,
        Err(CloudError::Network)
    ));
    assert!(job.is_settled());
    assert!(server.pending.is_empty());
    assert!(!server.bodies.contains_key("file-00"));
    assert_eq!(
        &server.calls[server.calls.len() - 8..],
        [
            "Cloud.ClientCommitFileUpload#1",
            "abort-file",
            "Cloud.ClientCommitFileUpload#1",
            "abort-file",
            "Cloud.ClientCommitFileUpload#1",
            "abort-file",
            "Cloud.CompleteAppUploadBatchBlocking#1",
            "complete-result-2"
        ]
    );
}

#[test]
fn a_schema_3_journal_opens_with_its_one_staged_upload() {
    let (_root, job, _server) = fixture();
    let directory = job.directory().to_owned();
    let path = directory.join("state.json");
    let mut current = serde_json::to_value(&job.state).unwrap();
    drop(job);
    let object = current.as_object_mut().unwrap();
    object.remove("staged_uploads");
    object.insert("schema".into(), 3.into());
    object.insert(
        "phase".into(),
        serde_json::json!({"Open": {"batch_id": 123}}),
    );
    for (staged, opens) in [
        (serde_json::json!(0), true),
        (serde_json::Value::Null, true),
        (serde_json::json!(1), false),
        (serde_json::json!([0, 0]), false),
    ] {
        let mut old = current.clone();
        old["staged_upload"] = staged.clone();
        fs::write(&path, serde_json::to_vec(&old).unwrap()).unwrap();
        let opened = UploadTransaction::open(&directory);
        assert_eq!(opened.is_ok(), opens, "{staged}");
        if let Ok(job) = opened {
            assert_eq!(
                job.state.staged_uploads.len(),
                usize::from(!staged.is_null())
            );
            assert_eq!(job.state.schema, SCHEMA);
            assert!(!job.is_settled());
        }
    }
}

#[test]
fn a_wave_holds_ten_files_or_64_megabytes() {
    let sized = |sizes: &[u32]| -> Vec<CloudFile> {
        sizes
            .iter()
            .enumerate()
            .map(|(i, size)| CloudFile {
                size: *size,
                ..file(&format!("file-{i}"), b"")
            })
            .collect()
    };
    const MB: u32 = 1024 * 1024;
    assert_eq!(
        waves(&sized(&[1; 12])),
        [(0..10).collect::<Vec<_>>(), vec![10, 11]]
    );
    assert_eq!(
        waves(&sized(&[40 * MB, 24 * MB, 1, 100 * MB, 2])),
        [vec![0, 1], vec![2], vec![3], vec![4]]
    );
    assert!(waves(&[]).is_empty());
}
