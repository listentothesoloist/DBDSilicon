//! Voile, the Wine runtime Windows games run in. Obelisk downloads the release
//! it pins with a Windows build's Transfer, before the game's files, and a
//! launch that still misses it downloads it first; a development runtime that
//! voile's install-dev.sh put in `current` takes precedence.
//!
//! The release also carries the Linux engine's runtime, in its `linux` folder.
use obelisk_core::transfer::CancelToken;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{
    fs,
    future::Future,
    io::Write,
    path::{Path, PathBuf},
    sync::{
        OnceLock,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

const RELEASE: &str = include_str!("../voile-release.json");
/// Where install-dev.sh puts a locally built runtime.
const DEVELOPMENT: &str = "current";
/// What every release archive describes itself with.
const MANIFEST: &str = "runtime.json";
/// Unfinished work in the runtimes folder starts with this, and the next
/// install removes it.
const UNFINISHED: &str = ".";
/// The Linux engine's runtime, inside a release (or the development runtime).
const LINUX: &str = "linux";

/// Two installs never extract into the same folder.
static INSTALLING: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// The Voile release Obelisk runs Windows games with.
#[derive(Debug, Deserialize)]
pub struct Release {
    pub version: String,
    pub url: String,
    pub size: u64,
    pub sha256: String,
}

pub fn release() -> &'static Release {
    static VALUE: OnceLock<Release> = OnceLock::new();
    VALUE.get_or_init(|| serde_json::from_str(RELEASE).expect("checked-in Voile release"))
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum Failure {
    #[error("backend.voile.downloadFailed")]
    Download,
    #[error("backend.voile.damaged")]
    Damaged,
    #[error("backend.voile.storageFailed")]
    Storage,
    #[error("backend.voile.cancelled")]
    Cancelled,
}
impl From<std::io::Error> for Failure {
    fn from(_: std::io::Error) -> Self {
        Self::Storage
    }
}

/// The runtime in `root` Windows games run in: the development runtime, else
/// the pinned release once installed.
pub fn runtime(root: &Path) -> Option<PathBuf> {
    located(root, release())
}

fn located(root: &Path, release: &Release) -> Option<PathBuf> {
    let development = root.join(DEVELOPMENT);
    if has_wine(&development) {
        return Some(development);
    }
    let pinned = root.join(&release.version);
    (has_wine(&pinned) && version(&pinned).as_deref() == Some(release.version.as_str()))
        .then_some(pinned)
}

fn has_wine(runtime: &Path) -> bool {
    runtime.join("bin/wine").is_file()
}

/// The Linux engine's runtime in `root`: the development runtime's, else the
/// pinned release's once installed.
pub fn linux_runtime(root: &Path) -> Option<PathBuf> {
    located_linux(root, release())
}

fn located_linux(root: &Path, release: &Release) -> Option<PathBuf> {
    let development = root.join(DEVELOPMENT).join(LINUX);
    if has_engine(&development) {
        return Some(development);
    }
    let pinned = root.join(&release.version);
    let linux = pinned.join(LINUX);
    (has_wine(&pinned)
        && version(&pinned).as_deref() == Some(release.version.as_str())
        && has_engine(&linux))
    .then_some(linux)
}

/// The engine and the Wine it runs.
fn has_engine(runtime: &Path) -> bool {
    runtime.join("engine/fexdarwin-run").is_file() && runtime.join("wine/bin/wine").exists()
}

/// The Voile release Windows games run in now, as its runtime.json names it:
/// the development runtime's, else the pinned release once installed.
pub fn installed_release(root: &Path) -> Option<String> {
    installed(root, release())
}

fn installed(root: &Path, release: &Release) -> Option<String> {
    release_of(&located(root, release)?)
}

/// The Voile release `runtime` comes from: a Rosetta runtime, or the Linux
/// engine's runtime inside one.
pub(crate) fn release_of(runtime: &Path) -> Option<String> {
    version(runtime).or_else(|| version(runtime.parent()?))
}

fn version(runtime: &Path) -> Option<String> {
    #[derive(Deserialize)]
    struct Manifest {
        version: String,
    }
    let manifest = fs::read(runtime.join(MANIFEST)).ok()?;
    serde_json::from_slice::<Manifest>(&manifest)
        .ok()
        .map(|manifest| manifest.version)
}

/// One install of the pinned release, which the player can cancel and watch.
#[derive(Default)]
pub struct Install {
    received: AtomicU64,
    /// The size of the archive being installed, once known.
    total: AtomicU64,
    cancel: CancelToken,
}

impl Install {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cancel(&self) {
        self.cancel.cancel();
    }

    /// The bytes downloaded so far, and the archive's size.
    pub fn progress(&self) -> (u64, u64) {
        let total = self.total.load(Ordering::Acquire);
        let total = if total == 0 { release().size } else { total };
        (self.received.load(Ordering::Acquire), total)
    }

    /// Installs the pinned release in `root` unless a runtime is already
    /// there. It keeps the releases it replaces: a game may run from one, and
    /// `runtimes` decides when they go. `archive`, a local copy of the
    /// release, stands in for the download while the release is private.
    pub async fn run(&self, root: &Path, archive: Option<&Path>) -> Result<PathBuf, Failure> {
        self.install(root, release(), archive).await
    }

    async fn install(
        &self,
        root: &Path,
        release: &Release,
        archive: Option<&Path>,
    ) -> Result<PathBuf, Failure> {
        let _installing = self.cancellable(INSTALLING.lock()).await?;
        if let Some(runtime) = located(root, release) {
            return Ok(runtime);
        }
        let extracted = self.extract(root, release, archive).await?;
        let runtime = extracted.path().join(format!("voile-{}", release.version));
        if !has_wine(&runtime) || version(&runtime).as_deref() != Some(release.version.as_str()) {
            return Err(Failure::Damaged);
        }
        let (root, version) = (root.to_owned(), release.version.clone());
        tokio::task::spawn_blocking(move || {
            let destination = root.join(&version);
            if destination.exists() {
                fs::remove_dir_all(&destination)?;
            }
            fs::rename(&runtime, &destination)?;
            Ok(destination)
        })
        .await
        .map_err(|_| Failure::Storage)?
    }

    /// Checks `archive` (or downloads it) and extracts it into a folder of
    /// `root` that goes away unless the caller moves what it holds.
    async fn extract(
        &self,
        root: &Path,
        expected: &Release,
        archive: Option<&Path>,
    ) -> Result<tempfile::TempDir, Failure> {
        self.total.store(expected.size, Ordering::Release);
        let unfinished = root.to_owned();
        tokio::task::spawn_blocking(move || {
            fs::create_dir_all(&unfinished)?;
            remove_entries(&unfinished, |name| name.starts_with(UNFINISHED));
            Ok::<_, Failure>(())
        })
        .await
        .map_err(|_| Failure::Storage)??;
        let downloaded;
        let archive = match archive {
            Some(archive) => {
                let (path, expected_sha) = (archive.to_owned(), expected.sha256.clone());
                let matches = tokio::task::spawn_blocking(move || {
                    crate::sha(&path).is_ok_and(|sha| sha == expected_sha)
                })
                .await
                .map_err(|_| Failure::Storage)?;
                if !matches {
                    return Err(Failure::Damaged);
                }
                self.received.store(expected.size, Ordering::Release);
                archive
            }
            None => {
                downloaded = self.download(root, expected).await?;
                &*downloaded
            }
        };
        let extracted = tempfile::Builder::new()
            .prefix(UNFINISHED)
            .tempdir_in(root)?;
        let mut tar = tokio::process::Command::new("/usr/bin/tar")
            .arg("-xJf")
            .arg(archive)
            .arg("-C")
            .arg(extracted.path())
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true)
            .spawn()?;
        if !self.cancellable(tar.wait()).await??.success() {
            return Err(Failure::Damaged);
        }
        Ok(extracted)
    }

    async fn download(
        &self,
        root: &Path,
        release: &Release,
    ) -> Result<tempfile::TempPath, Failure> {
        let client = reqwest::Client::builder()
            .https_only(true)
            .connect_timeout(Duration::from_secs(20))
            .read_timeout(Duration::from_secs(60))
            .build()
            .map_err(|_| Failure::Download)?;
        let mut response = self
            .cancellable(client.get(&release.url).send())
            .await?
            .and_then(reqwest::Response::error_for_status)
            .map_err(|_| Failure::Download)?;
        let mut file = tempfile::Builder::new()
            .prefix(UNFINISHED)
            .tempfile_in(root)?;
        let mut digest = Sha256::new();
        let mut size = 0_u64;
        while let Some(bytes) = self
            .cancellable(response.chunk())
            .await?
            .map_err(|_| Failure::Download)?
        {
            size += bytes.len() as u64;
            if size > release.size {
                return Err(Failure::Damaged);
            }
            digest.update(&bytes);
            file.write_all(&bytes)?;
            self.received.store(size, Ordering::Release);
        }
        if size != release.size || format!("{:x}", digest.finalize()) != release.sha256 {
            return Err(Failure::Damaged);
        }
        Ok(file.into_temp_path())
    }

    /// Stops waiting for `work` as soon as the install is cancelled.
    async fn cancellable<T>(&self, work: impl Future<Output = T>) -> Result<T, Failure> {
        self.cancel.race(work).await.map_err(|_| Failure::Cancelled)
    }
}

/// Removes the releases in `root` the pinned one replaces. `runtimes` calls
/// it once a launch finds no Game session, of any storefront, that may run a
/// game from one.
pub fn remove_replaced(root: &Path) {
    remove_replaced_by(root, &release().version);
}

/// Removes every release in `root` but `version`, leaving the development
/// runtime and unfinished installs.
fn remove_replaced_by(root: &Path, version: &str) {
    remove_entries(root, |name| {
        name != DEVELOPMENT && name != version && !name.starts_with(UNFINISHED)
    });
}

/// Removes what `root` holds under the names `remove` picks. Whatever cannot be
/// removed now goes with a later install.
pub(crate) fn remove_entries(root: &Path, remove: impl Fn(&str) -> bool) {
    let Ok(entries) = fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        if !remove(&name.to_string_lossy()) {
            continue;
        }
        let path = entry.path();
        let _ = match entry.file_type() {
            Ok(kind) if kind.is_dir() => fs::remove_dir_all(path),
            _ => fs::remove_file(path),
        };
    }
}

#[cfg(test)]
mod tests;
