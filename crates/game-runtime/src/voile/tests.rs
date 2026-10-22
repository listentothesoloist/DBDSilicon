use super::*;

/// A release archive laid out as voile's package.sh makes it, with the Linux
/// engine's runtime in its `linux` folder, and the release that pins it.
/// `manifest_version` is what its runtime.json claims.
fn release_archive(directory: &Path, version: &str, manifest_version: &str) -> (PathBuf, Release) {
    let package = directory.join("package");
    let runtime = package.join(format!("voile-{version}"));
    fs::create_dir_all(runtime.join("bin")).unwrap();
    fs::write(runtime.join("bin/wine"), "wine").unwrap();
    fs::create_dir_all(runtime.join("linux/engine")).unwrap();
    fs::create_dir_all(runtime.join("linux/wine/bin")).unwrap();
    fs::write(runtime.join("linux/engine/fexdarwin-run"), "engine").unwrap();
    fs::write(runtime.join("linux/wine/bin/wine"), "wine").unwrap();
    fs::write(
        runtime.join(MANIFEST),
        format!(r#"{{"version": "{manifest_version}"}}"#),
    )
    .unwrap();
    let archive = directory.join(format!("voile-{version}.tar.xz"));
    let status = std::process::Command::new("/usr/bin/tar")
        .arg("-cJf")
        .arg(&archive)
        .arg("-C")
        .arg(&package)
        .arg(format!("voile-{version}"))
        .status()
        .unwrap();
    assert!(status.success());
    let release = Release {
        version: version.into(),
        url: "https://example.invalid/voile.tar.xz".into(),
        size: fs::metadata(&archive).unwrap().len(),
        sha256: crate::sha(&archive).unwrap(),
    };
    (archive, release)
}

fn names(root: &Path) -> Vec<String> {
    let mut names: Vec<_> = fs::read_dir(root)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

#[test]
fn the_checked_in_release_names_its_archive() {
    let release = release();
    assert!(release.url.ends_with(&format!(
        "/releases/download/{0}/voile-{0}.tar.xz",
        release.version
    )));
    assert_eq!(release.sha256.len(), 64);
}

#[tokio::test]
async fn a_release_keeps_the_ones_before_it_until_they_are_removed() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("Voile");
    let (archive, release) = release_archive(directory.path(), "v1.0.0-2", "v1.0.0-2");
    fs::create_dir_all(root.join("v1.0.0-1/bin")).unwrap();
    fs::create_dir_all(root.join(".interrupted")).unwrap();

    let runtime = Install::new()
        .install(&root, &release, Some(&archive))
        .await
        .unwrap();

    assert_eq!(runtime, root.join("v1.0.0-2"));
    assert_eq!(located(&root, &release), Some(runtime));
    assert_eq!(names(&root), ["v1.0.0-1", "v1.0.0-2"]);

    // Once no game may run from them, the ones before it go.
    fs::create_dir_all(root.join("current/bin")).unwrap();
    fs::create_dir_all(root.join(".unfinished")).unwrap();
    remove_replaced_by(&root, "v1.0.0-2");
    assert_eq!(names(&root), [".unfinished", "current", "v1.0.0-2"]);
}

#[tokio::test]
async fn a_damaged_archive_installs_nothing() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("Voile");
    let (archive, mut release) = release_archive(directory.path(), "v1.0.0-2", "v1.0.0-2");
    release.sha256 = "0".repeat(64);
    let failure = Install::new()
        .install(&root, &release, Some(&archive))
        .await;
    assert_eq!(failure, Err(Failure::Damaged));
    assert_eq!(names(&root), Vec::<String>::new());
}

#[tokio::test]
async fn an_archive_of_another_release_installs_nothing() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("Voile");
    let (archive, release) = release_archive(directory.path(), "v1.0.0-2", "v1.0.0-1");
    let failure = Install::new()
        .install(&root, &release, Some(&archive))
        .await;
    assert_eq!(failure, Err(Failure::Damaged));
    assert_eq!(names(&root), Vec::<String>::new());
}

#[tokio::test]
async fn the_development_runtime_takes_precedence_over_any_release() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("Voile");
    let (archive, release) = release_archive(directory.path(), "v1.0.0-2", "v1.0.0-2");
    fs::create_dir_all(root.join("current/bin")).unwrap();
    fs::write(root.join("current/bin/wine"), "wine").unwrap();
    let runtime = Install::new()
        .install(&root, &release, Some(&archive))
        .await
        .unwrap();
    assert_eq!(runtime, root.join("current"));
    assert_eq!(names(&root), ["current"]);
}

#[tokio::test]
async fn the_installed_release_is_the_runtime_games_run_in() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("Voile");
    let (archive, release) = release_archive(directory.path(), "v1.0.0-2", "v1.0.0-2");
    assert_eq!(installed(&root, &release), None);
    Install::new()
        .install(&root, &release, Some(&archive))
        .await
        .unwrap();
    assert_eq!(installed(&root, &release), Some("v1.0.0-2".into()));
    fs::create_dir_all(root.join("current/bin")).unwrap();
    fs::write(root.join("current/bin/wine"), "wine").unwrap();
    fs::write(root.join("current").join(MANIFEST), r#"{"version": "dev"}"#).unwrap();
    assert_eq!(installed(&root, &release), Some("dev".into()));
}

#[tokio::test]
async fn a_cancelled_install_stops_before_it_starts() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("Voile");
    let (archive, release) = release_archive(directory.path(), "v1.0.0-2", "v1.0.0-2");
    let install = Install::new();
    install.cancel();
    let failure = install.install(&root, &release, Some(&archive)).await;
    assert_eq!(failure, Err(Failure::Cancelled));
    assert!(!root.exists());
}

#[tokio::test]
async fn the_linux_runtime_comes_with_its_release() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("Voile");
    let (archive, release) = release_archive(directory.path(), "v1.0.0-2", "v1.0.0-2");
    assert_eq!(located_linux(&root, &release), None);
    Install::new()
        .install(&root, &release, Some(&archive))
        .await
        .unwrap();
    assert_eq!(
        located_linux(&root, &release),
        Some(root.join("v1.0.0-2/linux"))
    );
}

#[test]
fn a_linux_runtime_left_from_another_release_is_not_used() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("Voile");
    let (_, release) = release_archive(directory.path(), "v1.0.0-2", "v1.0.0-2");
    fs::create_dir_all(root.join("v1.0.0-2/linux/engine")).unwrap();
    fs::create_dir_all(root.join("v1.0.0-2/linux/wine/bin")).unwrap();
    fs::write(root.join("v1.0.0-2/linux/engine/fexdarwin-run"), "engine").unwrap();
    fs::write(root.join("v1.0.0-2/linux/wine/bin/wine"), "wine").unwrap();
    // No Windows runtime of this release around it.
    assert_eq!(located_linux(&root, &release), None);
}

#[test]
fn the_development_linux_runtime_takes_precedence() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("Voile");
    let (_, release) = release_archive(directory.path(), "v1.0.0-2", "v1.0.0-2");
    fs::create_dir_all(root.join("current/linux/engine")).unwrap();
    fs::create_dir_all(root.join("current/linux/wine/bin")).unwrap();
    fs::write(root.join("current/linux/engine/fexdarwin-run"), "engine").unwrap();
    fs::write(root.join("current/linux/wine/bin/wine"), "wine").unwrap();
    assert_eq!(
        located_linux(&root, &release),
        Some(root.join("current/linux"))
    );
}
