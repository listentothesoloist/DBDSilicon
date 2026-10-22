use super::*;

/// A library and another place to choose, both canonical.
struct Places {
    _root: tempfile::TempDir,
    library: PathBuf,
    elsewhere: PathBuf,
}

fn places() -> Places {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().canonicalize().unwrap();
    let (library, elsewhere) = (path.join("library"), path.join("elsewhere"));
    fs::create_dir_all(&library).unwrap();
    fs::create_dir_all(&elsewhere).unwrap();
    Places {
        _root: root,
        library,
        elsewhere,
    }
}

/// An account's installations, as the games folder sees them.
#[derive(Default)]
struct Account {
    installations: Vec<(PathBuf, u64)>,
    gathered: Mutex<Vec<PathBuf>>,
    refreshed: Mutex<u32>,
}

impl Resident for Account {
    fn installations(&self) -> Vec<(PathBuf, u64)> {
        self.installations.clone()
    }
    fn gather(self: Arc<Self>, folder: &Path) {
        self.gathered.lock().unwrap().push(folder.to_owned());
    }
    fn refresh(&self) {
        *self.refreshed.lock().unwrap() += 1;
    }
}

fn registered(folder: &GamesFolder, account: Account) -> Arc<Account> {
    let account = Arc::new(account);
    let resident: Arc<dyn Resident> = account.clone();
    folder.register(Arc::downgrade(&resident));
    account
}

#[test]
fn obelisk_s_own_folder_holds_the_games_until_the_player_chooses_another() {
    let places = places();
    let folder = GamesFolder::new(&places.library);
    let view = folder.view();
    assert_eq!(view.path, places.library.join("Games"));
    assert!(view.default);
    assert!(
        folder.connected(),
        "Obelisk's own folder is made when needed"
    );
}

#[test]
fn a_chosen_folder_gets_obelisk_s_own_folder_inside_it_and_stays_chosen() {
    let places = places();
    let folder = GamesFolder::new(&places.library);
    let account = registered(&folder, Account::default());
    let before = folder.view().revision;
    folder.choose(&places.elsewhere).unwrap();
    let chosen = places.elsewhere.join(CHOSEN_DIRECTORY);
    let view = folder.view();
    assert_eq!(view.path, chosen);
    assert!(!view.default && view.revision > before);
    assert!(chosen.is_dir());
    assert_eq!(
        *account.gathered.lock().unwrap(),
        std::slice::from_ref(&chosen)
    );
    assert_eq!(GamesFolder::new(&places.library).path(), chosen);
    // Choosing Obelisk's folder there again keeps it, not one inside it.
    folder.choose(&chosen).unwrap();
    assert_eq!(folder.path(), chosen);
}

#[test]
fn obelisk_s_own_folder_chosen_again_forgets_the_player_s_choice() {
    let places = places();
    let folder = GamesFolder::new(&places.library);
    folder.choose(&places.elsewhere).unwrap();
    folder.choose_default().unwrap();
    assert!(folder.view().default);
    assert!(!places.library.join(FILE).exists());
    assert!(GamesFolder::new(&places.library).view().default);
}

#[test]
fn a_folder_inside_obelisk_s_library_or_a_game_or_missing_is_refused() {
    let places = places();
    let folder = GamesFolder::new(&places.library);
    let game = places.elsewhere.join("Games/steam-620-macos");
    fs::create_dir_all(&game).unwrap();
    let _account = registered(
        &folder,
        Account {
            installations: vec![(game.clone(), 10)],
            ..Account::default()
        },
    );
    fs::create_dir_all(places.library.join("Voile")).unwrap();
    assert_eq!(
        folder.inspect(&places.library.join("Voile")),
        Err(Refusal::Inside)
    );
    assert_eq!(folder.inspect(&game), Err(Refusal::Inside));
    assert_eq!(
        folder.inspect(&places.elsewhere.join("missing")),
        Err(Refusal::Missing)
    );
    assert!(matches!(
        folder.choose(&game),
        Err(ChoiceFailure::Refused(Refusal::Inside))
    ));
    assert!(folder.view().default);
}

