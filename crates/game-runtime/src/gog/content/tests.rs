//! GOG's part of the installations through their Content interface, with a
//! network the test stands in for and real folders in a temporary library.
use super::*;
use crate::{
    installations::{Content as _, Installations, Offer, Queueing, tests::eventually},
    queue::DownloadQueue,
    runtimes::{
        Runtimes,
        tests::{FakeRuntimes, ready, recorded},
    },
    sessions::Sessions,
};
use std::sync::Mutex;
use tokio::sync::Notify;

const ACCOUNT: &str = "48628349957132247";

/// What the test's GOG network was asked, in order.
#[derive(Default)]
struct Fake {
    calls: Mutex<Vec<String>>,
    /// Renewing credentials waits until the test releases it.
    held: Mutex<bool>,
    released: Notify,
    /// The account signed out: asking for it fails.
    signed_out: Mutex<bool>,
}

impl Fake {
    fn call(&self, call: &str) {
        self.calls.lock().unwrap().push(call.into());
    }
    fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }
}

impl Network for Fake {
    fn prepare(&self) -> BoxFuture<'_, Result<(), Failure>> {
        Box::pin(async move {
            self.call("prepare");
            if *self.held.lock().unwrap() {
                self.released.notified().await;
            }
            Ok(())
        })
    }
    fn account_id(&self) -> BoxFuture<'_, Result<String, Failure>> {
        Box::pin(async move {
            self.call("account");
            match *self.signed_out.lock().unwrap() {
                true => Err(Failure::Failed("backend.gog.unauthorized".into())),
                false => Ok(ACCOUNT.into()),
            }
        })
    }
    fn platform<'a>(
        &'a self,
        _: &'a Product,
        language: &'a str,
    ) -> BoxFuture<'a, Result<Platform, Failure>> {
        Box::pin(async move {
            self.call(&format!("platform {language}"));
            Ok(Platform::Windows)
        })
    }
    fn download<'a>(
        &'a self,
        transfer: &'a Transfer<Gog>,
        language: &'a str,
    ) -> BoxFuture<'a, Result<gog_kit::client::Installation, Failure>> {
        Box::pin(async move {
            self.call(&format!("download {language}"));
            let product_id = &transfer.product.product_id;
            let platform = transfer.platform.expect("a settled platform");
            Ok(gog_kit::client::Installation {
                product_id: product_id.clone(),
                user_id: ACCOUNT.into(),
                directory: transfer
                    .directory
                    .join(gog_kit::client::installation_directory_name(
                        product_id, platform,
                    ))
                    .to_string_lossy()
                    .into_owned(),
                platform,
                total_bytes: 100,
                build_id: "1".into(),
                version_name: "1.0".into(),
                language: language.into(),
                content_root: String::new(),
                script_interpreter: false,
                products: Vec::new(),
                dependencies: Vec::new(),
            })
        })
    }
    fn progress(&self) -> Option<TransferProgress> {
        None
    }
    fn cancel(&self) {
        self.call("cancel");
    }
    fn update_available<'a>(&'a self, _: &'a Receipt<Gog>) -> BoxFuture<'a, Result<bool, Failure>> {
        Box::pin(async move {
            self.call("update");
            Ok(false)
        })
    }
}

struct Fixture {
    _root: tempfile::TempDir,
    locations: Locations,
    fake: Arc<Fake>,
    content: Content<Fake>,
}

impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let library = root.path().canonicalize().unwrap();
        let locations = Locations::new(
            &library,
            Path::new("/Applications/Obelisk.app"),
            &library.join("home"),
        );
        let fake = Arc::new(Fake::default());
        let saves = fake.clone();
        let content = Content::new(
            fake.clone(),
            Arc::new(move || saves.call("saved")),
            "fr-FR".into(),
            locations.clone(),
        );
        Self {
            _root: root,
            locations,
            fake,
            content,
        }
    }

    fn transfer(&self, platform: Platform) -> Transfer<Gog> {
        Transfer {
            product: witcher(),
            name: "The Witcher".into(),
            platform: Some(platform),
            directory: self.locations.library.join("Games/GOG"),
        }
    }

    fn receipt(&self, platform: Platform) -> Receipt<Gog> {
        let directory = self.locations.library.join("Games/GOG").join(
            gog_kit::client::installation_directory_name("1207658924", platform),
        );
        std::fs::create_dir_all(&directory).unwrap();
        Receipt {
            product: witcher(),
            name: "The Witcher".into(),
            directory: directory.to_string_lossy().into_owned(),
            platform,
            total_bytes: 100,
            needs_verification: false,
        }
    }
}

