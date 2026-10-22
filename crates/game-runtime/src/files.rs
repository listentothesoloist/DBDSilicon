use crate::{Config, Error};
use std::{
    fs,
    path::{Path, PathBuf},
};

pub(crate) struct Prepared {
    pub directory: PathBuf,
    pub run_directory: PathBuf,
    pub profile: PathBuf,
    pub report: PathBuf,
    pub endpoints: [(String, String); 2],
    pub storage: Option<crate::NativeStorage>,
}
impl Drop for Prepared {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.run_directory);
    }
}
impl Prepared {
    pub fn home(&self) -> PathBuf {
        self.storage
            .as_ref()
            .map_or_else(|| self.run_directory.join("home"), |s| s.home.clone())
    }
    pub fn enable_storage(&mut self, storage: &crate::NativeStorage) -> Result<(), Error> {
        safe_storage_path(&storage.home).map_err(|error| Error::Cloud(error.into()))?;
        let steam = storage.steam_directory();
        let app = storage.cache_directory();
        for path in [app.join("remote"), steam.join("appcache")] {
            safe_storage_path(&path).map_err(|error| Error::Cloud(error.into()))?;
            private_directory(&path)?;
        }
        // The coordinator must publish a complete local projection before a
        // native host starts. Validate it without loading the game's SDK. SDK
        // quota/readback readiness now belongs to the game's own attachment.
        crate::native_cache_files(app.clone(), storage.app_id, storage.platforms)?;
        use std::io::Write;
        let extra = format!(
            "\n(allow file-read* (subpath {}))\n(allow file-write* (subpath {}) (subpath {}))\n",
            quote(&storage.home)?,
            quote(&app)?,
            quote(&steam.join("appcache"))?
        );
        fs::OpenOptions::new()
            .append(true)
            .open(&self.profile)?
            .write_all(extra.as_bytes())?;
        self.storage = Some(storage.clone());
        Ok(())
    }
    pub fn consumer_environment(&self, service: &str) -> std::collections::HashMap<String, String> {
        let mut environment: std::collections::HashMap<_, _> =
            self.endpoints.iter().cloned().collect();
        environment.insert(
            "DYLD_INSERT_LIBRARIES".into(),
            self.directory
                .join(crate::package::ADAPTER)
                .to_string_lossy()
                .into_owned(),
        );
        environment.insert("OBELISK_PRIVATE_STEAM_DISCOVERY".into(), service.into());
        environment
    }
}
pub(crate) fn private_directory(path: &Path) -> Result<(), Error> {
    fs::create_dir_all(path)?;
    if !fs::symlink_metadata(path)?.file_type().is_dir() {
        return Err(Error::Storage);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}
/// Refuses a path for Cloud storage that is relative, climbs out with `..`,
/// or passes through a symbolic link other than macOS's /var and /tmp.
pub(crate) fn safe_storage_path(path: &Path) -> cloud_vault::Result<()> {
    if !path.is_absolute()
        || path
            .components()
            .any(|c| matches!(c, std::path::Component::ParentDir))
    {
        return Err(cloud_vault::Error::Invalid);
    }
    for parent in path.ancestors() {
        match fs::symlink_metadata(parent) {
            Ok(m)
                if m.file_type().is_symlink()
                    && ![Path::new("/var"), Path::new("/tmp")].contains(&parent) =>
            {
                return Err(cloud_vault::Error::Invalid);
            }
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
    }
    Ok(())
}
/// Writes `bytes` to a file named `name` in a new folder inside `parent`
/// that only this user opens, whose name starts with `prefix`. The folder
/// lives as long as the returned TempDir.
pub(crate) fn private_temp_file(
    parent: &Path,
    prefix: &str,
    name: &str,
    bytes: &[u8],
) -> std::io::Result<(tempfile::TempDir, PathBuf)> {
    use std::os::unix::fs::PermissionsExt;
    let directory = tempfile::Builder::new()
        .prefix(prefix)
        .permissions(fs::Permissions::from_mode(0o700))
        .tempdir_in(parent)?;
    let path = directory.path().join(name);
    obelisk_core::write_atomic(&path, bytes)?;
    Ok((directory, path))
}
pub(crate) fn prepare(
    config: &Config,
    cancelled: &std::sync::atomic::AtomicBool,
) -> Result<Prepared, Error> {
    if config.wine.is_none() && !config.steam_api.as_deref().is_some_and(Path::is_file) {
        return Err(Error::Unavailable);
    }
    let directory = crate::package::prepare(config, cancelled)?;
    let id = uuid::Uuid::new_v4();
    let run_directory = config.cache.join(format!("{}{id}", crate::package::LAUNCH));
    crate::package::remove_stale(&config.cache, &directory, &run_directory);
    let listeners = [
        std::net::TcpListener::bind("127.0.0.1:0")?,
        std::net::TcpListener::bind("127.0.0.1:0")?,
    ];
    // Refuse an unsupported or malformed SDK library before anything is written.
    // The selected slice itself is owned by Launch, which the supervisor reads.
    if let Some(library) = &config.steam_api {
        crate::supervisor::architecture::Architecture::preferred(library)?;
    }
    let prepared = Prepared {
        storage: None,
        directory,
        profile: run_directory.join("host.sb"),
        report: crate::diagnostics::launch_report(&config.diagnostics, config.app_id, id),
        run_directory,
        endpoints: [
            (
                "Steam3Master".into(),
                listeners[0].local_addr()?.to_string(),
            ),
            (
                "SteamClientService".into(),
                listeners[1].local_addr()?.to_string(),
            ),
        ],
    };
    let run_directory = &prepared.run_directory;
    private_directory(run_directory)?;
    private_directory(&run_directory.join("home"))?;
    private_directory(&run_directory.join("tmp"))?;
    private_directory(&config.diagnostics)?;
    fs::write(&prepared.profile, sandbox(config, &prepared.directory)?)?;
    Ok(prepared)
}
fn quote(path: &Path) -> Result<String, Error> {
    serde_json::to_string(path.to_str().ok_or(Error::Storage)?).map_err(|_| Error::Storage)
}
fn sandbox(config: &Config, directory: &Path) -> Result<String, Error> {
    let mut profile = format!(
        "(version 1)\n(allow default)\n(deny file-write*)\n(allow file-write* (literal \"/dev/null\"))\n(deny file-read* (subpath {}) (subpath {}))\n(deny process-exec)\n(allow process-exec (literal {}))\n(deny mach-lookup (global-name-regex \".*[Ss]team.*\") (global-name \"com.apple.coreservices.launchservicesd\") (global-name \"com.apple.lsd.mapdb\"))\n",
        quote(&config.home.join("Library/Application Support/Steam"))?,
        quote(
            &config
                .home
                .join("Library/Application Support/DBDSilicon")
        )?,
        quote(&directory.join(crate::package::HOST))?
    );
    // A fresh, read-only native generation cannot inspect the ordinary saves
    // owned by Rust. Permit only the pinned runtime under the real home. The
    // native host never receives root-0 API storage or any save projection.
    profile.push_str(&format!(
        "(deny file-read* (subpath {}))\n(allow file-read* (subpath {}))\n",
        quote(&config.home)?,
        quote(directory)?
    ));
    // realpath on the host executable needs metadata for each ancestor. Grant
    // those exact directories, without granting enumeration or save contents.
    for ancestor in directory.ancestors().skip(1) {
        profile.push_str(&format!(
            "(allow file-read-metadata (literal {}))\n",
            quote(ancestor)?
        ));
    }
    for root in &config.cloud_save_roots {
        profile.push_str(&format!("(deny file-read* (subpath {}))\n", quote(root)?));
    }
    Ok(profile)
}
/// Replaces `path` atomically without fsync: diagnostics need no durability,
/// unlike [`obelisk_core::write_atomic`].
fn atomic_write(
    path: &Path,
    write: impl FnOnce(&mut std::fs::File) -> Result<(), Error>,
) -> Result<(), Error> {
    let mut temporary = tempfile::NamedTempFile::new_in(path.parent().ok_or(Error::Storage)?)?;
    write(temporary.as_file_mut())?;
    temporary.persist(path).map_err(|_| Error::Storage)?;
    Ok(())
}

pub(crate) fn report(path: &Path, proof: &crate::Proof) -> Result<(), Error> {
    atomic_write(path, |file| {
        serde_json::to_writer_pretty(file, proof).map_err(|_| Error::Storage)
    })
}
pub(crate) fn failure_report(
    config: &Config,
    error: &Error,
    events: &[crate::protocol::Event],
) -> Result<(), Error> {
    private_directory(&config.diagnostics)?;
    let path = crate::diagnostics::failure_report(
        &config.diagnostics,
        config.app_id,
        uuid::Uuid::new_v4(),
    );
    let report =
        serde_json::json!({"app_id": config.app_id, "error": error.to_string(), "events": events});
    fs::write(
        path,
        serde_json::to_vec_pretty(&report).map_err(|_| Error::Storage)?,
    )?;
    Ok(())
}

#[cfg(test)]
mod atomic_tests {
    use super::*;
    use std::io::Write;

    #[cfg(target_os = "macos")]
    #[test]
    fn sandbox_access_child() {
        let Some(root) = std::env::var_os("OBELISK_SANDBOX_ACCESS_FIXTURE") else {
            return;
        };
        let root = PathBuf::from(root);
        assert!(root.join("home/cache/runtime").canonicalize().is_ok());
        assert!(fs::read(root.join("home/saves/slot.dat")).is_err());
        assert!(fs::read(root.join("home/Library/Application Support/Steam/fixture")).is_err());
        if std::env::var_os("OBELISK_SANDBOX_STORAGE_FIXTURE").is_some() {
            let steam = root.join("home/sdk/Library/Application Support/Steam");
            assert!(fs::write(steam.join("userdata/42/42/remote/slot"), b"allowed").is_ok());
            assert!(fs::write(steam.join("userdata/42/43/slot"), b"wrong app").is_err());
            assert!(fs::write(steam.join("userdata/43/42/slot"), b"wrong account").is_err());
            assert!(fs::write(steam.join("config/loginusers.vdf"), b"credentials").is_err());
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn sandbox_allows_runtime_path_resolution_but_denies_save_contents() {
        let root = tempfile::tempdir().unwrap();
        let root = root.path().canonicalize().unwrap();
        let home = root.join("home");
        let runtime = home.join("cache/runtime");
        fs::create_dir_all(&runtime).unwrap();
        fs::create_dir_all(home.join("saves")).unwrap();
        fs::create_dir_all(home.join("Library/Application Support/Steam")).unwrap();
        fs::write(home.join("saves/slot.dat"), b"fixture save").unwrap();
        fs::write(
            home.join("Library/Application Support/Steam/fixture"),
            b"fixture installation",
        )
        .unwrap();
        let config = Config {
            helpers: runtime.clone(),
            package: root.join("unused"),
            cache: home.join("cache"),
            diagnostics: root.join("logs"),
            steam_api: None,
            wine: None,
            home,
            app_id: 42,
            cloud_save_roots: vec![],
        };
        let executable = std::env::current_exe().unwrap();
        let profile = format!(
            "{}\n(allow process-exec (literal {}))",
            sandbox(&config, &runtime).unwrap(),
            quote(&executable).unwrap()
        );
        let result = std::process::Command::new("/usr/bin/sandbox-exec")
            .args(["-p", &profile])
            .arg(executable)
            .args(["files::atomic_tests::sandbox_access_child", "--exact"])
            .env("OBELISK_SANDBOX_ACCESS_FIXTURE", &root)
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stdout)
        );
        let storage = crate::NativeStorage {
            home: config.home.join("sdk"),
            account_id: 0x011000010000002a,
            app_id: 42,
            platforms: 2,
        };
        let steam = storage.home.join("Library/Application Support/Steam");
        for path in ["userdata/42/43", "userdata/43/42", "config"] {
            fs::create_dir_all(steam.join(path)).unwrap();
        }
        steam_kit::cloud::SdkCloudPaths::new(steam.join("userdata/42/42"), 42, 2)
            .unwrap()
            .prepare_index(&[])
            .unwrap();
        let run = root.join("run");
        fs::create_dir_all(&run).unwrap();
        let mut prepared = Prepared {
            directory: runtime,
            run_directory: run.clone(),
            profile: run.join("profile"),
            report: run.join("report"),
            endpoints: Default::default(),
            storage: None,
        };
        fs::write(&prepared.profile, &profile).unwrap();
        prepared.enable_storage(&storage).unwrap();
        let result = std::process::Command::new("/usr/bin/sandbox-exec")
            .args(["-f"])
            .arg(&prepared.profile)
            .arg(std::env::current_exe().unwrap())
            .args(["files::atomic_tests::sandbox_access_child", "--exact"])
            .env("OBELISK_SANDBOX_ACCESS_FIXTURE", &root)
            .env("OBELISK_SANDBOX_STORAGE_FIXTURE", "1")
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stdout)
        );
    }

    #[test]
    fn only_a_mac_game_needs_its_steam_api_library_to_start_the_host() {
        let root = tempfile::tempdir().unwrap();
        let mut config = Config {
            helpers: root.path().join("helpers"),
            package: root.path().join("unused"),
            cache: root.path().join("cache"),
            diagnostics: root.path().join("logs"),
            steam_api: None,
            wine: None,
            home: root.path().join("home"),
            app_id: 42,
            cloud_save_roots: vec![],
        };
        // A cancelled launch stops at the runtime package, after the library.
        let cancelled = std::sync::atomic::AtomicBool::new(true);

        assert!(matches!(
            prepare(&config, &cancelled),
            Err(Error::Unavailable)
        ));
        config.wine = Some(crate::wine::tests::config(root.path()));
        assert!(matches!(
            prepare(&config, &cancelled),
            Err(Error::Cancelled)
        ));
    }

    #[test]
    fn incomplete_atomic_writes_preserve_the_previous_file_and_remove_temporary_files() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("state.json");
        fs::write(&path, b"previous").unwrap();
        let result = atomic_write(&path, |file| {
            file.write_all(b"incomplete")?;
            Err(Error::Storage)
        });
        assert!(result.is_err());
        assert_eq!(fs::read(&path).unwrap(), b"previous");
        assert_eq!(fs::read_dir(root.path()).unwrap().count(), 1);
        atomic_write(&path, |file| {
            file.write_all(b"complete")?;
            Ok(())
        })
        .unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"complete");
    }
}