#[test]
fn inspecting_a_folder_counts_each_installation_outside_it_once() {
    let places = places();
    let folder = GamesFolder::new(&places.library);
    let inside = places
        .elsewhere
        .join(CHOSEN_DIRECTORY)
        .join("Steam/steam-1-macos");
    let outside = places.library.join("Games/Steam/steam-2-macos");
    fs::create_dir_all(&inside).unwrap();
    fs::create_dir_all(&outside).unwrap();
    let _first = registered(
        &folder,
        Account {
            installations: vec![(inside, 5), (outside.clone(), 7)],
            ..Account::default()
        },
    );
    let _second = registered(
        &folder,
        Account {
            installations: vec![(outside, 7)],
            ..Account::default()
        },
    );
    let candidate = folder.inspect(&places.elsewhere).unwrap();
    assert_eq!(candidate.path, places.elsewhere.join(CHOSEN_DIRECTORY));
    assert_eq!(candidate.games, 1);
    // The same disk: the installation only changes its place.
    assert_eq!(candidate.copied_bytes, 0);
}

#[test]
fn a_disk_change_refreshes_every_account_and_a_move_finds_its_installation() {
    let places = places();
    let folder = GamesFolder::new(&places.library);
    let account = registered(&folder, Account::default());
    folder.volumes_changed();
    assert_eq!(*account.refreshed.lock().unwrap(), 1);
    // The chosen folder's disk left: the view tells it.
    folder.choose(&places.elsewhere).unwrap();
    assert!(folder.view().connected);
    fs::remove_dir_all(&places.elsewhere).unwrap();
    let before = folder.view().revision;
    folder.volumes_changed();
    let view = folder.view();
    assert!(!view.connected && view.revision > before);
    folder.choose_default().unwrap();
    let moved = places.library.join("Games/Steam/steam-620-macos");
    fs::create_dir_all(&moved).unwrap();
    assert_eq!(
        folder.installation(
            "Steam",
            Path::new("/Volumes/Gone/Obelisk Games/Steam/steam-620-macos")
        ),
        Some(moved)
    );
    assert_eq!(
        folder.installation("Steam", Path::new("/elsewhere/steam-570-macos")),
        None
    );
}

#[test]
fn only_obelisk_s_emptied_folders_are_tidied() {
    let places = places();
    let folder = GamesFolder::new(&places.library);
    let old = places.elsewhere.join(CHOSEN_DIRECTORY);
    fs::create_dir_all(old.join("Steam")).unwrap();
    fs::create_dir_all(old.join("Epic/epic-game")).unwrap();
    folder.tidy(&old.join("Steam"));
    assert!(!old.join("Steam").exists());
    assert!(old.exists(), "Epic's games are still there");
    let player = places.elsewhere.join("Jeux");
    fs::create_dir_all(player.join("Steam")).unwrap();
    folder.tidy(&player.join("Steam"));
    assert!(player.join("Steam").exists(), "not Obelisk's folder");
    // The games folder itself stays, even emptied.
    fs::create_dir_all(places.library.join("Games/Steam")).unwrap();
    folder.tidy(&places.library.join("Games/Steam"));
    assert!(places.library.join("Games").exists());
}

#[test]
fn a_copy_an_interrupted_move_left_goes_at_the_next_start() {
    let places = places();
    let left = places
        .library
        .join("Games/Steam/.steam-620-macos.obelisk-moving");
    let game = places.library.join("Games/Steam/steam-570-macos");
    fs::create_dir_all(&left).unwrap();
    fs::create_dir_all(&game).unwrap();
    GamesFolder::new(&places.library);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while left.exists() {
        assert!(std::time::Instant::now() < deadline, "the copy stayed");
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert!(game.exists());
}

#[test]
fn only_a_missing_external_disk_is_disconnected() {
    assert!(disconnected(Path::new(
        "/Volumes/Obelisk Test Disk That Is Not There/Obelisk Games/Steam/steam-620-macos"
    )));
    assert!(!disconnected(Path::new(
        "/Users/someone/Games/steam-620-macos"
    )));
    assert!(!disconnected(Path::new("/Volumes")));
}
