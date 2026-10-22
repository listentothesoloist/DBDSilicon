//! The process's one download queue. Every storefront account's Transfers
//! wait in it and run one at a time, in its order: the player's downloads,
//! updates and repairs, Obelisk's automatic updates, and a launch's own
//! Transfer, which goes first and pauses the one under way. While a game
//! runs, whatever its storefront, automatic updates wait.
//!
//! The queue only orders: each Transfer's owner, an account's
//! installations, says when it may start, starts it and reports its end. The
//! queue never calls an owner while it holds its own lock, and owners never
//! call the queue while they hold theirs.
use std::sync::{Arc, Mutex, Weak};
use tokio::sync::{oneshot, watch};

/// The storefront a queued Transfer, or a game's diagnostics, belong to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Storefront {
    Steam,
    Epic,
    Gog,
}

/// What a queued Transfer does to its game.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// The game has no installation of that build yet.
    Install,
    /// The storefront offers a newer build.
    Update,
    /// The installed build's files are checked and repaired.
    Repair,
    /// The installation moves into the games folder.
    Move,
}

/// One Transfer in the queue, as the player sees it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Item {
    pub id: u64,
    pub storefront: Storefront,
    pub account: String,
    /// The game, as its storefront's installations name it.
    pub game: String,
    pub name: String,
    pub kind: Kind,
    /// Obelisk queued it for a newer build, not the player.
    pub automatic: bool,
    /// A launch waits for it: it goes first and cannot be paused.
    pub launching: bool,
    /// The player paused it: it waits until resumed.
    pub paused: bool,
}

/// Everything the queue presents. `revision` grows with every change.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct View {
    pub revision: u64,
    /// The Transfer under way.
    pub running: Option<Item>,
    /// The Transfers waiting, in the order they will run.
    pub waiting: Vec<Item>,
    /// A game runs: automatic updates wait.
    pub held: bool,
}

/// What an account's installations do for the Transfers they queued.
pub(crate) trait Owner: Send + Sync {
    /// Whether the Transfer `id` may start now.
    fn ready(&self, id: u64) -> bool;
    /// Starts the Transfer `id`; the owner reports `finished` once it ends.
    fn start(self: Arc<Self>, id: u64);
    /// Stops the running Transfer `id`; `finished` then reports its end.
    fn stop(&self, id: u64);
    /// The Transfer `id` left the queue without running.
    fn removed(&self, id: u64);
}

/// A Transfer an owner adds to the queue.
#[derive(Clone, Debug)]
pub(crate) struct Request {
    pub storefront: Storefront,
    pub account: String,
    pub game: String,
    pub name: String,
    pub kind: Kind,
    pub automatic: bool,
}

/// What becomes of a running Transfer the queue stopped, once it ends.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Stop {
    /// It waits first again, paused or not.
    Requeue { paused: bool },
    /// It leaves the queue.
    Remove,
}

struct Entry {
    item: Item,
    owner: Weak<dyn Owner>,
    /// A launch waits for its turn here instead of the owner starting it.
    turn: Option<oneshot::Sender<()>>,
}

struct Running {
    entry: Entry,
    stop: Option<Stop>,
}

#[derive(Default)]
struct State {
    revision: u64,
    next_id: u64,
    running: Option<Running>,
    waiting: Vec<Entry>,
    /// The owners whose storefront runs a game.
    holders: Vec<Weak<dyn Owner>>,
}

impl State {
    fn view(&self) -> View {
        View {
            revision: self.revision,
            running: self
                .running
                .as_ref()
                .map(|running| running.entry.item.clone()),
            waiting: self
                .waiting
                .iter()
                .map(|entry| entry.item.clone())
                .collect(),
            held: self.held(),
        }
    }

    fn held(&self) -> bool {
        !self.holders.is_empty()
    }

    /// Where a Transfer that goes first lands: behind the launches waiting.
    fn head(&self) -> usize {
        self.waiting
            .iter()
            .take_while(|entry| entry.item.launching)
            .count()
    }

    fn eligible(&self, item: &Item) -> bool {
        !item.paused && !(item.automatic && self.held())
    }

    /// Marks the running Transfer stopped, unless a launch waits for it,
    /// and returns what its owner must stop.
    fn stop_running(&mut self, how: Stop, which: impl Fn(&Item) -> bool) -> Stopping {
        let running = self.running.as_mut()?;
        let item = &running.entry.item;
        if running.stop.is_some() || item.launching || !which(item) {
            return None;
        }
        running.stop = Some(how);
        Some((item.id, running.entry.owner.clone()))
    }
}

