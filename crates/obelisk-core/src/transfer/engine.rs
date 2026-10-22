//! The chunk engine every storefront runs between the network and the disk.
//! It owns the queues between the two sides, the blocking worker's
//! registration and the cancellation that ties them together. A storefront
//! keeps its own assembly, resume strategy and mirror policy.
use std::future::Future;

use futures_util::{Stream, StreamExt, stream};
use tokio::sync::mpsc;

use super::{CancelToken, Cancelled};

/// The order fetched chunks reach [`Chunks::receive`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Delivery {
    /// Request order, for a disk side that writes and hashes sequentially.
    InOrder,
    /// As each chunk arrives, for a disk side that places chunks itself: a
    /// slow chunk then holds up nothing fetched after it.
    AsFetched,
}

/// How hard one engine pulls on the network.
#[derive(Clone, Copy, Debug)]
pub struct Pipeline {
    pub delivery: Delivery,
    /// Chunk fetches in flight at once. At least one.
    pub fetches: usize,
    /// Fetched chunks waiting for the disk side. Bounds decoded memory. At
    /// least one.
    pub queue: usize,
}

/// The disk worker ended without an outcome: it panicked, or the runtime shut
/// down under it. Each storefront turns it into its own vocabulary.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Interrupted;

/// The disk side's end of a running engine. Only [`run_engine`] makes one; it
/// lives on the blocking pool with the disk closure. Dropping it stops the
/// network side.
pub struct Chunks<C, T, E> {
    requests: mpsc::UnboundedSender<C>,
    results: mpsc::Receiver<Result<T, E>>,
    refused: bool,
}

impl<C, T, E> Chunks<C, T, E> {
    /// This end, with the network side's: the requests it takes up and the
    /// queue it delivers outcomes to.
    fn open(queue: usize) -> (Self, mpsc::UnboundedReceiver<C>, mpsc::Sender<Result<T, E>>) {
        let (requests, taken) = mpsc::unbounded_channel();
        let (deliveries, results) = mpsc::channel(queue);
        let chunks = Self {
            requests,
            results,
            refused: false,
        };
        (chunks, taken, deliveries)
    }

    /// A disk side's end with no engine behind it, for a test that plays the
    /// network side: it takes requests and queues outcomes itself, and closes
    /// its request receiver where the engine would stop.
    #[cfg(any(test, feature = "testing"))]
    pub fn detached(
        queue: usize,
    ) -> (Self, mpsc::UnboundedReceiver<C>, mpsc::Sender<Result<T, E>>) {
        Self::open(queue)
    }
}

impl<C, T, E: From<Cancelled>> Chunks<C, T, E> {
    /// Queues `chunk` for the network side. Requests are unbounded: the disk
    /// side bounds how far it plans ahead.
    ///
    /// `false` once the network side has stopped, after a chunk failed for
    /// good or the Transfer was cancelled. The request is then dropped and
    /// there is nothing to handle: the outcome that stopped the network side
    /// is already queued ahead of anything requested after it, so `receive`
    /// returns it no later than in this request's place. Every later request
    /// is refused too, without reaching the network side; see [`Self::refused`].
    pub fn request(&mut self, chunk: C) -> bool {
        self.refused = self.refused || self.requests.send(chunk).is_err();
        !self.refused
    }

    /// Whether a request was refused. A disk side stops planning more work
    /// then: the plan holding the refused request already reaches the outcome
    /// that stopped the network side. Seen only through a refusal, never
    /// earlier, so that a disk side never stops with nothing planned that
    /// would receive the outcome.
    pub fn refused(&self) -> bool {
        self.refused
    }

