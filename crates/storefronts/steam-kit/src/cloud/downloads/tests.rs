use super::*;
use sha1::{Digest, Sha1};
use std::cell::Cell;

fn files(count: usize) -> Vec<CloudFile> {
    (0..count)
        .map(|i| CloudFile {
            name: i.to_string(),
            sha1: crate::content::hex(&Sha1::digest(b"save")),
            size: 4,
            timestamp: 1,
            persist_state: 0,
            platforms: 7,
            reupload_requested: false,
        })
        .collect()
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
async fn sixteen_small_files_or_one_large_file_are_read_at_once() {
    let active = Cell::new(0);
    let peak = Cell::new(0);
    let requested = Cell::new(0);
    for large in [false, true] {
        let mut files = files(40);
        if large {
            for file in &mut files {
                file.size = 32 * 1024 * 1024 + 1;
            }
        }
        peak.set(0);
        requested.set(0);
        let operation = download_files(
            &files,
            |_| {
                requested.set(requested.get() + 1);
                std::future::ready(Ok(()))
            },
            |_, ()| async {
                let _active = Active::new(&active, &peak);
                std::future::pending::<()>().await;
                Ok(Vec::new())
            },
            &|| true,
            |_, _| panic!("cancelled read must not deliver bytes"),
        );
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(10), operation)
                .await
                .is_err()
        );
        let expected = if large { 1 } else { 16 };
        assert_eq!(peak.get(), expected);
        assert_eq!(requested.get(), expected);
        assert_eq!(active.get(), 0);
    }
}

#[tokio::test]
async fn revocation_corruption_and_receiver_failure_stop_the_batch() {
    let authorized = Cell::new(true);
    let requests = Cell::new(0);
    let result = download_files(
        &files(8),
        |_| {
            authorized.set(false);
            std::future::ready(Ok(()))
        },
        |_, ()| async {
            requests.set(requests.get() + 1);
            Ok(b"save".to_vec())
        },
        &|| authorized.get(),
        |_, _| panic!("revoked transfer"),
    )
    .await;
    assert!(matches!(result, Err(CloudError::Revoked)));
    assert_eq!(requests.get(), 0);

    let result = download_files(
        &files(8),
        |_| std::future::ready(Ok(())),
        |_, ()| async { Ok(b"bad!".to_vec()) },
        &|| true,
        |_, _| panic!("unverified bytes"),
    )
    .await;
    assert!(matches!(result, Err(CloudError::Integrity)));

    requests.set(0);
    let result = download_files(
        &files(32),
        |_| {
            requests.set(requests.get() + 1);
            std::future::ready(Ok(()))
        },
        |_, ()| async { Ok(b"save".to_vec()) },
        &|| true,
        |_, _| Err(CloudError::Storage),
    )
    .await;
    assert!(matches!(result, Err(CloudError::Storage)));
    // Nothing past the first window is requested once the receiver failed.
    assert!(requests.get() <= 16);
}
