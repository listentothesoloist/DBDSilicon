//! Force quitting a game for its player: the game, what it started, and what
//! still runs for its installation or its Wine prefix. A process ends only
//! when it runs this game's or Voile's files and uses this game's files, so
//! another game and Obelisk itself never end with it.
use crate::wine;
use std::{
    collections::HashMap,
    io::Read,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::Duration,
};

/// A process listing still running after this long is abandoned.
const LISTING_TIMEOUT: Duration = Duration::from_secs(10);

/// What ending the spawned game needs to know of its launch.
pub(crate) struct Game {
    /// The environment the launch gave the game, before its host's own.
    pub environment: HashMap<String, String>,
    pub installation: Option<PathBuf>,
    pub program: Option<PathBuf>,
}

/// The processes a game may leave behind: they run files from one of `runs`,
/// use files under one of `uses`, and run nothing from `spared`.
#[derive(Debug, Default, PartialEq)]
pub(crate) struct Scope {
    runs: Vec<PathBuf>,
    uses: Vec<PathBuf>,
    spared: Vec<PathBuf>,
}

impl Scope {
    /// The scope of `game`, whose launch helper is `pid`, for a supervisor
    /// running `own`.
    pub(crate) fn of(
        game: &Game,
        wine: Option<&wine::Config>,
        pid: Option<u32>,
        own: &Path,
    ) -> Self {
        let mut scope = Self::default();
        if let Some(installation) = &game.installation {
            scope.runs.push(installation.clone());
            scope.uses.push(installation.clone());
        }
        if let Some(wine) = wine {
            scope.runs.push(wine.runtime.clone());
            scope
                .uses
                .extend(wine::private_folders(wine, &game.environment, pid));
        }
        scope.spared.push(bundle(own));
        // lsof names files by their real path, such as /private/tmp for /tmp.
        for paths in [&mut scope.runs, &mut scope.uses, &mut scope.spared] {
            let real: Vec<_> = paths
                .iter()
                .filter_map(|path| path.canonicalize().ok())
                .filter(|path| !paths.contains(path))
                .collect();
            paths.extend(real);
        }
        scope
    }
}

/// The application bundle `executable` belongs to, else its folder.
fn bundle(executable: &Path) -> PathBuf {
    executable
        .ancestors()
        .filter(|folder| {
            folder
                .extension()
                .is_some_and(|extension| extension == "app")
        })
        .last()
        .or(executable.parent())
        .unwrap_or(executable)
        .to_owned()
}

/// The processes of an `lsof -Fpfn` listing inside `scope`, except `own`.
pub(crate) fn matching(listing: &str, scope: &Scope, own: u32) -> Vec<u32> {
    #[derive(Default)]
    struct Seen {
        runs: bool,
        uses: bool,
        spared: bool,
    }
    fn settle(current: Option<(u32, Seen)>, own: u32, found: &mut Vec<u32>) {
        if let Some((pid, seen)) = current
            && seen.runs
            && seen.uses
            && !seen.spared
            && pid != own
            && !found.contains(&pid)
        {
            found.push(pid);
        }
    }
    let under = |path: &Path, roots: &[PathBuf]| roots.iter().any(|root| path.starts_with(root));
    let mut found = Vec::new();
    let mut current = None;
    // lsof lists what a process runs, its executable and libraries, as txt.
    let mut runs = false;
    for line in listing.lines() {
        if let Some(pid) = line.strip_prefix('p') {
            settle(current.take(), own, &mut found);
            current = pid.parse().ok().map(|pid| (pid, Seen::default()));
        } else if let Some(descriptor) = line.strip_prefix('f') {
            runs = descriptor == "txt";
        } else if let Some(name) = line.strip_prefix('n')
            && let Some((_, seen)) = &mut current
        {
            let path = Path::new(name);
            if runs {
                seen.runs |= under(path, &scope.runs);
                seen.spared |= under(path, &scope.spared);
            }
            seen.uses |= under(path, &scope.uses);
        }
    }
    settle(current, own, &mut found);
    found
}

/// Every descendant of `root` in a `ps -o pid=,ppid=` listing.
pub(crate) fn descendants(listing: &str, root: u32) -> Vec<u32> {
    let parents: Vec<(u32, u32)> = listing
        .lines()
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            Some((fields.next()?.parse().ok()?, fields.next()?.parse().ok()?))
        })
        .collect();
    let mut found = Vec::new();
    let mut pending = vec![root];
    while let Some(parent) = pending.pop() {
        for &(pid, ppid) in &parents {
            if ppid == parent && pid != root && !found.contains(&pid) {
                found.push(pid);
                pending.push(pid);
            }
        }
    }
    found
}

/// Everything the game's helper `pid` started, listed while it lives: once
/// it dies, they belong to launchd.
pub(crate) fn started_by(pid: u32) -> Vec<u32> {
    let mut command = Command::new("/bin/ps");
    command.args(["-axo", "pid=,ppid="]);
    listing(command)
        .map(|listing| descendants(&listing, pid))
        .unwrap_or_default()
}

/// Kills whatever still runs inside `scope`.
pub(crate) fn sweep(scope: &Scope) {
    let mut command = Command::new("/usr/sbin/lsof");
    command.args(["-n", "-P", "-w", "-Fpfn"]);
    if let Some(listing) = listing(command) {
        kill(&matching(&listing, scope, std::process::id()));
    }
}

pub(crate) fn kill(pids: &[u32]) {
    if pids.is_empty() {
        return;
    }
    let _ = Command::new("/bin/kill")
        .arg("-KILL")
        .args(pids.iter().map(u32::to_string))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

/// What `command` prints, unless it fails or outlasts `LISTING_TIMEOUT`.
pub(crate) fn listing(mut command: Command) -> Option<String> {
    let mut child = command
        .env_clear()
        .env("LC_ALL", "C")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let mut output = child.stdout.take()?;
    // Read apart: a full pipe would otherwise keep the command from exiting.
    let reading = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        output.read_to_end(&mut bytes).map(|_| bytes)
    });
    let code = wine::wait(child, LISTING_TIMEOUT).ok().flatten();
    let bytes = reading.join().ok()?.ok()?;
    code.map(|_| String::from_utf8_lossy(&bytes).into_owned())
}

#[cfg(test)]
mod tests;
