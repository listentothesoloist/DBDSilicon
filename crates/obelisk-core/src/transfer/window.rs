use futures_util::{StreamExt, stream::FuturesUnordered};
use std::future::Future;

/// How many transfers of one storefront overlap, and how many bytes they may
/// hold at once.
#[derive(Clone, Copy, Debug)]
pub struct Window {
    pub width: usize,
    pub bytes: u64,
}

impl Window {
    /// Runs `transfer` on each unit, at most `width` at once and within
    /// `bytes`; a unit larger than `bytes` runs alone. Results reach `receive`
    /// as they complete, so a slow unit holds back nothing after it. `check`
    /// runs before each unit starts, before each result is received and once
    /// at the end. The first error stops the window, and dropping the future
    /// cancels every transfer in flight; no detached worker outlives it.
    pub async fn run<U, T, E, F: Future<Output = Result<T, E>>>(
        self,
        units: impl IntoIterator<Item = U>,
        size: impl Fn(&U) -> u64,
        check: impl Fn() -> Result<(), E>,
        transfer: impl Fn(U) -> F,
        mut receive: impl FnMut(T) -> Result<(), E>,
    ) -> Result<(), E> {
        let transfer = &transfer;
        let mut queued = units.into_iter().peekable();
        let mut pending = FuturesUnordered::new();
        let mut bytes = 0_u64;
        loop {
            // A large unit runs alone. Smaller ones overlap without holding
            // the whole set in memory.
            while let Some(unit) = queued.next_if(|unit| {
                pending.is_empty()
                    || pending.len() < self.width && bytes.saturating_add(size(unit)) <= self.bytes
            }) {
                check()?;
                let unit_size = size(&unit);
                bytes = bytes.saturating_add(unit_size);
                pending.push(async move { (unit_size, transfer(unit).await) });
            }
            let Some((unit_size, result)) = pending.next().await else {
                break;
            };
            let value = result?;
            bytes = bytes.saturating_sub(unit_size);
            check()?;
            receive(value)?;
        }
        check()
    }
}

#[cfg(test)]
mod tests;
