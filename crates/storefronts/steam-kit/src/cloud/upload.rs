//! CM upload execution. Policy, preferences and native-runtime exclusion belong
//! to the caller. Never resume a possibly sent mutation by sending it again.
use super::{CloudError, CloudFile, CloudListing, read, wire};
use prost::Message;
use serde::{Deserialize, Serialize};
use sha1::{Digest, Sha1};
use std::{
    collections::BTreeMap,
    fs,
    io::Write,
    os::unix::fs::{MetadataExt, PermissionsExt},
    path::{Path, PathBuf},
};

const MAX_FILE: usize = 256 * 1024 * 1024;
const MAX_JOURNAL: u64 = 64 * 1024 * 1024;
/// How many file requests of one batch Steam's client keeps in flight.
pub(crate) const PARALLEL_FILES: usize = 10;
/// How many bytes Steam's client uploads at once, unless one file is larger.
const PARALLEL_BYTES: u64 = 64 * 1024 * 1024;
/// The journal's format. Schema 3 journals still open.
const SCHEMA: u32 = 4;

/// Bytes must come from the coordinator's immutable archive, not a running game.
pub struct CloudUpload {
    pub name: String,
    pub bytes: Vec<u8>,
    pub timestamp: u64,
    pub platforms: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum UploadPhase {
    Prepared,
    /// The begin request might have arrived even when its reply did not.
    Beginning,
    Open {
        batch_id: u64,
    },
    Completing {
        batch_id: u64,
    },
    Acknowledged,
    Verified,
    /// Closing a failed batch does not roll back already committed files.
    Interrupted {
        batch_id: Option<u64>,
    },
    /// Steam acknowledged closure. This alone does not fence an older file
    /// mutation whose response was lost; the journal tracks that separately.
    Closed {
        batch_id: u64,
    },
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct State {
    schema: u32,
    before: CloudListing,
    uploads: Vec<CloudFile>,
    deletes: Vec<String>,
    phase: UploadPhase,
    unanswered_mutation: bool,
    // Begin-file replied; no positive commit has been sent while unanswered is
    // false. Retain the exact files needed to report interrupted HTTP uploads.
    // Schema 3 kept at most one, as `staged_upload`.
    #[serde(alias = "staged_upload", deserialize_with = "staged")]
    staged_uploads: Vec<usize>,
}

/// The staged uploads as either schema wrote them.
fn staged<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<Vec<usize>, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Staged {
        One(Option<usize>),
        Many(Vec<usize>),
    }
    Ok(match Staged::deserialize(deserializer)? {
        Staged::One(one) => one.into_iter().collect(),
        Staged::Many(many) => many,
    })
}

/// Private, locked journal and immutable file bodies. No signed URL, header or
/// credential is persisted. Retain this directory until the coordinator has
/// reconciled the outcome; interrupted transactions are never restarted here.
pub struct UploadTransaction {
    directory: PathBuf,
    state: State,
    poisoned: bool,
    _lock: fs::File,
}

impl UploadTransaction {
    pub fn create(
        parent: &Path,
        before: CloudListing,
        uploads: Vec<CloudUpload>,
        deletes: Vec<String>,
    ) -> Result<Self, CloudError> {
        safe_directory(parent)?;
        if before.app_id == 0
            || before.steam_id == 0
            || before.files.len() > 100_000
            || uploads.len() + deletes.len() > 100_000
            || uploads.is_empty() && deletes.is_empty()
        {
            return Err(CloudError::Invalid("upload scope"));
        }
        let directory = tempfile::Builder::new()
            .prefix("upload-")
            .permissions(fs::Permissions::from_mode(0o700))
            .tempdir_in(parent)
            .map_err(|_| CloudError::Storage)?;
        let mut files = Vec::new();
        for (index, upload) in uploads.into_iter().enumerate() {
            if upload.bytes.len() > MAX_FILE {
                return Err(CloudError::Invalid("upload size"));
            }
            let file = CloudFile {
                name: upload.name,
                sha1: crate::content::hex(&Sha1::digest(&upload.bytes)),
                size: upload.bytes.len() as u32,
                timestamp: upload.timestamp,
                persist_state: 0,
                platforms: upload.platforms,
                reupload_requested: false,
            };
            write_new(
                &directory.path().join(format!("file-{index}.bin")),
                &upload.bytes,
            )?;
            files.push(file);
        }
        let state = State {
            schema: SCHEMA,
            before,
            uploads: files,
            deletes,
            phase: UploadPhase::Prepared,
            unanswered_mutation: false,
            staged_uploads: Vec::new(),
        };
        expected(&state)?;
        let lock = lock(directory.path())?;
        let mut transaction = Self {
            directory: directory.path().into(),
            state,
            poisoned: false,
            _lock: lock,
        };
        transaction.save(UploadPhase::Prepared)?;
        let _ = directory.keep();
        sync(parent)?;
        Ok(transaction)
    }

