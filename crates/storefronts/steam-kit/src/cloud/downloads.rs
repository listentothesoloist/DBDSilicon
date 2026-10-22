use super::{CloudError, CloudFile};
use obelisk_core::transfer::Window;

/// Each file waits about 200 ms for its CM request before its HTTP body, so
/// only overlapping files makes a save set of hundreds of them fast.
const WINDOW: Window = Window {
    width: 16,
    bytes: 32 * 1024 * 1024,
};

/// CM requests and HTTP bodies overlap in one sliding window and are consumed
/// as each file completes. Dropping this future cancels every in-flight read;
/// no detached worker can outlive the transfer owner.
pub(crate) async fn download_files<P, R, D>(
    files: &[CloudFile],
    request: impl Fn(CloudFile) -> R,
    download: impl Fn(CloudFile, P) -> D,
    authorized: &impl Fn() -> bool,
    mut receive: impl FnMut(&CloudFile, Vec<u8>) -> Result<(), CloudError>,
) -> Result<(), CloudError>
where
    R: Future<Output = Result<P, CloudError>>,
    D: Future<Output = Result<Vec<u8>, CloudError>>,
{
    let (request, download) = (&request, &download);
    WINDOW
        .run(
            files,
            |file| u64::from(file.size),
            || check(authorized),
            |file| async move {
                let plan = request(file.clone()).await?;
                check(authorized)?;
                let read = download(file.clone(), plan).await?;
                check(authorized)?;
                file.verify(&read)?;
                Ok((file, read))
            },
            |(file, read)| receive(file, read),
        )
        .await
}

fn check(authorized: &impl Fn() -> bool) -> Result<(), CloudError> {
    if authorized() {
        Ok(())
    } else {
        Err(CloudError::Revoked)
    }
}

#[cfg(test)]
mod tests;
