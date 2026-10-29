//! The image's startup order, independent of its hardware binding.

/// Keep the initialized owners while reporting the start of the runtime.
/// An initialization refusal publishes no diagnostic through an absent console.
pub(crate) fn initialize<R, E>(
    install: impl FnOnce() -> Result<R, E>,
    report: impl FnOnce(),
) -> Result<R, E> {
    let runtime = install()?;
    report();
    Ok(runtime)
}

#[cfg(test)]
mod tests {
    use core::cell::{Cell, RefCell};

    use oer_diagnostics::{Console, Fifo, FifoWriter, Lost, Note, Patience};

    /// Before the root installs its console parts, the early FIFO has no room.
    struct EarlyFifo<'a> {
        installed: &'a Cell<bool>,
        bytes: &'a RefCell<Vec<u8>>,
    }

    impl Fifo for EarlyFifo<'_> {
        fn has_room(&self) -> bool {
            self.installed.get()
        }

        fn push(&mut self, byte: u8) {
            self.bytes.borrow_mut().push(byte);
        }

        fn flush(&mut self) {}
    }

    #[test]
    fn first_boot_records_reach_the_host_without_loss_or_a_sequence_gap() {
        let installed = Cell::new(false);
        let bytes = RefCell::new(Vec::new());
        let lost = Lost::new();
        let mut writer = FifoWriter::new(
            EarlyFifo {
                installed: &installed,
                bytes: &bytes,
            },
            &lost,
            Patience::NONE,
        );
        let mut console = Console::new(|chunk: &[u8]| {
            writer.text(chunk);
            Ok::<_, ()>(())
        });
        let owner = super::initialize(
            || {
                installed.set(true);
                Ok::<_, ()>(Box::new(42))
            },
            || {
                console
                    .note(&Note::runtime_start(oer_diagnostics::name!("test"; 64)))
                    .unwrap();
                console.note(&Note::placement_pass()).unwrap();
            },
        )
        .unwrap();

        assert_eq!(*owner, 42);
        assert_eq!(lost.take(), 0);
        let bytes = bytes.into_inner();
        let mut stream = oer_diagnostics::render::Stream::new();
        let mut scratch = [0; oer_diagnostics::MAX_PAYLOAD + 7];
        let mut records = 0;
        for line in bytes.split(|byte| *byte == b'\n') {
            let line = line.strip_suffix(b"\r").unwrap_or(line);
            if let Some(record) = stream.line(line, &mut scratch).unwrap() {
                assert!(record.gap.is_none());
                records += 1;
            }
        }
        assert_eq!(records, 2);
    }

    #[test]
    fn initialization_refusal_returns_its_owner_without_reporting_startup() {
        let error_owner = Box::new(42);
        let address = &*error_owner as *const i32;
        let reported = Cell::new(false);
        let result = super::initialize(|| Err::<(), _>(error_owner), || reported.set(true));
        assert_eq!(&*result.unwrap_err() as *const i32, address);
        assert!(!reported.get());
    }
}