    pub fn open(directory: &Path) -> Result<Self, CloudError> {
        safe_directory(directory)?;
        let lock = lock(directory)?;
        let path = directory.join("state.json");
        regular(&path, MAX_JOURNAL)?;
        let mut state: State =
            serde_json::from_slice(&fs::read(&path).map_err(|_| CloudError::Storage)?)
                .map_err(|_| CloudError::Storage)?;
        if !(3..=SCHEMA).contains(&state.schema)
            || state.before.app_id == 0
            || state.before.steam_id == 0
        {
            return Err(CloudError::Invalid("upload journal"));
        }
        state.schema = SCHEMA;
        expected(&state)?;
        let mut staged = state.staged_uploads.clone();
        staged.sort_unstable();
        staged.dedup();
        if staged.len() != state.staged_uploads.len()
            || staged.last().is_some_and(|i| *i >= state.uploads.len())
            || matches!(
                state.phase,
                UploadPhase::Prepared
                    | UploadPhase::Completing { .. }
                    | UploadPhase::Acknowledged
                    | UploadPhase::Verified
            ) && (!state.staged_uploads.is_empty() || state.unanswered_mutation)
        {
            return Err(CloudError::Invalid("upload progress"));
        }
        let transaction = Self {
            directory: directory.into(),
            state,
            poisoned: false,
            _lock: lock,
        };
        for (index, file) in transaction.state.uploads.iter().enumerate() {
            transaction.content(index, file)?;
        }
        Ok(transaction)
    }

    pub fn directory(&self) -> &Path {
        &self.directory
    }
    pub fn phase(&self) -> &UploadPhase {
        &self.state.phase
    }
    pub fn steam_id(&self) -> u64 {
        self.state.before.steam_id
    }
    pub fn app_id(&self) -> u32 {
        self.state.before.app_id
    }
    /// The planned file writes, in upload order.
    pub fn uploads(&self) -> &[CloudFile] {
        &self.state.uploads
    }
    /// The planned remote deletions.
    pub fn deletes(&self) -> &[String] {
        &self.state.deletes
    }
    /// The immutable body of planned upload `index`, checked against its hash.
    pub fn upload_body(&self, index: usize) -> Result<Vec<u8>, CloudError> {
        let file = self
            .state
            .uploads
            .get(index)
            .ok_or(CloudError::Invalid("upload index"))?;
        self.content(index, file)
    }

    /// Reports complete receipts for the coordinator's confirmed recovery path.
    /// False does not prevent a separate fresh-comparison retry policy. This
    /// method says nothing about rollback or equality.
    pub fn is_settled(&self) -> bool {
        !self.poisoned
            && match self.phase() {
                UploadPhase::Prepared | UploadPhase::Acknowledged | UploadPhase::Verified => true,
                UploadPhase::Closed { .. } => {
                    !self.state.unanswered_mutation && self.state.staged_uploads.is_empty()
                }
                _ => false,
            }
    }

    fn save(&mut self, phase: UploadPhase) -> Result<(), CloudError> {
        self.save_progress(phase, self.state.unanswered_mutation)
    }

    fn save_progress(
        &mut self,
        phase: UploadPhase,
        unanswered_mutation: bool,
    ) -> Result<(), CloudError> {
        let mut next = self.state.clone();
        next.phase = phase;
        next.unanswered_mutation = unanswered_mutation;
        self.persist(next)
    }

    fn save_file_progress(
        &mut self,
        unanswered: bool,
        staged: Vec<usize>,
    ) -> Result<(), CloudError> {
        let mut next = self.state.clone();
        next.unanswered_mutation = unanswered;
        next.staged_uploads = staged;
        self.persist(next)
    }

