use super::*;

/// An owner whose Transfers the test ends, and what the queue asked of it.
#[derive(Default)]
struct Fake {
    /// Transfers that may not start yet.
    blocked: Mutex<Vec<u64>>,
    log: Mutex<Vec<String>>,
}

impl Fake {
    fn log(&self) -> Vec<String> {
        self.log.lock().unwrap().clone()
    }
    fn note(&self, entry: String) {
        self.log.lock().unwrap().push(entry);
    }
}

impl Owner for Fake {
    fn ready(&self, id: u64) -> bool {
        !self.blocked.lock().unwrap().contains(&id)
    }
    fn start(self: Arc<Self>, id: u64) {
        self.note(format!("start:{id}"));
    }
    fn stop(&self, id: u64) {
        self.note(format!("stop:{id}"));
    }
    fn removed(&self, id: u64) {
        self.note(format!("removed:{id}"));
    }
}

fn request(game: &str, automatic: bool) -> Request {
    Request {
        storefront: Storefront::Steam,
        account: "42".into(),
        game: game.into(),
        name: game.into(),
        kind: if automatic {
            Kind::Update
        } else {
            Kind::Install
        },
        automatic,
    }
}

fn fixture() -> (Arc<DownloadQueue>, Arc<Fake>, Arc<dyn Owner>) {
    let fake = Arc::new(Fake::default());
    let owner: Arc<dyn Owner> = fake.clone();
    (DownloadQueue::new(), fake, owner)
}

fn running(queue: &DownloadQueue) -> Option<u64> {
    queue.view().running.map(|item| item.id)
}

fn waiting(queue: &DownloadQueue) -> Vec<(u64, bool)> {
    queue
        .view()
        .waiting
        .iter()
        .map(|item| (item.id, item.paused))
        .collect()
}

#[test]
fn transfers_run_one_at_a_time_in_their_order() {
    let (queue, fake, owner) = fixture();
    let first = queue.push(&owner, request("620", false));
    let second = queue.push(&owner, request("730", false));
    queue.advance();
    assert_eq!(running(&queue), Some(first));
    assert_eq!(waiting(&queue), [(second, false)]);

    assert!(!queue.finished(first), "a Transfer that ended leaves");
    assert_eq!(running(&queue), Some(second));
    assert_eq!(
        fake.log(),
        [format!("start:{first}"), format!("start:{second}")]
    );
    queue.finished(second);
    assert_eq!(queue.view().running, None);
}

#[test]
fn a_transfer_that_may_not_start_yet_lets_the_next_one_go() {
    let (queue, fake, owner) = fixture();
    let first = queue.push(&owner, request("620", false));
    let second = queue.push(&owner, request("730", false));
    fake.blocked.lock().unwrap().push(first);
    queue.advance();
    assert_eq!(running(&queue), Some(second));

    fake.blocked.lock().unwrap().clear();
    queue.finished(second);
    assert_eq!(running(&queue), Some(first));
}

#[test]
fn pausing_the_running_transfer_stops_it_and_it_waits_first_until_resumed() {
    let (queue, fake, owner) = fixture();
    let first = queue.push(&owner, request("620", false));
    let second = queue.push(&owner, request("730", false));
    queue.advance();
    queue.pause(first);
    assert_eq!(fake.log().last(), Some(&format!("stop:{first}")));

    assert!(queue.finished(first), "a paused Transfer stays queued");
    assert_eq!(running(&queue), Some(second));
    assert_eq!(waiting(&queue), [(first, true)]);
    queue.finished(second);
    assert_eq!(queue.view().running, None);

    queue.resume(first);
    assert_eq!(running(&queue), Some(first));
}

#[test]
fn a_running_game_holds_automatic_updates_and_stops_the_one_under_way() {
    let (queue, fake, owner) = fixture();
    let update = queue.push(&owner, request("620", true));
    let download = queue.push(&owner, request("730", false));
    queue.advance();
    assert_eq!(running(&queue), Some(update));

    queue.hold(&owner, true);
    assert_eq!(fake.log().last(), Some(&format!("stop:{update}")));
    assert!(queue.finished(update));
    assert_eq!(running(&queue), Some(download), "the player's own goes on");
    queue.finished(download);
    assert_eq!(queue.view().running, None);
    assert_eq!(waiting(&queue), [(update, false)]);

    queue.hold(&owner, false);
    assert_eq!(running(&queue), Some(update));
}

