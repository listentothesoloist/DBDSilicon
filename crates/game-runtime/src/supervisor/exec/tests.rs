use super::*;
use std::fs;

#[test]
fn the_game_writes_its_log_after_the_header() {
    let directory = tempfile::tempdir().unwrap();
    let log = directory.path().join("wine-1245620.log");
    fs::write(&log, "last launch").unwrap();
    let admission = Admission {
        executable: "/bin/sh".into(),
        arguments: vec!["-c".into(), "echo game >&2".into()],
        working_directory: "/".into(),
        log: Some(log.clone()),
        log_header: Some("obelisk: engine=wine-macos\n".into()),
    };

    let status = Command::new(&admission.executable)
        .args(&admission.arguments)
        .stderr(standard_error(&admission).unwrap())
        .status()
        .unwrap();

    assert!(status.success());
    assert_eq!(
        fs::read_to_string(&log).unwrap(),
        "obelisk: engine=wine-macos\ngame\n"
    );
}