    fn persist(&mut self, next: State) -> Result<(), CloudError> {
        if self.poisoned {
            return Err(CloudError::Storage);
        }
        let result = (|| {
            let bytes = serde_json::to_vec(&next).map_err(|_| CloudError::Storage)?;
            if bytes.len() as u64 > MAX_JOURNAL {
                return Err(CloudError::Storage);
            }
            obelisk_core::write_atomic(&self.directory.join("state.json"), &bytes)
                .map_err(|_| CloudError::Storage)?;
            sync(&self.directory)
        })();
        if result.is_err() {
            self.poisoned = true;
        }
        result?;
        self.state = next;
        Ok(())
    }

    fn content(&self, index: usize, file: &CloudFile) -> Result<Vec<u8>, CloudError> {
        let path = self.directory.join(format!("file-{index}.bin"));
        regular(&path, MAX_FILE as u64)?;
        let bytes = fs::read(path).map_err(|_| CloudError::Storage)?;
        file.verify(&bytes)?;
        Ok(bytes)
    }
}

fn safe_directory(directory: &Path) -> Result<(), CloudError> {
    if !directory.is_absolute() {
        return Err(CloudError::Storage);
    }
    // macOS /var is a system alias. Canonicalize the caller's parent first, but
    // reject user-supplied links at any subsequent transaction path component.
    for parent in directory.ancestors() {
        if !fs::symlink_metadata(parent).is_ok_and(|m| m.file_type().is_dir()) {
            return Err(CloudError::Storage);
        }
    }
    Ok(())
}
fn regular(path: &Path, max: u64) -> Result<(), CloudError> {
    let metadata = fs::symlink_metadata(path).map_err(|_| CloudError::Storage)?;
    if !metadata.file_type().is_file() || metadata.nlink() != 1 || metadata.len() > max {
        return Err(CloudError::Storage);
    }
    Ok(())
}
fn lock(directory: &Path) -> Result<fs::File, CloudError> {
    let path = directory.join("owner.lock");
    let file = match fs::File::create_new(&path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            regular(&path, 0)?;
            fs::File::options()
                .read(true)
                .write(true)
                .open(&path)
                .map_err(|_| CloudError::Storage)?
        }
        Err(_) => return Err(CloudError::Storage),
    };
    file.set_permissions(fs::Permissions::from_mode(0o600))
        .map_err(|_| CloudError::Storage)?;
    file.try_lock().map_err(|_| CloudError::Storage)?;
    Ok(file)
}
fn write_new(path: &Path, bytes: &[u8]) -> Result<(), CloudError> {
    let file = fs::File::create_new(path).map_err(|_| CloudError::Storage)?;
    write_private(file, bytes)
}
/// Account data is written owner-only and fsynced before the caller treats it
/// as published.
pub(crate) fn write_private(mut file: fs::File, bytes: &[u8]) -> Result<(), CloudError> {
    file.set_permissions(fs::Permissions::from_mode(0o600))
        .map_err(|_| CloudError::Storage)?;
    file.write_all(bytes).map_err(|_| CloudError::Storage)?;
    file.sync_all().map_err(|_| CloudError::Storage)
}
/// fsync a directory so a completed rename or create within it is durable.
pub(crate) fn sync(path: &Path) -> Result<(), CloudError> {
    fs::File::open(path)
        .and_then(|file| file.sync_all())
        .map_err(|_| CloudError::Storage)
}

fn valid_file(file: &CloudFile) -> bool {
    !file.name.is_empty()
        && file.name.len() <= 4096
        && !file.name.chars().any(char::is_control)
        && file.size as usize <= MAX_FILE
        && (0..=2).contains(&file.persist_state)
        && (file.persist_state != 0
            || file.sha1.len() == 40
                && file
                    .sha1
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)))
}
fn expected(state: &State) -> Result<BTreeMap<String, CloudFile>, CloudError> {
    if state.before.files.len() > 100_000 || state.uploads.len() + state.deletes.len() > 100_000 {
        return Err(CloudError::Invalid("upload manifest size"));
    }
    let mut files = BTreeMap::new();
    for file in &state.before.files {
        if !valid_file(file) || files.insert(file.name.clone(), file.clone()).is_some() {
            return Err(CloudError::Invalid("remote file"));
        }
    }
    let mut changed = std::collections::BTreeSet::new();
    for file in &state.uploads {
        if !valid_file(file) || file.persist_state != 0 || !changed.insert(&file.name) {
            return Err(CloudError::Invalid("upload file"));
        }
        files.insert(file.name.clone(), file.clone());
    }
    for name in &state.deletes {
        if !changed.insert(name) || !files.get(name).is_some_and(|file| file.persist_state == 0) {
            return Err(CloudError::Invalid("upload deletion"));
        }
        files.remove(name);
    }
    files.retain(|_, file| file.persist_state == 0);
    if files.len() > 100_000 {
        return Err(CloudError::Invalid("upload manifest size"));
    }
    Ok(files)
}