    /// Blocks for the next fetched chunk in the pipeline's delivery order. In
    /// its place comes the error that stopped the network side, or `Cancelled`.
    /// Also `Cancelled` when the engine's future was dropped, which cancels
    /// its token. Waits only for what this side requested: with nothing
    /// requested it blocks until the Transfer is cancelled. Call it from the
    /// disk closure only, never from async code.
    pub fn receive(&mut self) -> Result<T, E> {
        // The pump closes this queue only after the outcome that stopped it,
        // so a closed queue means the engine's future was dropped. The token
        // may not be cancelled yet at that point, so it is not consulted.
        self.results
            .blocking_recv()
            .unwrap_or_else(|| Err(Cancelled.into()))
    }
}

/// Runs one content engine and returns the disk side's outcome once both
/// sides have ended.
///
/// `disk` runs on the blocking pool, registered with `cancel`, so the Transfer
/// keeps its admission until the worker really stops. It must check `cancel`
/// at its usual points.
///
/// `fetch` runs on this task with at most `pipeline.fetches` chunks in flight.
/// It is called once per request, synchronously and in request order, when the
/// request is taken up and before its future is first polled. Work done in the
/// closure before it returns the future, such as numbering a mirror ticket,
/// keeps request order. It may borrow anything that outlives this call.
/// A fetch error ends the network side once it is delivered. So does a
/// cancellation, which is delivered as `Cancelled` so that a disk side blocked
/// in `receive` wakes, and it wins over a chunk that is ready. When `disk`
/// returns early, fetches still in flight are dropped, and no request it left
/// queued starts once the network side sees it gone.
///
/// Dropping the returned future cancels `cancel`: the disk worker keeps
/// running and stops at its next check. A disk worker that panics returns
/// `Interrupted`.
pub async fn run_engine<C, T, R, E, F>(
    pipeline: Pipeline,
    cancel: &CancelToken,
    disk: impl FnOnce(Chunks<C, T, E>) -> Result<R, E> + Send + 'static,
    fetch: impl Fn(C) -> F,
) -> Result<R, E>
where
    C: Send + 'static,
    T: Send + 'static,
    R: Send + 'static,
    E: From<Cancelled> + From<Interrupted> + Send + 'static,
    F: Future<Output = Result<T, E>>,
{
    // A zero window never pulls a request, leaving the disk side blocked for good.
    assert!(pipeline.fetches > 0 && pipeline.queue > 0);
    let (chunks, taken, deliveries) = Chunks::open(pipeline.queue);
    let guard = cancel.cancel_on_drop();
    let (outcome, ()) = tokio::join!(
        cancel.spawn_blocking(move || disk(chunks)),
        pump(taken, deliveries, pipeline, cancel, fetch),
    );
    guard.disarm();
    outcome.map_err(|_| E::from(Interrupted))?
}

/// Fetches requested chunks within the pipeline's window and delivers them in
/// its order. Stops as soon as the disk side goes away, a chunk fails for good,
/// or cancellation is requested.
async fn pump<C, T, E, F>(
    queue: mpsc::UnboundedReceiver<C>,
    deliveries: mpsc::Sender<Result<T, E>>,
    pipeline: Pipeline,
    cancel: &CancelToken,
    fetch: impl Fn(C) -> F,
) where
    F: Future<Output = Result<T, E>>,
    E: From<Cancelled>,
{
    let requested = queued(queue).map(fetch);
    let fetched = match pipeline.delivery {
        Delivery::InOrder => requested.buffered(pipeline.fetches).left_stream(),
        Delivery::AsFetched => requested.buffer_unordered(pipeline.fetches).right_stream(),
    };
    deliver(fetched, deliveries, cancel).await;
}

fn queued<C>(requests: mpsc::UnboundedReceiver<C>) -> impl Stream<Item = C> {
    stream::unfold(requests, |mut requests| async move {
        requests.recv().await.map(|chunk| (chunk, requests))
    })
}

