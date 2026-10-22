//! The runtimes a game needs beside its own files: Voile for a build that
//! runs in Wine, Valve's Steam components for every Steam game. A Transfer
//! gets the missing ones before the game's files. A launch that still misses
//! one gets it after its Admission, before its game is prepared. Each shows
//! the download its own way. A Voile release the pinned one replaced goes
//! only when a launch finds no Game session, of any storefront, that may run
//! a game from it.
use crate::{
    installations::blocking,
    sessions::{self, BoxFuture, Failure, failed},
};
use obelisk_core::{Platform, transfer::CancelToken};
use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};

/// How often a download under way shows its progress.
const PACE: Duration = Duration::from_millis(250);

/// A runtime a build needs to launch, beside its own files.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Runtime {
    /// Voile, the Wine runtime Windows games run in.
    Voile,
    /// Valve's Steam components, which Obelisk's Steam host loads.
    SteamComponents,
}

/// How much of a runtime has downloaded: the bytes received, and the total.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Progress {
    pub runtime: Runtime,
    pub received: u64,
    pub total: u64,
}

/// One runtime's download, which the one waiting for it runs, cancels and
/// watches.
pub(crate) trait RuntimeDownload: Send + Sync {
    /// The bytes received so far, and the total.
    fn progress(&self) -> (u64, u64);
    fn cancel(&self);
    /// A failure is the runtime's own localization key.
    fn run(&self) -> BoxFuture<'_, Result<(), Failure>>;
}

/// Where the runtimes are: whether one is in place, its download, and the
/// Voile releases the pinned one replaced. Tests stand in for it.
pub(crate) trait RuntimeSource: Send + Sync {
    /// It reads a few files, never a whole runtime.
    fn present(&self, runtime: Runtime) -> bool;
    fn download(&self, runtime: Runtime) -> Arc<dyn RuntimeDownload>;
    /// Removes the Voile releases the pinned one replaced. What cannot go now
    /// goes with a later launch.
    fn remove_replaced(&self);
}

/// The process's runtimes, which every storefront's Transfers and launches
/// share.
pub struct Runtimes {
    source: Arc<dyn RuntimeSource>,
    /// Whether each storefront's Game sessions may run a game now.
    sessions: Mutex<Vec<Box<dyn Fn() -> bool + Send + Sync>>>,
}

impl Runtimes {
    /// Gets the runtimes into the places `locations` names, where launches
    /// read them.
    pub fn new(locations: &crate::Locations) -> Arc<Self> {
        Self::from_source(Arc::new(Downloads(locations.clone())))
    }

    pub(crate) fn from_source(source: Arc<dyn RuntimeSource>) -> Arc<Self> {
        Arc::new(Self {
            source,
            sessions: Mutex::new(Vec::new()),
        })
    }

    /// Counts a storefront's Game sessions, which `may_run` tells about,
    /// before a launch removes a replaced release.
    pub(crate) fn watch_sessions(&self, may_run: impl Fn() -> bool + Send + Sync + 'static) {
        self.sessions.lock().unwrap().push(Box::new(may_run));
    }

    /// Gets each runtime a build of `platform` from P needs that is not in
    /// place, in order. `shown` gets each download's progress when it starts
    /// and every `PACE`. Cancelling `cancel` stops the download under way and
    /// any after it. Never removes a release.
    pub(crate) async fn get<P: sessions::Storefront>(
        &self,
        platform: Platform,
        cancel: &CancelToken,
        shown: impl Fn(Progress) + Send,
    ) -> Result<(), Failure> {
        for runtime in needed::<P>(platform) {
            let source = self.source.clone();
            if blocking(storage_failed(runtime), move || source.present(runtime)).await? {
                continue;
            }
            if cancel.is_cancelled() {
                return Err(Failure::Cancelled);
            }
            let download = self.source.download(runtime);
            let mut run = download.run();
            let outcome = loop {
                let (received, total) = download.progress();
                shown(Progress {
                    runtime,
                    received,
                    total,
                });
                tokio::select! {
                    outcome = &mut run => break outcome,
                    () = cancel.cancelled() => {
                        download.cancel();
                        // The install cleans up before the wait ends.
                        break (&mut run).await;
                    }
                    () = tokio::time::sleep(PACE) => {}
                }
            };
            match outcome {
                Err(_) if cancel.is_cancelled() => return Err(Failure::Cancelled),
                outcome => outcome?,
            }
        }
        match cancel.is_cancelled() {
            true => Err(Failure::Cancelled),
            false => Ok(()),
        }
    }