#[test]
fn automatic_updates_wait_until_no_storefront_runs_a_game() {
    let (queue, _, owner) = fixture();
    let (_, _, other) = fixture();
    let update = queue.push(&owner, request("620", true));
    queue.hold(&owner, true);
    queue.hold(&other, true);
    queue.hold(&owner, false);
    assert!(queue.view().held, "the other storefront still runs one");
    assert_eq!(running(&queue), None);

    queue.withdraw(&other);
    assert!(!queue.view().held);
    assert_eq!(running(&queue), Some(update));
}

#[test]
fn a_launch_goes_first_and_pauses_the_transfer_under_way_until_it_is_done() {
    let (queue, fake, owner) = fixture();
    let download = queue.push(&owner, request("730", false));
    queue.advance();
    let (launch, mut turn) = queue.push_first(&owner, request("620", false));
    assert_eq!(fake.log().last(), Some(&format!("stop:{download}")));
    assert!(
        turn.try_recv().is_err(),
        "the Transfer under way stops first"
    );
    queue.pause(launch);
    assert_eq!(
        waiting(&queue),
        [(launch, false)],
        "a launch's never pauses"
    );

    assert!(queue.finished(download));
    assert_eq!(running(&queue), Some(launch));
    assert!(turn.try_recv().is_ok());
    assert_eq!(waiting(&queue), [(download, false)]);
    queue.pause(launch);
    queue.hold(&owner, true);
    assert!(!fake.log().contains(&format!("stop:{launch}")));

    queue.finished(launch);
    assert_eq!(running(&queue), Some(download));
}

#[test]
fn a_launch_that_stopped_waiting_gives_its_turn_back() {
    let (queue, _, owner) = fixture();
    let download = queue.push(&owner, request("730", false));
    queue.advance();
    let (_, turn) = queue.push_first(&owner, request("620", false));
    drop(turn);
    queue.finished(download);
    assert_eq!(running(&queue), Some(download));
}

#[test]
fn removing_a_waiting_transfer_tells_its_owner_and_the_running_one_stops_for_good() {
    let (queue, fake, owner) = fixture();
    let first = queue.push(&owner, request("620", false));
    let second = queue.push(&owner, request("730", false));
    let third = queue.push(&owner, request("440", false));
    queue.advance();

    queue.remove(second);
    assert_eq!(fake.log().last(), Some(&format!("removed:{second}")));
    queue.remove(first);
    assert_eq!(fake.log().last(), Some(&format!("stop:{first}")));
    assert!(!queue.finished(first));
    assert_eq!(running(&queue), Some(third));
    assert!(queue.view().waiting.is_empty());
}

#[test]
fn moving_a_transfer_to_the_top_leaves_the_one_under_way() {
    let (queue, _, owner) = fixture();
    let first = queue.push(&owner, request("620", false));
    let second = queue.push(&owner, request("730", false));
    let third = queue.push(&owner, request("440", false));
    queue.advance();
    queue.move_to_top(third);
    assert_eq!(running(&queue), Some(first));
    assert_eq!(waiting(&queue), [(third, false), (second, false)]);
}

#[test]
fn a_signed_out_account_withdraws_every_transfer_and_a_dropped_owner_is_forgotten() {
    let (queue, fake, owner) = fixture();
    let first = queue.push(&owner, request("620", false));
    let second = queue.push(&owner, request("730", false));
    queue.advance();
    queue.withdraw(&owner);
    assert_eq!(
        fake.log(),
        [
            format!("start:{first}"),
            format!("stop:{first}"),
            format!("removed:{second}")
        ]
    );
    queue.finished(first);
    assert_eq!(queue.view().running, None);

    let (other, _, gone) = fixture();
    other.push(&gone, request("620", false));
    drop(gone);
    other.advance();
    assert_eq!(other.view().running, None);
    assert!(other.view().waiting.is_empty());
}

#[tokio::test]
async fn every_change_is_published() {
    let (queue, _, owner) = fixture();
    let before = queue.view().revision;
    queue.push(&owner, request("620", false));
    let view = queue.changed(before).await;
    assert_eq!(view.waiting.len(), 1);
}
