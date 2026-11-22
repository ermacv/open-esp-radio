use std::{sync::mpsc, time::Duration};

use super::{Answer, answer};

fn lines(text: &[&str]) -> mpsc::Receiver<Vec<u8>> {
    let (sender, received) = mpsc::channel();
    for line in text {
        sender.send(line.as_bytes().to_vec()).unwrap();
    }
    std::mem::forget(sender);
    received
}

#[test]
fn a_peer_answer_ends_at_its_ok_or_err_line() {
    let mut printed = Vec::new();
    let ok = answer(
        &lines(&["@READY", "@ERR n unknown", "@OK SYNC", "later"]),
        "SYNC",
        Duration::from_secs(1),
        &mut |text| printed.push(text.to_owned()),
    );
    assert_eq!(ok, Answer::Accepted);
    assert_eq!(printed, ["@READY", "@ERR n unknown", "@OK SYNC"]);
    let err = answer(
        &lines(&["@ERR BURST busy"]),
        "BURST",
        Duration::from_secs(1),
        &mut |_| {},
    );
    assert_eq!(err, Answer::Refused("@ERR BURST busy".to_owned()));
    let silent = answer(
        &lines(&["boot"]),
        "SYNC",
        Duration::from_millis(200),
        &mut |_| {},
    );
    assert_eq!(silent, Answer::Unanswered);
}