pub(crate) trait Transport {
    async fn commit(
        &mut self,
        request: &wire::CommitUploadRequest,
    ) -> Result<wire::CommitUploadResponse, CloudError> {
        self.call("Cloud.ClientCommitFileUpload#1", request).await
    }
    async fn call<R: Message + Default>(
        &mut self,
        name: &str,
        request: &impl Message,
    ) -> Result<R, CloudError>;
    /// Answers `requests` to the same method, in their order. A transport
    /// may keep up to `PARALLEL_FILES` in flight. The first failure ends the
    /// set, and the requests already sent may have arrived.
    async fn calls<R: Message + Default>(
        &mut self,
        name: &str,
        requests: &[impl Message],
    ) -> Result<Vec<R>, CloudError> {
        let mut answers = Vec::with_capacity(requests.len());
        for request in requests {
            answers.push(self.call(name, request).await?);
        }
        Ok(answers)
    }
    async fn block(&mut self, block: &wire::UploadBlock, bytes: &[u8]) -> Result<(), CloudError>;
    /// Sends each file's blocks in sequence. A transport may send up to
    /// `PARALLEL_FILES` files at once; the first failure ends the set.
    async fn bodies(&mut self, files: &[(&[wire::UploadBlock], &[u8])]) -> Result<(), CloudError> {
        for (blocks, bytes) in files {
            for block in *blocks {
                self.block(block, bytes).await?;
            }
        }
        Ok(())
    }
    /// Commits each file, as `calls` answers requests.
    async fn commits(
        &mut self,
        requests: &[wire::CommitUploadRequest],
    ) -> Result<Vec<wire::CommitUploadResponse>, CloudError> {
        let mut receipts = Vec::with_capacity(requests.len());
        for request in requests {
            receipts.push(self.commit(request).await?);
        }
        Ok(receipts)
    }
    async fn listing(&mut self, app_id: u32) -> Result<CloudListing, CloudError>;
}

/// Every step re-checks authorization: a revoked session must not advance a
/// partially applied batch.
fn check(authorize: &impl Fn() -> bool) -> Result<(), CloudError> {
    if authorize() {
        Ok(())
    } else {
        Err(CloudError::Revoked)
    }
}