/// The Witcher, as GOG names it.
fn witcher() -> Product {
    Product {
        product_id: "1207658924".into(),
    }
}

#[tokio::test]
async fn a_transfer_saves_renewed_credentials_first_and_installs_the_language() {
    let fixture = Fixture::new();
    let transfer = fixture.transfer(Platform::Windows);
    assert_eq!(
        fixture.content.begin(&transfer).await.unwrap(),
        Platform::Windows
    );
    let receipt = fixture.content.download(&transfer).await.unwrap();
    assert_eq!(receipt.product, witcher());
    assert_eq!(receipt.name, "The Witcher");
    assert_eq!(receipt.platform, Platform::Windows);
    assert!(!receipt.needs_verification);
    assert_eq!(
        Path::new(&receipt.directory),
        fixture
            .locations
            .library
            .join("Games/GOG/gog-1207658924-windows")
    );
    let calls = fixture.fake.calls();
    assert_eq!(&calls[..3], ["prepare", "saved", "download fr-FR"]);
}

#[tokio::test]
async fn a_transfer_without_a_platform_installs_the_build_gog_picks_in_its_language() {
    let fixture = Fixture::new();
    let transfer = Transfer {
        platform: None,
        ..fixture.transfer(Platform::Mac)
    };
    assert_eq!(
        fixture.content.begin(&transfer).await.unwrap(),
        Platform::Windows
    );
    // Picking the build may renew the credentials: they are saved after it.
    eventually(|| fixture.fake.calls().len() == 4).await;
    assert_eq!(
        fixture.fake.calls(),
        ["prepare", "saved", "platform fr-FR", "saved"]
    );
}

#[tokio::test]
async fn a_transfer_cancelled_while_its_credentials_renew_never_begins() {
    let fixture = Arc::new(Fixture::new());
    *fixture.fake.held.lock().unwrap() = true;
    let beginning = {
        let fixture = fixture.clone();
        tokio::spawn(async move {
            let transfer = fixture.transfer(Platform::Mac);
            fixture.content.begin(&transfer).await
        })
    };
    while fixture.fake.calls().is_empty() {
        tokio::task::yield_now().await;
    }
    fixture.content.cancel();
    fixture.fake.released.notify_one();
    assert_eq!(beginning.await.unwrap(), Err(Failure::Cancelled));
}

#[tokio::test]
async fn a_windows_launch_gets_voile_before_it_is_prepared_and_a_mac_launch_gets_none() {
    let fixture = Fixture::new();
    let (events, record) = recorded();
    let runtimes = FakeRuntimes::new(&[], record);
    runtimes.hold();
    let sessions = Sessions::<Gog>::new(
        Box::new(|| Ok(None)),
        Runtimes::from_source(runtimes.clone()),
    );
    eventually(|| !sessions.state().recovering).await;
    let admission = sessions
        .admit(witcher().product_id, ACCOUNT.into())
        .unwrap();
    let canceller = admission.canceller();
    let windows = fixture.receipt(Platform::Windows);

    let (launched, ()) = tokio::join!(fixture.content.launch(&windows, admission, None), async {
        eventually(|| !events.lock().unwrap().is_empty()).await;
        canceller.cancel();
    });

    assert_eq!(launched, Err(Failure::Cancelled), "nothing is prepared");
    assert_eq!(*events.lock().unwrap(), ["voile"]);

    // Downloads no longer wait: one the Mac build got would show.
    runtimes.release();
    // The signed-out account stops the Mac launch once prepared.
    *fixture.fake.signed_out.lock().unwrap() = true;
    let admission = sessions
        .admit(witcher().product_id, ACCOUNT.into())
        .unwrap();
    let launched = fixture
        .content
        .launch(&fixture.receipt(Platform::Mac), admission, None)
        .await;
    assert_eq!(
        launched,
        Err(Failure::Failed("backend.gog.unauthorized".into()))
    );
    assert_eq!(*events.lock().unwrap(), ["voile"], "a Mac build needs none");
}