/// A running Transfer's id and owner, which the queue asked to stop.
type Stopping = Option<(u64, Weak<dyn Owner>)>;

/// The process's one download queue.
pub struct DownloadQueue {
    state: Mutex<State>,
    published: watch::Sender<View>,
}

impl DownloadQueue {
    pub fn new() -> Arc<Self> {
        let state = State::default();
        let (published, _) = watch::channel(state.view());
        Arc::new(Self {
            state: Mutex::new(state),
            published,
        })
    }

    pub fn view(&self) -> View {
        self.published.borrow().clone()
    }

    /// Waits for a revision newer than `after`, or returns at once.
    pub async fn changed(&self, after: u64) -> View {
        let mut receiver = self.published.subscribe();
        let _ = receiver.wait_for(|view| view.revision > after).await;
        receiver.borrow().clone()
    }

    /// Changes the state and publishes it; `change` returns what to do once
    /// the lock is released.
    fn update<T>(&self, change: impl FnOnce(&mut State) -> T) -> T {
        let mut state = self.state.lock().unwrap();
        let before = state.view();
        let result = change(&mut state);
        let mut after = state.view();
        after.revision = before.revision;
        if after != before {
            state.revision += 1;
            after.revision = state.revision;
            self.published.send_replace(after);
        }
        result
    }

    fn entry(
        state: &mut State,
        owner: &Arc<dyn Owner>,
        request: Request,
        launching: bool,
    ) -> Entry {
        state.next_id += 1;
        Entry {
            item: Item {
                id: state.next_id,
                storefront: request.storefront,
                account: request.account,
                game: request.game,
                name: request.name,
                kind: request.kind,
                automatic: request.automatic,
                launching,
                paused: false,
            },
            owner: Arc::downgrade(owner),
            turn: None,
        }
    }

    /// Adds a Transfer behind the others and returns its id. It may start at
    /// the next `advance`, once its owner recorded it.
    pub(crate) fn push(&self, owner: &Arc<dyn Owner>, request: Request) -> u64 {
        self.update(|state| {
            let entry = Self::entry(state, owner, request, false);
            let id = entry.item.id;
            state.waiting.push(entry);
            id
        })
    }

    /// Puts a launch's Transfer first, pausing the one under way, and returns
    /// its id with the turn it waits for. Once its turn came, it runs until
    /// `finished`; removed before, its turn is dropped.
    pub(crate) fn push_first(
        &self,
        owner: &Arc<dyn Owner>,
        request: Request,
    ) -> (u64, oneshot::Receiver<()>) {
        let (sender, turn) = oneshot::channel();
        let (id, stopping) = self.update(|state| {
            let mut entry = Self::entry(state, owner, request, true);
            entry.turn = Some(sender);
            let id = entry.item.id;
            let head = state.head();
            state.waiting.insert(head, entry);
            let stopping = state.stop_running(Stop::Requeue { paused: false }, |_| true);
            (id, stopping)
        });
        stop(stopping);
        self.advance();
        (id, turn)
    }

    /// Ends the running Transfer `id`. When the queue stopped it to pause
    /// it, it waits first again: `true` then.
    pub(crate) fn finished(&self, id: u64) -> bool {
        let requeued = self.update(|state| {
            let running = state
                .running
                .take_if(|running| running.entry.item.id == id)?;
            let Some(Stop::Requeue { paused }) = running.stop else {
                return Some(false);
            };
            let mut entry = running.entry;
            entry.item.paused = paused;
            let head = state.head();
            state.waiting.insert(head, entry);
            Some(true)
        });
        self.advance();
        requeued.unwrap_or(false)
    }

    /// Takes the Transfer `id` out of the queue: a waiting one at once, the
    /// running one once its owner stopped it.
    pub fn remove(&self, id: u64) {
        let (removed, stopping) = self.update(|state| {
            if let Some(index) = state.waiting.iter().position(|entry| entry.item.id == id) {
                return (Some(state.waiting.remove(index).owner), None);
            }
            let stopping = state.running.as_mut().and_then(|running| {
                (running.entry.item.id == id && running.stop != Some(Stop::Remove)).then(|| {
                    running.stop = Some(Stop::Remove);
                    (id, running.entry.owner.clone())
                })
            });
            (None, stopping)
        });
        if let Some(owner) = removed.and_then(|owner| owner.upgrade()) {
            owner.removed(id);
        }
        stop(stopping);
        self.advance();
    }