/// Runs the batch, then confirms it with one complete listing: every file
/// must carry the identity the batch gave it. Like Steam's own client, no
/// remote byte is read back.
pub(crate) async fn execute(
    transport: &mut impl Transport,
    job: &mut UploadTransaction,
    authorize: &impl Fn() -> bool,
) -> Result<CloudListing, CloudError> {
    if job.poisoned {
        return Err(CloudError::Storage);
    }
    let app = job.app_id();
    check(authorize)?;
    match job.phase() {
        UploadPhase::Prepared => {
            // Acquire the current complete view before any mutation. This detects
            // stale user choices; the protocol does not offer atomic CAS.
            if transport.listing(app).await? != job.state.before {
                return Err(CloudError::Changed);
            }
            for (index, file) in job.state.uploads.iter().enumerate() {
                job.content(index, file)?;
            }
            check(authorize)?;
            job.save_progress(UploadPhase::Beginning, true)?;
            let begin: wire::BeginBatchResponse = transport
                .call(
                    "Cloud.BeginAppUploadBatch#1",
                    &wire::BeginBatchRequest {
                        app_id: app,
                        machine_name: "DBDSilicon".into(),
                        uploads: job.state.uploads.iter().map(|f| f.name.clone()).collect(),
                        deletes: job.state.deletes.clone(),
                    },
                )
                .await?;
            let batch_id = begin
                .batch_id
                .filter(|id| *id != 0)
                .ok_or(CloudError::Invalid("upload batch id"))?;
            job.save_progress(UploadPhase::Open { batch_id }, false)?;
            let uploaded = upload_files(transport, job, batch_id, authorize).await;
            if let Err(error) = uploaded {
                // Best-effort closure releases Steam's batch. It never reports
                // rollback or permits a blind retry of already sent mutations.
                job.save(UploadPhase::Interrupted {
                    batch_id: Some(batch_id),
                })?;
                if authorize() {
                    // A failed HTTP transfer still needs its negative file
                    // receipt. Never negate an unanswered positive commit.
                    let _ = abort_staged_uploads(transport, job, authorize).await;
                    if job.poisoned {
                        return Err(CloudError::Storage);
                    }
                    // Preserve the original failure if the best-effort close
                    // also fails. A failed journal write still poisons the job.
                    if authorize() {
                        let _ = close_batch(transport, job, batch_id).await;
                    }
                    if job.poisoned {
                        return Err(CloudError::Storage);
                    }
                }
                return Err(error);
            }
            check(authorize)?;
            job.save(UploadPhase::Completing { batch_id })?;
            let _: wire::EmptyResponse = transport
                .call(
                    "Cloud.CompleteAppUploadBatchBlocking#1",
                    &wire::CompleteBatchRequest {
                        app_id: app,
                        batch_id,
                        result: 1,
                    },
                )
                .await?;
            job.save(UploadPhase::Acknowledged)?;
        }
        UploadPhase::Acknowledged | UploadPhase::Verified => {}
        _ => return Err(CloudError::Uncertain),
    }
    check(authorize)?;
    let actual = transport.listing(app).await?;
    let desired = expected(&job.state)?;
    let persisted: BTreeMap<_, _> = actual
        .files
        .iter()
        .filter(|f| f.persist_state == 0)
        .map(|f| (f.name.clone(), f))
        .collect();
    if actual.steam_id != job.steam_id()
        || actual.app_id != app
        || persisted.len() != desired.len()
        || desired.iter().any(|(name, file)| {
            persisted.get(name).is_none_or(|other| {
                file.sha1 != other.sha1
                    || file.size != other.size
                    || file.platforms != other.platforms
                    || other.reupload_requested
            })
        })
    {
        return Err(CloudError::Changed);
    }
    check(authorize)?;
    job.save(UploadPhase::Verified)?;
    Ok(actual)
}

async fn upload_files(
    transport: &mut impl Transport,
    job: &mut UploadTransaction,
    batch_id: u64,
    authorize: &impl Fn() -> bool,
) -> Result<(), CloudError> {
    for wave in waves(&job.state.uploads) {
        upload_wave(transport, job, batch_id, &wave, authorize).await?;
    }
    if job.state.deletes.is_empty() {
        return Ok(());
    }
    // As Steam's client does, the deletions go out together: the journal
    // holds them as one unanswered mutation until each is answered.
    let deletes: Vec<_> = job
        .state
        .deletes
        .iter()
        .map(|name| wire::DeleteRequest {
            app_id: job.app_id(),
            name: name.clone(),
            explicit: true,
            batch_id,
        })
        .collect();
    check(authorize)?;
    job.save_progress(job.phase().clone(), true)?;
    let _: Vec<wire::EmptyResponse> = transport
        .calls("Cloud.ClientDeleteFile#1", &deletes)
        .await?;
    job.save_progress(job.phase().clone(), false)
}

/// The uploads' indexes in order, grouped as Steam's client bounds its
/// parallel uploads: at most `PARALLEL_FILES` files and `PARALLEL_BYTES`.
fn waves(uploads: &[CloudFile]) -> Vec<Vec<usize>> {
    let mut waves: Vec<Vec<usize>> = Vec::new();
    let mut bytes = 0;
    for (index, file) in uploads.iter().enumerate() {
        let size = u64::from(file.size);
        match waves.last_mut() {
            Some(wave) if wave.len() < PARALLEL_FILES && bytes + size <= PARALLEL_BYTES => {
                wave.push(index);
                bytes += size;
            }
            _ => {
                waves.push(vec![index]);
                bytes = size;
            }
        }
    }
    waves
}