#[tokio::test]
async fn gog_keeps_no_cloud_saves() {
    let fixture = Fixture::new();
    let receipt = fixture.receipt(Platform::Mac);
    assert_eq!(fixture.content.archive(&receipt).await, Ok(()));
    assert_eq!(fixture.content.removed(&receipt).await, Ok(()));
    assert!(
        fixture
            .content
            .indicators(&["1207658924".into()])
            .unwrap()
            .is_empty()
    );
    let control = Arc::new(crate::cloud::Control::new(|_| {}));
    assert_eq!(
        fixture
            .content
            .maintain(&"1207658924".into(), Some(&receipt), control)
            .await,
        Err(Failure::Failed("backend.gog.unsupported".into()))
    );
}

#[test]
fn only_a_windows_build_runs_in_its_own_prefix() {
    let fixture = Fixture::new();
    let windows = fixture.receipt(Platform::Windows);
    let configuration = fixture.content.configuration(&windows);
    assert_eq!(
        configuration.prefix.as_deref(),
        Some(
            fixture
                .locations
                .wine_prefixes
                .join("gog-1207658924-windows")
                .as_path()
        )
    );
    assert_eq!(
        configuration.session_directory,
        fixture.locations.gog_sessions
    );
    assert_eq!(configuration.title, "The Witcher");
    configuration.validate(ACCOUNT).unwrap();

    let mac = fixture.receipt(Platform::Mac);
    let configuration = fixture.content.configuration(&mac);
    assert_eq!(configuration.prefix, None);
    configuration.validate(ACCOUNT).unwrap();
    assert_eq!(fixture.content.directory_name(&mac), "gog-1207658924-macos");
}

#[tokio::test]
async fn an_installed_build_updates_in_place_whether_updated_or_offered_again() {
    let fixture = Fixture::new();
    let library = fixture.locations.library.clone();
    let previous = library.join("Previous location/gog-1207658924-windows");
    std::fs::create_dir_all(&previous).unwrap();
    let file = library.join("gog-installations.json");
    let written = serde_json::json!([{
        "productId": "1207658924",
        "name": "The Witcher",
        "directory": previous,
        "platform": "windows",
        "totalBytes": 100
    }]);
    std::fs::write(&file, serde_json::to_vec(&written).unwrap()).unwrap();
    let sessions = Arc::new(Sessions::<Gog>::new(Box::new(|| Ok(None)), ready()));
    eventually(|| !sessions.state().recovering).await;
    let installations = Installations::new(
        Arc::new(fixture.content),
        sessions,
        ACCOUNT.to_string(),
        file,
        fixture.locations.home.clone(),
        crate::games_folder::GamesFolder::new(&library),
        Queueing {
            queue: DownloadQueue::new(),
            account: ACCOUNT.into(),
            name: |game: &String| game.clone(),
        },
    );
    let idle = || {
        let state = installations.state();
        !state.downloading && state.queued.is_empty()
    };

    installations.update("1207658924:windows");
    eventually(idle).await;
    let receipts = installations.state().receipts;
    assert_eq!(receipts.len(), 1);
    let windows = receipts[0].clone();
    assert_eq!(Path::new(&windows.directory), previous);

    installations.install(Offer {
        product: witcher(),
        name: "The Witcher".into(),
    });
    eventually(idle).await;
    let state = installations.state();
    assert_eq!(state.report, None);
    assert_eq!(
        state.receipts,
        [windows],
        "the offer goes over the installed build"
    );
    assert_eq!(
        fixture
            .fake
            .calls()
            .iter()
            .filter(|call| call.starts_with("download"))
            .count(),
        2
    );
}
