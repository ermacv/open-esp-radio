extern crate std;
use super::*;
use std::{collections::VecDeque, vec, vec::Vec};

struct Socket {
    input: VecDeque<Result<&'static [u8], ()>>,
    written: Vec<u8>,
    write_limit: usize,
    drains: VecDeque<bool>,
    operations: Vec<&'static str>,
}

impl Socket {
    fn new(input: Vec<Result<&'static [u8], ()>>, drains: Vec<bool>) -> Self {
        Self {
            input: input.into(),
            written: Vec::new(),
            write_limit: 2,
            drains: drains.into(),
            operations: Vec::new(),
        }
    }
}

impl Connection for Socket {
    async fn read(&mut self, bytes: &mut [u8]) -> Result<usize, ()> {
        let input = self.input.pop_front().expect("scripted read")?;
        bytes[..input.len()].copy_from_slice(input);
        Ok(input.len())
    }
    async fn write(&mut self, bytes: &[u8]) -> Result<usize, ()> {
        let count = bytes.len().min(self.write_limit);
        self.written.extend_from_slice(&bytes[..count]);
        Ok(count)
    }
    fn close(&mut self) {
        self.operations.push("close");
    }
    fn abort(&mut self) {
        self.operations.push("abort");
    }
    async fn drain(&mut self) -> bool {
        self.operations.push("drain");
        self.drains.pop_front().expect("scripted drain")
    }
}

fn run(socket: &mut Socket) -> Completion {
    use core::{
        future::Future,
        task::{Context, Poll, Waker},
    };
    let mut bytes = [0; 32];
    let mut session = core::pin::pin!(session(socket, &mut bytes));
    match session
        .as_mut()
        .poll(&mut Context::from_waker(Waker::noop()))
    {
        Poll::Ready(result) => result,
        Poll::Pending => panic!("scripted IO completes synchronously"),
    }
}

#[test]
fn peer_half_close_drains_all_echo_bytes_before_reuse() {
    let mut socket = Socket::new(vec![Ok(b"last reply"), Ok(b"")], vec![true]);
    assert!(matches!(run(&mut socket), Completion::Closed));
    assert_eq!(socket.written, b"last reply");
    assert_eq!(socket.operations, ["close", "drain"]);
}

#[test]
fn io_failure_and_zero_writes_abort_and_drain_reset() {
    for input in [vec![Err(())], vec![Ok(&b"cannot write"[..])]] {
        let mut socket = Socket::new(input, vec![true]);
        socket.write_limit = 0;
        assert!(matches!(run(&mut socket), Completion::Aborted));
        assert_eq!(socket.operations, ["abort", "drain"]);
    }
}

#[test]
fn unconfirmed_close_attempts_reset_before_retiring_socket() {
    for reset_drained in [false, true] {
        let mut socket = Socket::new(vec![Ok(b"last"), Ok(b"")], vec![false, reset_drained]);
        let result = run(&mut socket);
        assert_eq!(matches!(result, Completion::Aborted), reset_drained);
        assert_eq!(
            matches!(result, Completion::ResetUnconfirmed),
            !reset_drained
        );
        assert_eq!(socket.operations, ["close", "drain", "abort", "drain"]);
    }
}