async fn deliver<T, E: From<Cancelled>>(
    fetched: impl Stream<Item = Result<T, E>>,
    deliveries: mpsc::Sender<Result<T, E>>,
    cancel: &CancelToken,
) {
    tokio::pin!(fetched);
    loop {
        // Polling the stream refills the fetch window, so a gone disk side or
        // a cancellation must be seen first, or one more fetch would start.
        let next = tokio::select! {
            biased;
            () = deliveries.closed() => return,
            () = cancel.cancelled() => {
                let _ = deliveries.send(Err(Cancelled.into())).await;
                return;
            }
            result = fetched.next() => result,
        };
        let Some(result) = next else { return };
        let failed = result.is_err();
        if deliveries.send(result).await.is_err() || failed {
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        sync::{
            Arc, Mutex,
            atomic::{AtomicUsize, Ordering},
            mpsc as std_mpsc,
        },
        time::{Duration, Instant},
    };
    use tokio::sync::Notify;

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum Failure {
        Cancelled,
        Interrupted,
        Chunk(usize),
    }

    impl From<Cancelled> for Failure {
        fn from(_: Cancelled) -> Self {
            Self::Cancelled
        }
    }

    impl From<Interrupted> for Failure {
        fn from(_: Interrupted) -> Self {
            Self::Interrupted
        }
    }

    type TestChunks = Chunks<usize, usize, Failure>;

    const WAIT: Duration = Duration::from_secs(2);

    fn pipeline(delivery: Delivery, fetches: usize, queue: usize) -> Pipeline {
        Pipeline {
            delivery,
            fetches,
            queue,
        }
    }

    /// One gate per request, so a test decides exactly when each fetch finishes.
    fn gates(count: usize) -> Arc<Vec<Notify>> {
        Arc::new((0..count).map(|_| Notify::new()).collect())
    }

    /// Polls `done` from the disk side until it holds or the deadline passes.
    fn wait_until(done: impl Fn() -> bool) -> bool {
        let deadline = Instant::now() + WAIT;
        while !done() {
            if Instant::now() > deadline {
                return false;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        true
    }

    #[tokio::test]
    async fn chunks_reach_an_in_order_disk_side_in_request_order_however_they_finish() {
        let open = gates(3);
        let fetch_gates = Arc::clone(&open);
        let (finished, finishes) = std_mpsc::channel();
        let cancel = CancelToken::new();
        let disk = move |mut chunks: TestChunks| {
            for index in 0..3 {
                assert!(chunks.request(index));
            }
            // Finish backwards, each fetch only after the later one is done.
            for index in [2_usize, 1, 0] {
                open[index].notify_one();
                assert_eq!(finishes.recv_timeout(WAIT), Ok(index));
            }
            (0..3)
                .map(|_| chunks.receive())
                .collect::<Result<Vec<_>, _>>()
        };
        let order = run_engine(
            pipeline(Delivery::InOrder, 3, 3),
            &cancel,
            disk,
            |index: usize| {
                let (gates, finished) = (Arc::clone(&fetch_gates), finished.clone());
                async move {
                    gates[index].notified().await;
                    finished.send(index).unwrap();
                    Ok(index)
                }
            },
        )
        .await;
        assert_eq!(
            order,
            Ok(vec![0, 1, 2]),
            "the disk side writes in manifest order"
        );
    }

    #[tokio::test]
    async fn a_slow_chunk_holds_up_nothing_after_it_when_delivered_as_fetched() {
        let slow = Arc::new(Notify::new());
        let release = Arc::clone(&slow);
        let cancel = CancelToken::new();
        let disk = move |mut chunks: TestChunks| {
            for index in 0..3 {
                assert!(chunks.request(index));
            }
            let mut early = [chunks.receive()?, chunks.receive()?];
            early.sort_unstable();
            // Chunk 0 finishes only once both chunks after it arrived.
            release.notify_one();
            Ok((early, chunks.receive()?))
        };
        let received = run_engine(
            pipeline(Delivery::AsFetched, 3, 3),
            &cancel,
            disk,
            |index: usize| {
                let slow = Arc::clone(&slow);
                async move {
                    if index == 0 {
                        slow.notified().await;
                    }
                    Ok(index)
                }
            },
        )
        .await;
        assert_eq!(received, Ok(([1, 2], 0)));
    }

    #[tokio::test]
    async fn no_more_than_pipeline_fetches_are_in_flight() {
        const REQUESTS: usize = 6;
        let open = gates(REQUESTS);
        let fetch_gates = Arc::clone(&open);
        let started = Arc::new(AtomicUsize::new(0));
        let active = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));
        let observed = Arc::clone(&started);
        let cancel = CancelToken::new();
        let disk = move |mut chunks: TestChunks| {
            for index in 0..REQUESTS {
                assert!(chunks.request(index));
            }
            let mut received = Vec::new();
            for index in 0..REQUESTS {
                // Hold each chunk until the next one is in flight too, so the
                // window is full whenever it can be.
                let in_flight = (index + 2).min(REQUESTS);
                assert!(wait_until(|| observed.load(Ordering::Acquire) >= in_flight));
                open[index].notify_one();
                received.push(chunks.receive()?);
            }
            Ok(received)
        };
        let counters = (Arc::clone(&started), Arc::clone(&active), Arc::clone(&peak));
        // A queue smaller than the window still completes.
        let received = run_engine(
            pipeline(Delivery::InOrder, 2, 1),
            &cancel,
            disk,
            |index: usize| {
                let gates = Arc::clone(&fetch_gates);
                let (started, active, peak) = (
                    Arc::clone(&counters.0),
                    Arc::clone(&counters.1),
                    Arc::clone(&counters.2),
                );
                async move {
                    let running = active.fetch_add(1, Ordering::AcqRel) + 1;
                    peak.fetch_max(running, Ordering::AcqRel);
                    started.fetch_add(1, Ordering::AcqRel);
                    gates[index].notified().await;
                    active.fetch_sub(1, Ordering::AcqRel);
                    Ok(index)
                }
            },
        )
        .await;
        assert_eq!(received, Ok((0..REQUESTS).collect()));
        assert_eq!(peak.load(Ordering::Acquire), 2, "CDN requests stay bounded");
        assert_eq!(active.load(Ordering::Acquire), 0);
    }

    #[tokio::test]
    async fn fetch_is_called_in_request_order_before_its_future_runs() {
        const REQUESTS: usize = 6;
        let calls = Arc::new(Mutex::new(Vec::new()));
        let cancel = CancelToken::new();
        let disk = |mut chunks: TestChunks| {
            for index in 0..REQUESTS {
                assert!(chunks.request(index));
            }
            let mut received = (0..REQUESTS)
                .map(|_| chunks.receive())
                .collect::<Result<Vec<_>, _>>()?;
            received.sort_unstable();
            Ok(received)
        };
        let received = run_engine(
            pipeline(Delivery::AsFetched, 3, 3),
            &cancel,
            disk,
            |index: usize| {
                calls.lock().unwrap().push(index);
                let calls = Arc::clone(&calls);
                async move {
                    assert!(
                        calls.lock().unwrap().contains(&index),
                        "the closure ran before its future"
                    );
                    // Later requests finish sooner.
                    for _ in index..REQUESTS {
                        tokio::task::yield_now().await;
                    }
                    Ok(index)
                }
            },
        )
        .await;
        assert_eq!(received, Ok((0..REQUESTS).collect()));
        assert_eq!(
            *calls.lock().unwrap(),
            (0..REQUESTS).collect::<Vec<_>>(),
            "mirror tickets are numbered in request order"
        );
    }

    #[tokio::test]
    async fn a_cancellation_wakes_a_disk_side_blocked_in_receive() {
        let fetching = Notify::new();
        let cancel = CancelToken::new();
        let disk = |mut chunks: TestChunks| {
            assert!(chunks.request(0));
            chunks.receive()
        };
        // This fetch never finishes on its own: only the cancellation ends it.
        let engine = run_engine(
            pipeline(Delivery::InOrder, 1, 1),
            &cancel,
            disk,
            |_: usize| {
                fetching.notify_one();
                std::future::pending()
            },
        );
        let cancelling = async {
            fetching.notified().await;
            cancel.cancel();
        };
        let (outcome, ()) = tokio::join!(engine, cancelling);
        assert_eq!(outcome, Err(Failure::Cancelled));
    }

    #[tokio::test]
    async fn a_failed_chunk_is_received_in_its_place_and_stops_fetching() {
        let started = Arc::new(AtomicUsize::new(0));
        let cancel = CancelToken::new();
        let disk = |mut chunks: TestChunks| {
            for index in 0..4 {
                assert!(chunks.request(index));
            }
            Ok([chunks.receive(), chunks.receive()])
        };
        let received = run_engine(
            pipeline(Delivery::InOrder, 1, 4),
            &cancel,
            disk,
            |index: usize| {
                let started = Arc::clone(&started);
                async move {
                    started.fetch_add(1, Ordering::AcqRel);
                    if index == 1 {
                        return Err(Failure::Chunk(1));
                    }
                    Ok(index)
                }
            },
        )
        .await;
        assert_eq!(received, Ok([Ok(0), Err(Failure::Chunk(1))]));
        assert_eq!(
            started.load(Ordering::Acquire),
            2,
            "a chunk that fails for good stops the queue"
        );
    }

    async fn request_after_the_network_side_stopped(delivery: Delivery) {
        let cancel = CancelToken::new();
        let disk = move |mut chunks: TestChunks| {
            let first = if delivery == Delivery::InOrder {
                assert!(chunks.request(0));
                assert!(chunks.request(1));
                Some(chunks.receive())
            } else {
                assert!(chunks.request(1));
                None
            };
            let stopped = wait_until(|| chunks.requests.is_closed());
            let unrefused = !chunks.refused();
            // No retry: the first refused request is the one that matters.
            let accepted = chunks.request(10);
            Ok((
                first,
                stopped && unrefused,
                accepted,
                chunks.refused(),
                chunks.receive(),
            ))
        };
        let outcome = run_engine(
            pipeline(delivery, 2, 2),
            &cancel,
            disk,
            |index: usize| async move {
                match index {
                    0 => Ok(0),
                    1 => Err(Failure::Chunk(1)),
                    _ => std::future::pending().await,
                }
            },
        )
        .await;
        let expected_first = (delivery == Delivery::InOrder).then_some(Ok(0));
        assert_eq!(
            outcome,
            Ok((expected_first, true, false, true, Err(Failure::Chunk(1)))),
            "{delivery:?}: the request is dropped and the queued error still arrives"
        );
    }

    #[tokio::test]
    async fn a_request_after_the_network_side_stopped_is_dropped_and_the_original_error_still_arrives()
     {
        request_after_the_network_side_stopped(Delivery::InOrder).await;
        request_after_the_network_side_stopped(Delivery::AsFetched).await;
    }

    #[tokio::test]
    async fn a_disk_side_that_returns_early_stops_the_network_side() {
        let started = Arc::new(AtomicUsize::new(0));
        let cancel = CancelToken::new();
        let disk = |mut chunks: TestChunks| {
            for index in 0..8 {
                assert!(chunks.request(index));
            }
            chunks.receive()?;
            Ok(7)
        };
        let outcome = run_engine(
            pipeline(Delivery::InOrder, 2, 1),
            &cancel,
            disk,
            |index: usize| {
                let started = Arc::clone(&started);
                async move {
                    started.fetch_add(1, Ordering::AcqRel);
                    Ok(index)
                }
            },
        )
        .await;
        assert_eq!(outcome, Ok(7));
        assert!(
            started.load(Ordering::Acquire) <= 3,
            "no chunk is fetched for a disk side that is already gone"
        );
    }

    #[tokio::test]
    async fn no_fetch_starts_once_the_disk_side_is_gone() {
        let calls = AtomicUsize::new(0);
        let cancel = CancelToken::new();
        let hold = Arc::new(Notify::new());
        let (held, holding) = std_mpsc::channel();
        let (gone, going) = std_mpsc::channel();
        let disk = {
            let hold = Arc::clone(&hold);
            move |mut chunks: TestChunks| {
                assert!(chunks.request(0));
                assert_eq!(chunks.receive(), Ok(0));
                // The pump is idle with a free window. Queue more work and go
                // while its task cannot run, so it next wakes to both.
                hold.notify_one();
                holding.recv_timeout(WAIT).unwrap();
                for index in 1..8 {
                    assert!(chunks.request(index));
                }
                drop(chunks);
                gone.send(()).unwrap();
                Ok(7)
            }
        };
        let (outcome, ()) = tokio::join!(
            run_engine(
                pipeline(Delivery::InOrder, 2, 1),
                &cancel,
                disk,
                |index: usize| {
                    calls.fetch_add(1, Ordering::AcqRel);
                    async move { Ok(index) }
                },
            ),
            async {
                hold.notified().await;
                held.send(()).unwrap();
                // Blocks this task, and with it the pump, until the disk side
                // has dropped its end.
                going.recv_timeout(WAIT).unwrap();
            },
        );
        assert_eq!(outcome, Ok(7));
        assert_eq!(calls.load(Ordering::Acquire), 1);
    }

    #[tokio::test]
    async fn a_disk_side_with_nothing_to_request_returns_its_value() {
        let calls = AtomicUsize::new(0);
        let cancel = CancelToken::new();
        let outcome = run_engine(
            pipeline(Delivery::InOrder, 1, 1),
            &cancel,
            |_: TestChunks| Ok(5),
            |index: usize| {
                calls.fetch_add(1, Ordering::AcqRel);
                async move { Ok(index) }
            },
        )
        .await;
        assert_eq!(outcome, Ok(5));
        assert_eq!(calls.load(Ordering::Acquire), 0);
        assert!(!cancel.is_cancelled(), "a finished engine cancels nothing");
    }

    #[tokio::test]
    async fn dropping_the_engine_cancels_the_token_and_the_worker_reads_cancelled() {
        let cancel = CancelToken::new();
        let fetching = Arc::new(Notify::new());
        let (received, receipts) = std_mpsc::channel();
        let task = {
            let (cancel, fetching) = (cancel.clone(), Arc::clone(&fetching));
            tokio::spawn(async move {
                let disk = move |mut chunks: TestChunks| {
                    assert!(chunks.request(0));
                    received.send(chunks.receive()).unwrap();
                    Ok(())
                };
                run_engine(
                    pipeline(Delivery::InOrder, 1, 1),
                    &cancel,
                    disk,
                    move |_: usize| {
                        fetching.notify_one();
                        std::future::pending()
                    },
                )
                .await
            })
        };
        // The disk side requested its chunk and nothing will ever answer it.
        fetching.notified().await;
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        assert!(cancel.is_cancelled());
        assert_eq!(receipts.recv_timeout(WAIT), Ok(Err(Failure::Cancelled)));
        tokio::time::timeout(WAIT, cancel.wait_for_workers())
            .await
            .expect("the disk worker stops");
    }

    #[tokio::test]
    async fn a_panicking_disk_worker_returns_interrupted() {
        let cancel = CancelToken::new();
        let outcome = run_engine(
            pipeline(Delivery::InOrder, 1, 1),
            &cancel,
            |_: TestChunks| -> Result<(), Failure> { panic!("the disk worker failed") },
            |index: usize| async move { Ok(index) },
        )
        .await;
        assert_eq!(outcome, Err(Failure::Interrupted));
        assert!(!cancel.is_cancelled());
    }
}