/// Uploads the files at `wave` together, as Steam's client does: their
/// begins, then their bodies, then their commits. Between the begin replies
/// and the commits, the journal lists the wave as staged.
async fn upload_wave(
    transport: &mut impl Transport,
    job: &mut UploadTransaction,
    batch_id: u64,
    wave: &[usize],
    authorize: &impl Fn() -> bool,
) -> Result<(), CloudError> {
    let mut files = Vec::with_capacity(wave.len());
    for &index in wave {
        let file = job.state.uploads[index].clone();
        let bytes = job.content(index, &file)?;
        let sha = Sha1::digest(&bytes).to_vec();
        files.push((file, bytes, sha));
    }
    let begins: Vec<_> = files
        .iter()
        .map(|(file, _, sha)| wire::BeginUploadRequest {
            app_id: job.app_id(),
            file_size: file.size,
            raw_size: file.size,
            sha: sha.clone(),
            timestamp: file.timestamp,
            name: file.name.clone(),
            platforms: file.platforms,
            can_encrypt: false,
            batch_id,
        })
        .collect();
    check(authorize)?;
    job.save_progress(job.phase().clone(), true)?;
    let plans: Vec<wire::BeginUploadResponse> = transport
        .calls("Cloud.ClientBeginFileUpload#1", &begins)
        .await?;
    job.save_file_progress(false, wave.to_vec())?;
    // Validate every block before sending the first. Each file's blocks go
    // in sequence, preserving control requests around multipart data blocks.
    for (plan, (_, bytes, _)) in plans.iter().zip(&files) {
        if plan.encrypted || plan.blocks.len() > 4096 {
            return Err(CloudError::Invalid("upload format"));
        }
        for block in &plan.blocks {
            block_request(block, bytes)?;
        }
    }
    let bodies: Vec<_> = plans
        .iter()
        .zip(&files)
        .map(|(plan, (_, bytes, _))| (plan.blocks.as_slice(), bytes.as_slice()))
        .collect();
    check(authorize)?;
    transport.bodies(&bodies).await?;
    let commits: Vec<_> = files
        .iter()
        .map(|(file, _, sha)| wire::CommitUploadRequest {
            succeeded: true,
            app_id: job.app_id(),
            sha: sha.clone(),
            name: file.name.clone(),
        })
        .collect();
    check(authorize)?;
    job.save_progress(job.phase().clone(), true)?;
    let receipts = transport.commits(&commits).await?;
    // A missing result is malformed, not a negative acknowledgement.
    if receipts.iter().any(|receipt| receipt.committed.is_none()) {
        return Err(CloudError::Invalid("file commit result"));
    }
    job.save_file_progress(false, Vec::new())?;
    if receipts
        .iter()
        .any(|receipt| receipt.committed != Some(true))
    {
        return Err(CloudError::NotCommitted);
    }
    Ok(())
}

async fn close_batch(
    transport: &mut impl Transport,
    job: &mut UploadTransaction,
    batch_id: u64,
) -> Result<(), CloudError> {
    let _: wire::EmptyResponse = transport
        .call(
            "Cloud.CompleteAppUploadBatchBlocking#1",
            &wire::CompleteBatchRequest {
                app_id: job.app_id(),
                batch_id,
                result: 2,
            },
        )
        .await?;
    job.save(UploadPhase::Closed { batch_id })
}

/// Report failed transfers only when the durable journal proves that their
/// file commits were never sent. A lost abort reply is uncertain and is never
/// replayed.
async fn abort_staged_uploads(
    transport: &mut impl Transport,
    job: &mut UploadTransaction,
    authorize: &impl Fn() -> bool,
) -> Result<(), CloudError> {
    if job.state.unanswered_mutation {
        return Err(CloudError::Uncertain);
    }
    if job.state.staged_uploads.is_empty() {
        return Ok(());
    }
    check(authorize)?;
    let mut aborts = Vec::with_capacity(job.state.staged_uploads.len());
    for &index in &job.state.staged_uploads {
        let file = &job.state.uploads[index];
        aborts.push(wire::CommitUploadRequest {
            succeeded: false,
            app_id: job.app_id(),
            sha: Sha1::digest(job.content(index, file)?).to_vec(),
            name: file.name.clone(),
        });
    }
    job.save_progress(job.phase().clone(), true)?;
    let receipts = transport.commits(&aborts).await?;
    if receipts
        .iter()
        .any(|receipt| receipt.committed != Some(false))
    {
        return Err(CloudError::Invalid("failed transfer receipt"));
    }
    job.save_file_progress(false, Vec::new())
}

