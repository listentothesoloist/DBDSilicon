//! Runtimes through their interface, against runtimes the test stands in
//! for.
use super::*;
use crate::{epic::Epic, gog::Gog, installations::tests::eventually, steam::Steam};
use std::sync::{
    Mutex,
    atomic::{AtomicBool, Ordering},
};

/// What `FakeRuntimes` does, shared with each of its downloads.
struct FakeRuntime {
    present: Vec<Runtime>,
    /// The next download's failure.
    failure: Option<Failure>,
    /// Downloads wait until they are cancelled, or until the test releases
    /// them.
    held: Option<CancelToken>,
    record: Arc<dyn Fn(String) + Send + Sync>,
}

/// Runtimes a test stands in for. Those `present` are in place; a download
/// of another records its name through `record`, then waits once `hold` is
/// called until it is cancelled or `release` lets it go on, then fails as
/// `fail` says, or else puts its runtime in place. Removing the replaced
/// releases records "remove replaced".
pub(crate) struct FakeRuntimes(Arc<Mutex<FakeRuntime>>);

impl FakeRuntimes {
    pub(crate) fn new(
        present: &[Runtime],
        record: impl Fn(String) + Send + Sync + 'static,
    ) -> Arc<Self> {
        Arc::new(Self(Arc::new(Mutex::new(FakeRuntime {
            present: present.to_vec(),
            failure: None,
            held: None,
            record: Arc::new(record),
        }))))
    }
    pub(crate) fn fail(&self, failure: Failure) {
        self.0.lock().unwrap().failure = Some(failure);
    }
    pub(crate) fn hold(&self) {
        self.0.lock().unwrap().held = Some(CancelToken::new());
    }
    /// The downloads held go on, and the next ones no longer wait.
    pub(crate) fn release(&self) {
        if let Some(released) = self.0.lock().unwrap().held.take() {
            released.cancel();
        }
    }
    /// The runtimes in place now, in the order they came.
    pub(crate) fn in_place(&self) -> Vec<Runtime> {
        self.0.lock().unwrap().present.clone()
    }
}

/// Runtimes whose every runtime is in place.
pub(crate) fn ready() -> Arc<Runtimes> {
    Runtimes::from_source(FakeRuntimes::new(
        &[Runtime::SteamComponents, Runtime::Voile],
        |_| {},
    ))
}

pub(crate) fn runtime_name(runtime: Runtime) -> String {
    match runtime {
        Runtime::Voile => "voile".into(),
        Runtime::SteamComponents => "steam components".into(),
    }
}

impl RuntimeSource for FakeRuntimes {
    fn present(&self, runtime: Runtime) -> bool {
        self.0.lock().unwrap().present.contains(&runtime)
    }
    fn download(&self, runtime: Runtime) -> Arc<dyn RuntimeDownload> {
        Arc::new(FakeDownload {
            runtime,
            state: self.0.clone(),
            cancel: CancelToken::new(),
        })
    }
    fn remove_replaced(&self) {
        let record = self.0.lock().unwrap().record.clone();
        record("remove replaced".into());
    }
}

struct FakeDownload {
    runtime: Runtime,
    state: Arc<Mutex<FakeRuntime>>,
    cancel: CancelToken,
}

impl RuntimeDownload for FakeDownload {
    fn progress(&self) -> (u64, u64) {
        (5, 10)
    }
    fn cancel(&self) {
        self.cancel.cancel();
    }
    fn run(&self) -> BoxFuture<'_, Result<(), Failure>> {
        Box::pin(async move {
            let (record, held) = {
                let state = self.state.lock().unwrap();
                (state.record.clone(), state.held.clone())
            };
            record(runtime_name(self.runtime));
            if let Some(released) = held {
                tokio::select! {
                    biased;
                    () = self.cancel.cancelled() => return Err(Failure::Cancelled),
                    () = released.cancelled() => {}
                }
            }
            let mut state = self.state.lock().unwrap();
            if let Some(failure) = state.failure.take() {
                return Err(failure);
            }
            state.present.push(self.runtime);
            Ok(())
        })
    }
}

pub(crate) fn recorded() -> (
    Arc<Mutex<Vec<String>>>,
    impl Fn(String) + Send + Sync + 'static,
) {
    let events = Arc::new(Mutex::new(Vec::new()));
    let recording = events.clone();
    (events, move |event| recording.lock().unwrap().push(event))
}

#[tokio::test]
async fn a_build_gets_the_runtimes_its_storefront_and_platform_need_in_order() {
    /// What a build of `platform` from P downloads with nothing in place.
    async fn downloads<P: sessions::Storefront>(platform: Platform) -> Vec<String> {
        let (events, record) = recorded();
        let runtimes = Runtimes::from_source(FakeRuntimes::new(&[], record));
        runtimes
            .get::<P>(platform, &CancelToken::new(), |_| {})
            .await
            .unwrap();
        events.lock().unwrap().clone()
    }

    assert_eq!(
        downloads::<Steam>(Platform::Windows).await,
        ["steam components", "voile"]
    );
    assert_eq!(
        downloads::<Steam>(Platform::Mac).await,
        ["steam components"]
    );
    assert_eq!(downloads::<Epic>(Platform::Windows).await, ["voile"]);
    assert_eq!(downloads::<Epic>(Platform::Mac).await, Vec::<String>::new());
    assert_eq!(downloads::<Gog>(Platform::Windows).await, ["voile"]);
    assert_eq!(downloads::<Gog>(Platform::Mac).await, Vec::<String>::new());
}

