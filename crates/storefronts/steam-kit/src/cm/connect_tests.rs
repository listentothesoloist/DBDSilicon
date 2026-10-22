use super::first_to_answer;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::time::{Instant, sleep};

const STAGGER: Duration = Duration::from_millis(50);

#[tokio::test]
async fn a_server_that_never_answers_does_not_hold_up_the_next() {
    let start = Instant::now();

    let answer = first_to_answer(
        vec!["silent".into(), "failing".into(), "answering".into()],
        STAGGER,
        |server| async move {
            match server.as_str() {
                "silent" => {
                    sleep(Duration::from_secs(30)).await;
                    Some("silent")
                }
                "failing" => None,
                _ => {
                    sleep(Duration::from_millis(10)).await;
                    Some("answering")
                }
            }
        },
    )
    .await;

    assert_eq!(answer, Some("answering"));
    assert!(
        start.elapsed() < Duration::from_secs(1),
        "{:?}",
        start.elapsed()
    );
}

#[tokio::test]
async fn a_server_that_answers_in_time_is_the_only_one_tried() {
    let tried = Arc::new(Mutex::new(Vec::new()));

    let answer = first_to_answer(vec!["first".into(), "second".into()], STAGGER, |server| {
        tried.lock().unwrap().push(server.clone());
        async move {
            sleep(Duration::from_millis(5)).await;
            Some(server)
        }
    })
    .await;

    assert_eq!(answer.as_deref(), Some("first"));
    assert_eq!(*tried.lock().unwrap(), ["first"]);
}

#[tokio::test]
async fn no_server_answering_gives_nothing() {
    let answer = first_to_answer::<(), _, _>(
        vec!["one".into(), "two".into(), "three".into()],
        STAGGER,
        |_| async { None },
    )
    .await;

    assert_eq!(answer, None);
    assert_eq!(
        first_to_answer::<(), _, _>(Vec::new(), STAGGER, |_| async { None }).await,
        None
    );
}