    /// Takes every Transfer of `owner` out of the queue, and its hold, as
    /// its account signs out.
    pub(crate) fn withdraw(&self, owner: &Arc<dyn Owner>) {
        let ids: Vec<u64> = self.update(|state| {
            state.holders.retain(|holder| !owns(holder, owner));
            state
                .running
                .iter()
                .map(|running| &running.entry)
                .chain(state.waiting.iter())
                .filter(|entry| owns(&entry.owner, owner))
                .map(|entry| entry.item.id)
                .collect()
        });
        self.advance();
        for id in ids {
            self.remove(id);
        }
    }

    /// Pauses the Transfer `id`: a waiting one stays where it is, the running
    /// one stops and waits first. A launch's goes on.
    pub fn pause(&self, id: u64) {
        let stopping = self.update(|state| {
            if let Some(entry) = state.waiting.iter_mut().find(|entry| entry.item.id == id) {
                entry.item.paused = !entry.item.launching;
                return None;
            }
            state.stop_running(Stop::Requeue { paused: true }, |item| item.id == id)
        });
        stop(stopping);
        self.advance();
    }

    pub fn resume(&self, id: u64) {
        self.update(|state| {
            if let Some(entry) = state.waiting.iter_mut().find(|entry| entry.item.id == id) {
                entry.item.paused = false;
            }
        });
        self.advance();
    }

    /// Moves the waiting Transfer `id` first, behind any launch's. The one
    /// under way goes on.
    pub fn move_to_top(&self, id: u64) {
        self.update(|state| {
            let head = state.head();
            if let Some(index) = state.waiting.iter().position(|entry| entry.item.id == id)
                && index > head
            {
                let entry = state.waiting.remove(index);
                state.waiting.insert(head, entry);
            }
        });
    }

    /// Whether `owner`'s storefront runs a game. While one does, whatever its
    /// storefront, automatic updates wait, and one under way stops and waits
    /// first.
    pub(crate) fn hold(&self, owner: &Arc<dyn Owner>, holding: bool) {
        let stopping = self.update(|state| {
            state.holders.retain(|holder| !owns(holder, owner));
            if !holding {
                return None;
            }
            state.holders.push(Arc::downgrade(owner));
            state.stop_running(Stop::Requeue { paused: false }, |item| item.automatic)
        });
        stop(stopping);
        self.advance();
    }

    /// Starts the first waiting Transfer that may start, when none runs.
    /// Owners call it whenever what they let start may have changed.
    pub(crate) fn advance(&self) {
        loop {
            let candidates: Vec<(u64, Weak<dyn Owner>, bool)> = {
                let state = self.state.lock().unwrap();
                if state.running.is_some() {
                    return;
                }
                state
                    .waiting
                    .iter()
                    .filter(|entry| state.eligible(&entry.item))
                    .map(|entry| (entry.item.id, entry.owner.clone(), entry.item.launching))
                    .collect()
            };
            let mut chosen = None;
            let mut gone = Vec::new();
            for (id, owner, launching) in candidates {
                match owner.upgrade() {
                    None => gone.push(id),
                    Some(owner) if launching || owner.ready(id) => {
                        chosen = Some((id, owner));
                        break;
                    }
                    Some(_) => {}
                }
            }
            let claimed = self.update(|state| {
                state.waiting.retain(|entry| !gone.contains(&entry.item.id));
                let (id, _) = chosen.as_ref()?;
                if state.running.is_some() {
                    return None;
                }
                let index = state
                    .waiting
                    .iter()
                    .position(|entry| entry.item.id == *id)?;
                let mut entry = state.waiting.remove(index);
                let turn = entry.turn.take();
                state.running = Some(Running { entry, stop: None });
                Some(turn)
            });
            let Some((id, owner)) = chosen else { return };
            // Another caller started one, or the chosen one left: look again.
            let Some(turn) = claimed else { continue };
            match turn {
                // A launch that stopped waiting gives its turn back.
                Some(turn) => {
                    if turn.send(()).is_err() {
                        self.finished(id);
                    }
                }
                None => owner.start(id),
            }
            return;
        }
    }
}

fn owns(entry: &Weak<dyn Owner>, owner: &Arc<dyn Owner>) -> bool {
    std::ptr::addr_eq(entry.as_ptr(), Arc::as_ptr(owner))
}

/// Asks the owner of a running Transfer the queue stopped to stop it.
fn stop(stopping: Stopping) {
    if let Some((id, owner)) = stopping
        && let Some(owner) = owner.upgrade()
    {
        owner.stop(id);
    }
}

#[cfg(test)]
mod tests;
