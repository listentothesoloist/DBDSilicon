use super::*;

/// A runtime as Voile ships it: its engine, its Steam API bridge and its guest's resolver.
fn stock_runtime(root: &Path) -> std::path::PathBuf {
    let runtime = root.join("runtime");
    for (file, contents) in [
        ("engine/fexdarwin-run", "stock engine"),
        (STEAM_CLIENT, "stock bridge"),
        (RESOLV_CONF, "nameserver 1.1.1.1\n"),
    ] {
        fs::create_dir_all(runtime.join(file).parent().unwrap()).unwrap();
        fs::write(runtime.join(file), contents).unwrap();
    }
    runtime
}

/// An archive of fixes laid out as the runtime, made the way `make-archive.sh` makes it.
fn archive(root: &Path, files: &[(&str, &str)]) -> std::path::PathBuf {
    let staging = root.join("staging");
    for (file, contents) in files {
        fs::create_dir_all(staging.join(file).parent().unwrap()).unwrap();
        fs::write(staging.join(file), contents).unwrap();
    }
    let archive = root.join("runtime-fixes.tar.xz");
    let status = Command::new("/usr/bin/tar")
        .arg("-cJf")
        .arg(&archive)
        .arg("-C")
        .arg(&staging)
        .args(files.iter().map(|(file, _)| file))
        .status()
        .unwrap();
    assert!(status.success());
    archive
}

#[test]
fn a_stock_runtime_gets_the_fixes_once() {
    let root = tempfile::tempdir().unwrap();
    let runtime = stock_runtime(root.path());
    let archive = archive(
        root.path(),
        &[("engine/fexdarwin-run", "fixed engine"), (STEAM_CLIENT, "fixed bridge")],
    );

    super::runtime(&archive, &runtime).unwrap();
    assert_eq!(fs::read_to_string(runtime.join("engine/fexdarwin-run")).unwrap(), "fixed engine");
    assert_eq!(fs::read_to_string(runtime.join(STEAM_CLIENT)).unwrap(), "fixed bridge");
    assert_eq!(
        fs::read_to_string(runtime.join(MARKER)).unwrap(),
        crate::sha(&archive).unwrap()
    );

    // The marker spares the next launch the extraction.
    fs::write(runtime.join("engine/fexdarwin-run"), "left alone").unwrap();
    super::runtime(&archive, &runtime).unwrap();
    assert_eq!(fs::read_to_string(runtime.join("engine/fexdarwin-run")).unwrap(), "left alone");
}

#[test]
fn a_build_without_the_archive_leaves_the_runtime_as_shipped() {
    let root = tempfile::tempdir().unwrap();
    let runtime = stock_runtime(root.path());

    super::runtime(&root.path().join("missing.tar.xz"), &runtime).unwrap();
    assert_eq!(fs::read_to_string(runtime.join("engine/fexdarwin-run")).unwrap(), "stock engine");
    assert!(!runtime.join(MARKER).exists());
}

#[test]
fn the_prefix_gets_the_runtime_s_steam_api_bridge() {
    let root = tempfile::tempdir().unwrap();
    let runtime = stock_runtime(root.path());
    fs::write(runtime.join(STEAM_CLIENT), "fixed bridge").unwrap();
    let prefix = root.path().join("prefix");
    fs::create_dir_all(prefix.join(PREFIX_STEAM_CLIENT).parent().unwrap()).unwrap();
    fs::write(prefix.join(PREFIX_STEAM_CLIENT), "stock bridge").unwrap();

    super::prefix(&runtime, &prefix).unwrap();
    assert_eq!(fs::read_to_string(prefix.join(PREFIX_STEAM_CLIENT)).unwrap(), "fixed bridge");
}

#[test]
fn the_guest_resolves_through_at_most_three_servers() {
    let root = tempfile::tempdir().unwrap();
    let runtime = stock_runtime(root.path());

    resolver(&runtime).unwrap();
    let written = fs::read_to_string(runtime.join(RESOLV_CONF)).unwrap();
    let servers: Vec<&str> = written
        .lines()
        .filter_map(|line| line.strip_prefix("nameserver "))
        .collect();
    assert!(!servers.is_empty() && servers.len() <= MAX_SERVERS, "{written}");
    assert!(servers.iter().all(|server| server.parse::<Ipv4Addr>().is_ok()));
    assert!(written.ends_with("options timeout:2 attempts:1\n"));
}

const ROUTER: Ipv4Addr = Ipv4Addr::new(192, 168, 1, 1);
const CLOUDFLARE: Ipv4Addr = PUBLIC_SERVERS[0];
const GOOGLE: Ipv4Addr = PUBLIC_SERVERS[1];
const QUAD9: Ipv4Addr = PUBLIC_SERVERS[2];

fn ms(milliseconds: u64) -> Duration {
    Duration::from_millis(milliseconds)
}

#[test]
fn the_mac_s_server_comes_first_then_the_fastest_public_ones() {
    let answered = [(QUAD9, ms(40)), (ROUTER, ms(90)), (GOOGLE, ms(20))];
    assert_eq!(chosen(&[ROUTER], &answered), [ROUTER, GOOGLE, QUAD9]);
}

#[test]
fn a_server_that_does_not_answer_is_left_out() {
    // Cloudflare, Voile's default, unreachable through a VPN.
    let answered = [(GOOGLE, ms(30))];
    assert_eq!(chosen(&[ROUTER], &answered), [GOOGLE]);
}

#[test]
fn offline_every_candidate_is_kept_for_the_network_to_come_back() {
    assert_eq!(chosen(&[ROUTER], &[]), [ROUTER, CLOUDFLARE, GOOGLE]);
    assert_eq!(chosen(&[], &[]), [CLOUDFLARE, GOOGLE, QUAD9]);
}

#[test]
fn the_conf_lists_each_server_then_short_timeouts() {
    assert_eq!(
        resolv_conf(&[ROUTER, GOOGLE]),
        "nameserver 192.168.1.1\nnameserver 8.8.8.8\noptions timeout:2 attempts:1\n"
    );
}