/// Reports unfinished HTTP files as failed, then closes the known batch. No
/// file request is resent. A lost Begin reply or an unanswered commit/deletion
/// remains blocked, even if Steam previously acknowledged batch closure.
pub(crate) async fn settle(
    transport: &mut impl Transport,
    job: &mut UploadTransaction,
    authorize: &impl Fn() -> bool,
) -> Result<(), CloudError> {
    if job.poisoned {
        return Err(CloudError::Storage);
    }
    check(authorize)?;
    if job.is_settled() {
        return Ok(());
    }
    if job.state.unanswered_mutation {
        return Err(CloudError::Uncertain);
    }
    let batch_id = match job.phase() {
        UploadPhase::Open { batch_id }
        | UploadPhase::Completing { batch_id }
        | UploadPhase::Interrupted {
            batch_id: Some(batch_id),
        } => *batch_id,
        _ => return Err(CloudError::Uncertain),
    };
    abort_staged_uploads(transport, job, authorize).await?;
    check(authorize)?;
    job.save(UploadPhase::Interrupted {
        batch_id: Some(batch_id),
    })?;
    close_batch(transport, job, batch_id).await
}

pub(crate) fn block_request(
    block: &wire::UploadBlock,
    bytes: &[u8],
) -> Result<reqwest::Request, CloudError> {
    let url = read::download_url(&wire::DownloadResponse {
        host: block.host.clone(),
        path: block.path.clone(),
        https: block.https,
        ..Default::default()
    })?;
    let method = match block.method {
        1 => reqwest::Method::GET,
        2 => reqwest::Method::HEAD,
        3 => reqwest::Method::POST,
        4 => reqwest::Method::PUT,
        5 => reqwest::Method::DELETE,
        6 => reqwest::Method::OPTIONS,
        7 => reqwest::Method::PATCH,
        _ => return Err(CloudError::Invalid("upload HTTP method")),
    };
    let body = match &block.body {
        Some(body) if body.len() <= MAX_FILE => body.as_slice(),
        Some(_) => return Err(CloudError::Invalid("upload explicit body size")),
        None => {
            let start =
                usize::try_from(block.offset).map_err(|_| CloudError::Invalid("upload offset"))?;
            let end = start
                .checked_add(block.length as usize)
                .ok_or(CloudError::Invalid("upload length"))?;
            bytes
                .get(start..end)
                .ok_or(CloudError::Invalid("upload range"))?
        }
    };
    if (method == reqwest::Method::GET || method == reqwest::Method::HEAD) && !body.is_empty() {
        return Err(CloudError::Invalid("upload method body"));
    }
    let mut request = reqwest::Request::new(method, url);
    for header in &block.headers {
        let name = reqwest::header::HeaderName::from_bytes(header.name.as_bytes())
            .map_err(|_| CloudError::Invalid("upload header"))?;
        if name == reqwest::header::HOST {
            if !header.value.eq_ignore_ascii_case(&block.host) {
                return Err(CloudError::Invalid("upload Host mismatch"));
            }
            continue;
        }
        if name == reqwest::header::CONTENT_LENGTH {
            if header.value.parse::<usize>().ok() != Some(body.len()) {
                return Err(CloudError::Invalid("upload Content-Length"));
            }
            continue;
        }
        if [
            reqwest::header::PROXY_AUTHORIZATION,
            reqwest::header::TRANSFER_ENCODING,
            reqwest::header::CONNECTION,
            reqwest::header::COOKIE,
        ]
        .contains(&name)
        {
            return Err(CloudError::Invalid("upload header"));
        }
        let mut value = reqwest::header::HeaderValue::from_str(&header.value)
            .map_err(|_| CloudError::Invalid("upload header"))?;
        value.set_sensitive(true);
        request.headers_mut().append(name, value);
    }
    *request.body_mut() = Some(body.to_vec().into());
    Ok(request)
}

#[cfg(test)]
mod tests;
