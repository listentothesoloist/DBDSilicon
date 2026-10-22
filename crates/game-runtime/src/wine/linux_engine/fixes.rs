//! DBDSilicon's fixes to the pinned Voile runtime's Linux engine, without
//! which Dead by Daylight does not start or draw correctly. The app bundles
//! them as one archive of files laid out as the engine's runtime folder; each
//! launch puts them in place before the engine runs, once per archive and
//! runtime, so a runtime Voile installs again gets them too. See
//! `packaging/runtime-fixes` for what each file fixes and where its source is.
use crate::Error;
use std::{
    fs,
    net::{Ipv4Addr, SocketAddr, UdpSocket},
    path::Path,
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

/// The archive the runtime's fixes came from, by its SHA-256.
const MARKER: &str = ".dbdsilicon-fixes";
/// Valve's Steam API bridge, which `wineboot` copies into each prefix it creates.
const STEAM_CLIENT: &str = "wine/lib/wine/x86_64-windows/lsteamclient.dll";
const PREFIX_STEAM_CLIENT: &str = "drive_c/windows/system32/lsteamclient.dll";
/// The guest's resolver. Its libc reads at most three servers.
const RESOLV_CONF: &str = "root/etc/resolv.conf";
const MAX_SERVERS: usize = 3;
/// Public resolvers tried after the Mac's own: Cloudflare, Google and Quad9.
/// Voile's default, Cloudflare alone, cannot be reached on some networks and
/// VPNs, and Easy Anti-Cheat then fails to reach its servers.
const PUBLIC_SERVERS: [Ipv4Addr; 3] = [
    Ipv4Addr::new(1, 1, 1, 1),
    Ipv4Addr::new(8, 8, 8, 8),
    Ipv4Addr::new(9, 9, 9, 9),
];
/// How long a server has to answer before a launch leaves it out.
const PROBE_TIMEOUT: Duration = Duration::from_millis(700);

/// Puts the archive's files into `runtime`, unless they are already there,
/// and the guest's resolver in line with the Mac's. A build without the
/// archive, such as Obelisk's, leaves the runtime as Voile shipped it.
pub(super) fn runtime(archive: &Path, runtime: &Path) -> Result<(), Error> {
    if archive.is_file() {
        let version = crate::sha(archive)?;
        let marker = runtime.join(MARKER);
        if fs::read_to_string(&marker).ok().as_deref() != Some(version.as_str()) {
            // Unlinking first gives each binary a new file: macOS keeps a
            // replaced binary's old code signature for a file changed in place.
            let status = Command::new("/usr/bin/tar")
                .arg("-xJUf")
                .arg(archive)
                .arg("-C")
                .arg(runtime)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()?;
            if !status.success() {
                return Err(Error::Storage);
            }
            obelisk_core::write_atomic(&marker, version.as_bytes())?;
        }
    }
    resolver(runtime)
}

/// Gives the prefix the runtime's Steam API bridge, after `wineboot` copied
/// the one Voile shipped into it.
pub(super) fn prefix(runtime: &Path, prefix: &Path) -> Result<(), Error> {
    let fixed = runtime.join(STEAM_CLIENT);
    let installed = prefix.join(PREFIX_STEAM_CLIENT);
    if fixed.is_file() && installed.is_file() && crate::sha(&fixed)? != crate::sha(&installed)? {
        fs::copy(&fixed, &installed)?;
    }
    Ok(())
}

/// Writes the guest's resolv.conf with the servers that answer now: the
/// Mac's own first, in its order, then the public ones, fastest first. Each
/// is asked once, all at the same time. When none answers, as offline, the
/// Mac's and the public ones are all kept, for the network to come back.
fn resolver(runtime: &Path) -> Result<(), Error> {
    let path = runtime.join(RESOLV_CONF);
    if !path.parent().is_some_and(Path::is_dir) {
        return Ok(());
    }
    let mac = mac_servers();
    let candidates: Vec<Ipv4Addr> = mac
        .iter()
        .copied()
        .chain(PUBLIC_SERVERS.into_iter().filter(|server| !mac.contains(server)))
        .collect();
    let probes: Vec<_> = candidates
        .iter()
        .map(|&server| thread::spawn(move || answers(server).map(|time| (server, time))))
        .collect();
    let answered: Vec<(Ipv4Addr, Duration)> = probes
        .into_iter()
        .filter_map(|probe| probe.join().ok().flatten())
        .collect();
    let contents = resolv_conf(&chosen(&mac, &answered));
    if fs::read_to_string(&path).ok().as_deref() != Some(contents.as_str()) {
        obelisk_core::write_atomic(&path, contents.as_bytes())?;
    }
    Ok(())
}

/// The servers the guest uses: those that answered, the Mac's first, then
/// the public ones by how fast they answered; every candidate if none did.
fn chosen(mac: &[Ipv4Addr], answered: &[(Ipv4Addr, Duration)]) -> Vec<Ipv4Addr> {
    let mut servers: Vec<Ipv4Addr> = mac
        .iter()
        .copied()
        .filter(|server| answered.iter().any(|(known, _)| known == server))
        .collect();
    let mut public: Vec<&(Ipv4Addr, Duration)> = answered
        .iter()
        .filter(|(server, _)| !mac.contains(server))
        .collect();
    public.sort_by_key(|(_, time)| *time);
    servers.extend(public.into_iter().map(|(server, _)| *server));
    if servers.is_empty() {
        servers = mac
            .iter()
            .copied()
            .chain(PUBLIC_SERVERS.into_iter().filter(|server| !mac.contains(server)))
            .collect();
    }
    servers.truncate(MAX_SERVERS);
    servers
}

fn resolv_conf(servers: &[Ipv4Addr]) -> String {
    let mut contents: String = servers
        .iter()
        .map(|server| format!("nameserver {server}\n"))
        .collect();
    contents.push_str("options timeout:2 attempts:1\n");
    contents
}

/// How long `server` took to answer a DNS query for the root's name servers,
/// or `None` when it did not within `PROBE_TIMEOUT`.
fn answers(server: Ipv4Addr) -> Option<Duration> {
    let socket = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)).ok()?;
    socket.connect(SocketAddr::from((server, 53))).ok()?;
    let id = (std::process::id() as u16) ^ u16::from_be_bytes([server.octets()[2], server.octets()[3]]);
    let [high, low] = id.to_be_bytes();
    // Header: the id, recursion desired, one question. Question: the root, NS, IN.
    let query = [high, low, 0x01, 0x00, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 2, 0, 1];
    let started = Instant::now();
    socket.send(&query).ok()?;
    let mut reply = [0_u8; 512];
    while let Some(left) = PROBE_TIMEOUT.checked_sub(started.elapsed()) {
        socket.set_read_timeout(Some(left.max(Duration::from_millis(1)))).ok()?;
        let size = socket.recv(&mut reply).ok()?;
        // A reply to this query, from a server that serves it.
        if size >= 12 && reply[..2] == [high, low] && reply[2] & 0x80 != 0 {
            return Some(started.elapsed());
        }
    }
    None
}

/// The IPv4 DNS servers macOS lists, in its order. A loopback server is left
/// out: the guest may not reach a resolver that listens only on the Mac.
fn mac_servers() -> Vec<Ipv4Addr> {
    let Ok(output) = Command::new("/usr/sbin/scutil")
        .arg("--dns")
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
    else {
        return Vec::new();
    };
    let mut servers: Vec<Ipv4Addr> = Vec::new();
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        if !key.trim_start().starts_with("nameserver[") {
            continue;
        }
        if let Ok(server) = value.trim().parse::<Ipv4Addr>()
            && !server.is_loopback()
            && !server.is_unspecified()
            && !servers.contains(&server)
        {
            servers.push(server);
        }
    }
    servers
}

#[cfg(test)]
mod tests;