    /// Gets the runtimes as `get` does. Then, for a build that needs Voile,
    /// removes the releases the pinned one replaced, unless the Game sessions
    /// of a storefront may run a game. A failed or cancelled launch removes
    /// nothing.
    pub(crate) async fn get_for_launch<P: sessions::Storefront>(
        &self,
        platform: Platform,
        cancel: &CancelToken,
        shown: impl Fn(Progress) + Send,
    ) -> Result<(), Failure> {
        self.get::<P>(platform, cancel, shown).await?;
        if needed::<P>(platform).contains(&Runtime::Voile) && !self.may_run() {
            let source = self.source.clone();
            let _ = tokio::task::spawn_blocking(move || source.remove_replaced()).await;
        }
        Ok(())
    }

    /// Whether the Game sessions of a storefront may run a game now.
    fn may_run(&self) -> bool {
        self.sessions
            .lock()
            .unwrap()
            .iter()
            .any(|may_run| may_run())
    }
}

/// The runtimes a build of `platform` from P needs, in the order they are
/// got: the storefront's own, then Voile for a build that runs in Wine.
fn needed<P: sessions::Storefront>(platform: Platform) -> Vec<Runtime> {
    let mut needed = P::RUNTIMES.to_vec();
    if platform == Platform::Windows {
        needed.push(Runtime::Voile);
    }
    needed
}

fn storage_failed(runtime: Runtime) -> String {
    match runtime {
        Runtime::Voile => crate::voile::Failure::Storage.to_string(),
        Runtime::SteamComponents => crate::valve::Failure::Storage.to_string(),
    }
}

/// The runtimes Obelisk downloads, in the places its launches read them.
struct Downloads(crate::Locations);

impl RuntimeSource for Downloads {
    fn present(&self, runtime: Runtime) -> bool {
        match runtime {
            Runtime::Voile => crate::voile::runtime(&self.0.voile).is_some(),
            Runtime::SteamComponents => crate::valve::installed(&self.0.steam_runtime_package),
        }
    }
    fn download(&self, runtime: Runtime) -> Arc<dyn RuntimeDownload> {
        match runtime {
            Runtime::Voile => Arc::new(Download {
                install: crate::voile::Install::new(),
                path: self.0.voile.clone(),
            }),
            Runtime::SteamComponents => Arc::new(Download {
                install: crate::valve::Install::new(),
                path: self.0.steam_runtime_package.clone(),
            }),
        }
    }
    fn remove_replaced(&self) {
        crate::voile::remove_replaced(&self.0.voile);
    }
}

/// One runtime's install into `path`.
struct Download<I> {
    install: I,
    path: PathBuf,
}

impl RuntimeDownload for Download<crate::voile::Install> {
    fn progress(&self) -> (u64, u64) {
        self.install.progress()
    }
    fn cancel(&self) {
        self.install.cancel();
    }
    fn run(&self) -> BoxFuture<'_, Result<(), Failure>> {
        Box::pin(async move {
            let archive = development_archive();
            self.install
                .run(&self.path, archive.as_deref())
                .await
                .map(drop)
                .map_err(Into::into)
        })
    }
}

impl RuntimeDownload for Download<crate::valve::Install> {
    fn progress(&self) -> (u64, u64) {
        self.install.progress()
    }
    fn cancel(&self) {
        self.install.cancel();
    }
    fn run(&self) -> BoxFuture<'_, Result<(), Failure>> {
        Box::pin(async move { self.install.run(&self.path).await.map_err(Into::into) })
    }
}

/// A local copy of the pinned Voile release, which replaces its download in
/// development builds while the release is private.
fn development_archive() -> Option<PathBuf> {
    if !cfg!(debug_assertions) {
        return None;
    }
    std::env::var_os("OBELISK_VOILE_ARCHIVE")
        .filter(|archive| !archive.is_empty())
        .map(Into::into)
}

impl From<crate::voile::Failure> for Failure {
    fn from(failure: crate::voile::Failure) -> Self {
        match failure {
            crate::voile::Failure::Cancelled => Self::Cancelled,
            failure => failed(failure),
        }
    }
}

impl From<crate::valve::Failure> for Failure {
    fn from(failure: crate::valve::Failure) -> Self {
        match failure {
            crate::valve::Failure::Cancelled => Self::Cancelled,
            failure => failed(failure),
        }
    }
}

#[cfg(test)]
pub(crate) mod tests;
