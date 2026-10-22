use super::*;
use std::{
    collections::HashMap,
    io::{BufRead, BufReader},
    net::{TcpListener, TcpStream},
    sync::Mutex,
    thread,
};

/// What the loopback CDN answers for one path.
#[derive(Clone)]
enum Answer {
    Body(Vec<u8>),
    Status(&'static str),
    /// Sends the first bytes of a body, then never the rest.
    Stall(Vec<u8>),
}

/// Valve's CDN on the loopback interface, which records every request.
struct Cdn {
    base: String,
    requests: Arc<Mutex<Vec<String>>>,
}

impl Cdn {
    fn new(answers: HashMap<String, Answer>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let requests = Arc::new(Mutex::new(Vec::new()));
        let recorded = requests.clone();
        let answers = Arc::new(answers);
        thread::spawn(move || {
            for socket in listener.incoming().flatten() {
                let (answers, recorded) = (answers.clone(), recorded.clone());
                thread::spawn(move || serve(socket, &answers, &recorded));
            }
        });
        Self { base, requests }
    }

    fn requests(&self) -> Vec<String> {
        std::mem::take(&mut *self.requests.lock().unwrap())
    }
}

fn serve(socket: TcpStream, answers: &HashMap<String, Answer>, requests: &Mutex<Vec<String>>) {
    let mut reader = BufReader::new(socket.try_clone().unwrap());
    let mut line = String::new();
    if reader.read_line(&mut line).is_err() {
        return;
    }
    let path = line
        .split_whitespace()
        .nth(1)
        .unwrap_or_default()
        .to_owned();
    loop {
        let mut header = String::new();
        if reader.read_line(&mut header).unwrap_or(0) <= 2 {
            break;
        }
    }
    requests.lock().unwrap().push(path.clone());
    let mut socket = socket;
    let _ = match answers.get(&path) {
        Some(Answer::Body(body)) => write!(
            socket,
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        )
        .and_then(|()| socket.write_all(body)),
        Some(Answer::Status(status)) => write!(
            socket,
            "HTTP/1.1 {status}\r\nLocation: /elsewhere\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
        ),
        Some(Answer::Stall(body)) => {
            let sent = write!(
                socket,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            )
            .and_then(|()| socket.write_all(&body[..body.len() / 2]))
            .and_then(|()| socket.flush());
            thread::sleep(Duration::from_secs(10));
            sent
        }
        None => write!(
            socket,
            "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
        ),
    };
}

fn sha(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn zip_of(entries: &[(&str, &[u8])]) -> Vec<u8> {
    let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    for (name, bytes) in entries {
        writer
            .start_file(*name, zip::write::SimpleFileOptions::default())
            .unwrap();
        writer.write_all(bytes).unwrap();
    }
    writer.finish().unwrap().into_inner()
}

/// Two pinned packages, each holding a file the payload keeps and one it
/// leaves out, served by the CDN `edit` may change.
struct Fixture {
    _root: tempfile::TempDir,
    cdn: Cdn,
    manifest: Manifest,
    cache: PathBuf,
    payload: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        Self::serving(|_, answer| answer)
    }

    fn serving(edit: impl Fn(&str, Answer) -> Answer) -> Self {
        let files: [(&str, &str, &[u8]); 2] = [
            ("client", "steamclient.dylib", b"fixture engine"),
            (
                "breakpad",
                "Frameworks/Breakpad.framework/Versions/A/Breakpad",
                b"fixture crash reporter",
            ),
        ];
        let packages = files
            .iter()
            .map(|(package, path, bytes)| {
                let other: &[u8] = b"not part of the payload";
                (
                    *package,
                    zip_of(&[(path, bytes), ("unrelated.dylib", other)]),
                )
            })
            .collect::<Vec<_>>();
        let answers = packages
            .iter()
            .map(|(name, body)| {
                let path = format!("/{name}.zip");
                let answer = edit(name, Answer::Body(body.clone()));
                (path, answer)
            })
            .collect();
        let cdn = Cdn::new(answers);
        let manifest = Manifest {
            version: "42".into(),
            packages: packages
                .iter()
                .map(|(name, body)| SourcePackage {
                    name: (*name).into(),
                    url: format!("{}/{name}.zip", cdn.base),
                    size: body.len() as u64,
                    sha256: sha(body),
                })
                .collect(),
            files: files
                .iter()
                .map(|(package, path, bytes)| PayloadFile {
                    path: (*path).into(),
                    package: (*package).into(),
                    size: bytes.len() as u64,
                    sha256: sha(bytes),
                })
                .collect(),
        };
        let root = tempfile::tempdir().unwrap();
        let cache = root.path().join("SteamRuntime");
        let payload = payload_in(&cache, &manifest);
        Self {
            _root: root,
            cdn,
            manifest,
            cache,
            payload,
        }
    }

    async fn install(&self, install: &Install) -> Result<(), Failure> {
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .unwrap();
        install
            .install(&client, &self.manifest, &self.payload)
            .await
    }

    fn names(&self) -> Vec<String> {
        let mut names: Vec<_> = fs::read_dir(&self.cache)
            .map(|entries| {
                entries
                    .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
                    .collect()
            })
            .unwrap_or_default();
        names.sort();
        names
    }

    fn unfinished(&self) -> bool {
        self.names().iter().any(|name| name.starts_with(UNFINISHED))
    }
}

#[test]
fn the_payload_of_the_pinned_manifest_lives_in_the_steam_runtime_cache() {
    let locations = crate::Locations::new(
        Path::new("/library"),
        Path::new("/Obelisk.app"),
        Path::new("/home"),
    );
    assert_eq!(
        locations.steam_runtime_package,
        locations
            .steam_runtime_cache
            .join(format!("valve-{}.zip", package::manifest().version))
    );
    assert!(
        locations
            .steam_runtime_package
            .starts_with("/home/Library/Caches")
    );
    assert_eq!(Install::new().progress(), (0, size(package::manifest())));
}

#[tokio::test]
async fn an_install_keeps_only_the_pinned_files_in_one_payload() {
    let fixture = Fixture::new();
    let install = Install::new();
    fixture.install(&install).await.unwrap();

    assert_eq!(fixture.names(), ["valve-42.zip"]);
    package::verify(
        &fixture.payload,
        &fixture.manifest.files,
        &AtomicBool::new(false),
    )
    .unwrap();
    let archive = zip::ZipArchive::new(File::open(&fixture.payload).unwrap()).unwrap();
    assert_eq!(archive.len(), fixture.manifest.files.len());
    let total = size(&fixture.manifest);
    assert_eq!(install.progress(), (total, total));
    let mut requests = fixture.cdn.requests();
    requests.sort();
    assert_eq!(requests, ["/breakpad.zip", "/client.zip"]);
}

#[tokio::test]
async fn an_intact_payload_is_kept_without_a_download() {
    let fixture = Fixture::new();
    fixture.install(&Install::new()).await.unwrap();
    fixture.cdn.requests();
    let installed = fs::metadata(&fixture.payload).unwrap().modified().unwrap();

    fixture.install(&Install::new()).await.unwrap();

    assert!(fixture.cdn.requests().is_empty());
    assert_eq!(
        fs::metadata(&fixture.payload).unwrap().modified().unwrap(),
        installed
    );
}

#[tokio::test]
async fn a_payload_counts_as_installed_from_its_listing_alone() {
    let fixture = Fixture::new();
    let files = &fixture.manifest.files;
    assert!(!listed(&fixture.payload, files));
    fixture.install(&Install::new()).await.unwrap();
    assert!(listed(&fixture.payload, files));

    // The listing is all it reads: other bytes of the same sizes pass, and
    // `package::prepare` refuses them when it extracts.
    let engine: &[u8] = b"fixture ENGINE";
    let reporter: &[u8] = b"fixture CRASH REPORTER";
    let paths = [files[0].path.as_str(), files[1].path.as_str()];
    let same_sizes = zip_of(&[(paths[0], engine), (paths[1], reporter)]);
    fs::write(&fixture.payload, same_sizes).unwrap();
    assert!(listed(&fixture.payload, files));

    // A missing file, another size or a damaged listing does not pass.
    fs::write(&fixture.payload, zip_of(&[(paths[0], engine)])).unwrap();
    assert!(!listed(&fixture.payload, files));
    let other_size: &[u8] = b"fixture crash reporter, longer";
    let other_sizes = zip_of(&[(paths[0], engine), (paths[1], other_size)]);
    fs::write(&fixture.payload, other_sizes).unwrap();
    assert!(!listed(&fixture.payload, files));
    fs::write(&fixture.payload, b"PK damaged").unwrap();
    assert!(!listed(&fixture.payload, files));
}

#[tokio::test]
async fn a_damaged_payload_is_downloaded_anew() {
    let fixture = Fixture::new();
    fixture.install(&Install::new()).await.unwrap();
    fixture.cdn.requests();
    fs::write(&fixture.payload, b"PK damaged").unwrap();

    fixture.install(&Install::new()).await.unwrap();

    assert_eq!(fixture.cdn.requests().len(), 2);
    package::verify(
        &fixture.payload,
        &fixture.manifest.files,
        &AtomicBool::new(false),
    )
    .unwrap();
}

#[tokio::test]
async fn a_package_valve_no_longer_serves_asks_for_another_obelisk() {
    for status in ["404 Not Found", "410 Gone", "302 Found"] {
        let fixture = Fixture::serving(|name, answer| match name {
            "breakpad" => Answer::Status(status),
            _ => answer,
        });
        assert_eq!(
            fixture.install(&Install::new()).await,
            Err(Failure::Unavailable)
        );
        assert!(fixture.names().is_empty(), "{status}");
    }
}

#[tokio::test]
async fn a_cdn_that_fails_or_cannot_be_reached_is_a_download_failure() {
    for status in [
        "503 Service Unavailable",
        "429 Too Many Requests",
        "403 Forbidden",
    ] {
        let fixture = Fixture::serving(|_, _| Answer::Status(status));
        assert_eq!(
            fixture.install(&Install::new()).await,
            Err(Failure::Download),
            "{status}"
        );
        assert!(!fixture.payload.exists(), "{status}");
    }

    let mut fixture = Fixture::new();
    let closed = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = closed.local_addr().unwrap();
    drop(closed);
    for source in &mut fixture.manifest.packages {
        source.url = format!("http://{address}/{}.zip", source.name);
    }
    assert_eq!(
        fixture.install(&Install::new()).await,
        Err(Failure::Download)
    );
    assert!(!fixture.payload.exists());
}

#[tokio::test]
async fn a_package_that_differs_from_its_pin_installs_nothing() {
    let changed = Fixture::serving(|name, answer| match (name, answer) {
        ("client", Answer::Body(mut body)) => {
            let last = body.len() - 1;
            body[last] ^= 1;
            Answer::Body(body)
        }
        (_, answer) => answer,
    });
    let longer = Fixture::serving(|name, answer| match (name, answer) {
        ("client", Answer::Body(mut body)) => {
            body.extend_from_slice(b"more");
            Answer::Body(body)
        }
        (_, answer) => answer,
    });
    for fixture in [changed, longer] {
        assert_eq!(
            fixture.install(&Install::new()).await,
            Err(Failure::Damaged)
        );
        assert!(fixture.names().is_empty());
    }
}

#[tokio::test]
async fn a_pinned_file_that_differs_installs_nothing() {
    let mut fixture = Fixture::new();
    fixture.manifest.files[0].sha256 = "0".repeat(64);
    assert_eq!(
        fixture.install(&Install::new()).await,
        Err(Failure::Damaged)
    );
    assert!(fixture.names().is_empty());
}

#[tokio::test]
async fn a_cancelled_install_stops_before_it_starts() {
    let fixture = Fixture::new();
    let install = Install::new();
    install.cancel();
    assert_eq!(fixture.install(&install).await, Err(Failure::Cancelled));
    assert!(!fixture.cache.exists());
    assert!(fixture.cdn.requests().is_empty());
}

#[tokio::test]
async fn cancelling_a_download_leaves_nothing_behind() {
    let fixture = Fixture::serving(|name, answer| match (name, answer) {
        ("client", Answer::Body(body)) => Answer::Stall(body),
        (_, answer) => answer,
    });
    let install = Install::new();
    let cancel = async {
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        while install.progress().0 == 0 {
            assert!(std::time::Instant::now() < deadline, "timed out");
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        install.cancel();
    };
    let (installed, ()) = tokio::join!(fixture.install(&install), cancel);
    assert_eq!(installed, Err(Failure::Cancelled));
    assert!(fixture.names().is_empty());
}

#[tokio::test]
async fn an_install_removes_unfinished_downloads_and_other_payloads_only() {
    let fixture = Fixture::new();
    fs::create_dir_all(fixture.cache.join(".download-interrupted")).unwrap();
    fs::write(fixture.cache.join("valve-41.zip"), b"older manifest").unwrap();
    fs::create_dir_all(fixture.cache.join(".staging-preparing")).unwrap();
    fs::create_dir_all(fixture.cache.join("runtime-v1-prepared")).unwrap();
    fs::create_dir_all(fixture.cache.join("launch-running")).unwrap();

    fixture.install(&Install::new()).await.unwrap();

    assert!(!fixture.unfinished());
    assert_eq!(
        fixture.names(),
        [
            ".staging-preparing",
            "launch-running",
            "runtime-v1-prepared",
            "valve-42.zip"
        ]
    );
}

#[tokio::test]
async fn the_cdn_client_reaches_valve_over_https_only() {
    let fixture = Fixture::new();
    let client = client().unwrap();
    let failure = Install::new()
        .install(&client, &fixture.manifest, &fixture.payload)
        .await;
    assert_eq!(failure, Err(Failure::Download));
    assert!(fixture.cdn.requests().is_empty());
    assert!(!fixture.unfinished());
}
