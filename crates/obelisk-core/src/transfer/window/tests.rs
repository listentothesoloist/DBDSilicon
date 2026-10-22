use super::*;
use std::{
    cell::Cell,
    collections::BTreeSet,
    sync::atomic::{AtomicUsize, Ordering},
    time::Duration,
};

const WINDOW: Window = Window {
    width: 4,
    bytes: 100,
};

#[derive(Debug, PartialEq, Eq)]
enum Failed {
    Check,
    Receive,
}

struct Active<'a>(&'a Cell<usize>);
impl<'a> Active<'a> {
    fn new(active: &'a Cell<usize>, peak: &Cell<usize>) -> Self {
        active.set(active.get() + 1);
        peak.set(peak.get().max(active.get()));
        Self(active)
    }
}
impl Drop for Active<'_> {
    fn drop(&mut self) {
        self.0.set(self.0.get() - 1);
    }
}

#[tokio::test]
async fn runs_at_most_its_width_at_once_and_delivers_each_unit_once() {
    let active = Cell::new(0);
    let peak = Cell::new(0);
    let mut received = BTreeSet::new();
    WINDOW
        .run(
            0..WINDOW.width * 2 + 3,
            |_| 1,
            || Ok::<_, Failed>(()),
            |unit| {
                let (active, peak) = (&active, &peak);
                async move {
                    let _active = Active::new(active, peak);
                    tokio::task::yield_now().await;
                    Ok(unit)
                }
            },
            |unit| {
                assert!(received.insert(unit));
                Ok(())
            },
        )
        .await
        .unwrap();
    assert_eq!(received.len(), WINDOW.width * 2 + 3);
    assert_eq!(peak.get(), WINDOW.width);
    assert_eq!(active.get(), 0);
}

#[tokio::test]
async fn large_units_run_alone_and_dropping_cancels_every_transfer() {
    let active = Cell::new(0);
    let peak = Cell::new(0);
    let started = Cell::new(0);
    for size in [1, WINDOW.bytes + 1] {
        peak.set(0);
        started.set(0);
        let operation = WINDOW.run(
            0..WINDOW.width + 4,
            |_| size,
            || Ok::<_, Failed>(()),
            |_| {
                started.set(started.get() + 1);
                let (active, peak) = (&active, &peak);
                async move {
                    let _active = Active::new(active, peak);
                    std::future::pending::<()>().await;
                    Ok(())
                }
            },
            |()| panic!("a cancelled transfer must not deliver"),
        );
        assert!(
            tokio::time::timeout(Duration::from_millis(10), operation)
                .await
                .is_err()
        );
        let expected = if size > WINDOW.bytes { 1 } else { WINDOW.width };
        assert_eq!(peak.get(), expected);
        assert_eq!(started.get(), expected);
        assert_eq!(active.get(), 0);
    }
}

#[tokio::test]
async fn units_within_the_byte_budget_share_it() {
    let active = Cell::new(0);
    let peak = Cell::new(0);
    WINDOW
        .run(
            0..WINDOW.width * 2,
            |_| WINDOW.bytes / 2,
            || Ok::<_, Failed>(()),
            |_| {
                let (active, peak) = (&active, &peak);
                async move {
                    let _active = Active::new(active, peak);
                    tokio::task::yield_now().await;
                    Ok(())
                }
            },
            |()| Ok(()),
        )
        .await
        .unwrap();
    assert_eq!(peak.get(), 2);
}

#[tokio::test]
async fn transfers_overlap_instead_of_waiting_for_each_other() {
    let active = AtomicUsize::new(0);
    let peak = AtomicUsize::new(0);
    let mut received = 0;
    WINDOW
        .run(
            0..WINDOW.width * 2,
            |_| 1,
            || Ok::<_, Failed>(()),
            |_| {
                let (active, peak) = (&active, &peak);
                async move {
                    let now = active.fetch_add(1, Ordering::SeqCst) + 1;
                    peak.fetch_max(now, Ordering::SeqCst);
                    tokio::task::yield_now().await;
                    active.fetch_sub(1, Ordering::SeqCst);
                    Ok(())
                }
            },
            |()| {
                received += 1;
                Ok(())
            },
        )
        .await
        .unwrap();
    assert_eq!(received, WINDOW.width * 2);
    assert_eq!(peak.load(Ordering::SeqCst), WINDOW.width);
}

#[tokio::test]
async fn a_slow_unit_does_not_hold_back_the_units_after_it() {
    let started = Cell::new(0);
    let operation = WINDOW.run(
        0..WINDOW.width * 2,
        |_| 1,
        || Ok::<_, Failed>(()),
        |unit| {
            started.set(started.get() + 1);
            let started = &started;
            async move {
                // The first unit ends only once units past the first window
                // have started.
                while unit == 0 && started.get() <= WINDOW.width {
                    tokio::task::yield_now().await;
                }
                Ok(())
            }
        },
        |()| Ok(()),
    );
    tokio::time::timeout(Duration::from_secs(1), operation)
        .await
        .expect("the window refills while a unit is slow")
        .unwrap();
    assert_eq!(started.get(), WINDOW.width * 2);
}

#[tokio::test]
async fn failed_checks_and_receivers_stop_refills() {
    let started = Cell::new(0);
    let allowed = Cell::new(false);
    let check = || {
        if allowed.get() {
            Ok(())
        } else {
            Err(Failed::Check)
        }
    };
    let transfer = |_| {
        started.set(started.get() + 1);
        allowed.set(false);
        async { Ok(()) }
    };
    let result = WINDOW
        .run(
            0..WINDOW.width * 2,
            |_| 1,
            check,
            transfer,
            |()| panic!("a failed check must stop before receiving"),
        )
        .await;
    assert_eq!(result, Err(Failed::Check));
    assert_eq!(started.get(), 0);

    // A check that fails once transfers run stops before their results.
    allowed.set(true);
    let result = WINDOW
        .run(
            0..WINDOW.width * 2,
            |_| 1,
            check,
            transfer,
            |()| panic!("a failed check must stop before receiving"),
        )
        .await;
    assert_eq!(result, Err(Failed::Check));
    assert!((1..=WINDOW.width).contains(&started.get()));

    started.set(0);
    let result = WINDOW
        .run(
            0..WINDOW.width * 2,
            |_| 1,
            || Ok(()),
            |_| {
                started.set(started.get() + 1);
                async { Ok(()) }
            },
            |()| Err(Failed::Receive),
        )
        .await;
    assert_eq!(result, Err(Failed::Receive));
    // Nothing past the first window starts once the receiver failed.
    assert!(started.get() <= WINDOW.width);
}