#[tokio::test]
async fn runtimes_in_place_are_not_downloaded_again() {
    let (events, record) = recorded();
    let fake = FakeRuntimes::new(&[Runtime::Voile], record);
    let runtimes = Runtimes::from_source(fake.clone());
    let cancel = CancelToken::new();

    runtimes
        .get::<Steam>(Platform::Windows, &cancel, |_| {})
        .await
        .unwrap();
    runtimes
        .get::<Steam>(Platform::Windows, &cancel, |_| {})
        .await
        .unwrap();

    assert_eq!(*events.lock().unwrap(), ["steam components"]);
    assert_eq!(fake.in_place(), [Runtime::Voile, Runtime::SteamComponents]);
}

#[tokio::test]
async fn a_download_shows_its_runtime_and_bytes_and_cancelling_stops_it() {
    let (events, record) = recorded();
    let fake = FakeRuntimes::new(&[], record);
    fake.hold();
    let runtimes = Runtimes::from_source(fake.clone());
    let cancel = CancelToken::new();
    let shown = Arc::new(Mutex::new(Vec::new()));
    let getting = {
        let (runtimes, cancel, shown) = (runtimes.clone(), cancel.clone(), shown.clone());
        tokio::spawn(async move {
            runtimes
                .get::<Epic>(Platform::Windows, &cancel, |progress| {
                    shown.lock().unwrap().push(progress)
                })
                .await
        })
    };
    eventually(|| !shown.lock().unwrap().is_empty()).await;
    assert_eq!(
        shown.lock().unwrap()[0],
        Progress {
            runtime: Runtime::Voile,
            received: 5,
            total: 10
        }
    );

    cancel.cancel();

    assert_eq!(getting.await.unwrap(), Err(Failure::Cancelled));
    assert!(fake.in_place().is_empty());
    // The same token gets nothing more; a fresh one downloads again.
    assert_eq!(
        runtimes
            .get::<Epic>(Platform::Windows, &cancel, |_| {})
            .await,
        Err(Failure::Cancelled)
    );
    assert_eq!(*events.lock().unwrap(), ["voile"]);
    fake.release();
    runtimes
        .get::<Epic>(Platform::Windows, &CancelToken::new(), |_| {})
        .await
        .unwrap();
    assert_eq!(*events.lock().unwrap(), ["voile", "voile"]);
    assert_eq!(fake.in_place(), [Runtime::Voile]);
}

#[tokio::test]
async fn a_failed_download_ends_the_wait_with_its_own_failure_before_the_next_runtime() {
    let (events, record) = recorded();
    let fake = FakeRuntimes::new(&[], record);
    let failure = Failure::Failed("backend.steam.components.downloadFailed".into());
    fake.fail(failure.clone());
    let runtimes = Runtimes::from_source(fake);

    let got = runtimes
        .get::<Steam>(Platform::Windows, &CancelToken::new(), |_| {})
        .await;

    assert_eq!(got, Err(failure));
    assert_eq!(*events.lock().unwrap(), ["steam components"]);
}

#[tokio::test]
async fn a_transfer_keeps_the_voile_releases_the_pinned_one_replaced() {
    let (events, record) = recorded();
    let runtimes = Runtimes::from_source(FakeRuntimes::new(&[], record));

    for _ in 0..2 {
        runtimes
            .get::<Epic>(Platform::Windows, &CancelToken::new(), |_| {})
            .await
            .unwrap();
    }

    assert_eq!(*events.lock().unwrap(), ["voile"]);
}

#[tokio::test]
async fn a_launch_removes_the_replaced_voile_releases_once_no_game_may_run_from_one() {
    let (events, record) = recorded();
    let runtimes = Runtimes::from_source(FakeRuntimes::new(&[], record));
    let running = Arc::new(AtomicBool::new(true));
    let watched = running.clone();
    runtimes.watch_sessions(move || watched.load(Ordering::Acquire));
    runtimes.watch_sessions(|| false);
    let launch = |platform: Platform| {
        let runtimes = runtimes.clone();
        async move {
            runtimes
                .get_for_launch::<Epic>(platform, &CancelToken::new(), |_| {})
                .await
                .unwrap()
        }
    };

    launch(Platform::Windows).await;
    assert_eq!(*events.lock().unwrap(), ["voile"], "a game may run");

    running.store(false, Ordering::Release);
    // Voile is in place now: the releases go all the same.
    launch(Platform::Windows).await;
    assert_eq!(*events.lock().unwrap(), ["voile", "remove replaced"]);

    launch(Platform::Mac).await;
    assert_eq!(
        *events.lock().unwrap(),
        ["voile", "remove replaced"],
        "a Mac build needs no Voile"
    );
}

#[tokio::test]
async fn a_failed_or_cancelled_launch_removes_nothing() {
    let (events, record) = recorded();
    let fake = FakeRuntimes::new(&[], record);
    fake.fail(Failure::Failed("backend.voile.downloadFailed".into()));
    let runtimes = Runtimes::from_source(fake);

    let failed = runtimes
        .get_for_launch::<Gog>(Platform::Windows, &CancelToken::new(), |_| {})
        .await;
    let cancel = CancelToken::new();
    cancel.cancel();
    let cancelled = runtimes
        .get_for_launch::<Gog>(Platform::Windows, &cancel, |_| {})
        .await;

    assert_eq!(
        failed,
        Err(Failure::Failed("backend.voile.downloadFailed".into()))
    );
    assert_eq!(cancelled, Err(Failure::Cancelled));
    assert_eq!(*events.lock().unwrap(), ["voile"]);
}
