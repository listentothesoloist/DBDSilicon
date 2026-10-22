//! Revisioned snapshot observation shared by the friends and credential watches.
use tokio::sync::watch;

/// Waits until the observed value satisfies `is_ready`, checking the current
/// value before waiting so a change between the two cannot be missed.
/// `Err` carries the last value once every sender has been dropped.
pub async fn next_matching<T: Clone>(
    mut receiver: watch::Receiver<T>,
    is_ready: impl Fn(&T) -> bool,
) -> Result<T, T> {
    loop {
        let value = receiver.borrow_and_update().clone();
        if is_ready(&value) {
            return Ok(value);
        }
        if receiver.changed().await.is_err() {
            return Err(receiver.borrow().clone());
        }
    }
}

/// A revision that grows with each change, such as a credential rotation,
/// until it is stopped. Clones share one revision.
#[derive(Clone, Debug)]
pub struct Revisions(watch::Sender<RevisionState>);

#[derive(Clone, Copy, Debug)]
struct RevisionState {
    revision: u64,
    stopped: bool,
}

impl Revisions {
    /// Starts at revision 1.
    pub fn new() -> Self {
        Self(
            watch::channel(RevisionState {
                revision: 1,
                stopped: false,
            })
            .0,
        )
    }

    /// The revision now.
    pub fn current(&self) -> u64 {
        self.0.borrow().revision
    }

    /// The first revision other than `after`; `None` once stopped.
    pub async fn next(&self, after: u64) -> Option<u64> {
        next_matching(self.0.subscribe(), |state| {
            state.stopped || state.revision != after
        })
        .await
        .ok()
        .filter(|state| !state.stopped)
        .map(|state| state.revision)
    }

    /// Moves on to the next revision.
    pub fn advance(&self) {
        self.0
            .send_modify(|state| state.revision = state.revision.wrapping_add(1));
    }

    /// Ends every wait for a next revision, now and later.
    pub fn stop(&self) {
        self.0.send_modify(|state| state.stopped = true);
    }
}

impl Default for Revisions {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn returns_current_value_when_already_ready_and_last_value_on_close() {
        let (sender, _) = watch::channel(1_u64);
        assert_eq!(next_matching(sender.subscribe(), |v| *v >= 1).await, Ok(1));
        let waiting = tokio::spawn(next_matching(sender.subscribe(), |v| *v >= 2));
        sender.send_replace(2);
        assert_eq!(waiting.await.unwrap(), Ok(2));
        let closing = tokio::spawn(next_matching(sender.subscribe(), |v| *v >= 3));
        tokio::task::yield_now().await;
        drop(sender);
        assert_eq!(closing.await.unwrap(), Err(2));
    }
}
