use super::*;
use std::collections::VecDeque;

struct Serial {
    responses: VecDeque<Vec<u8>>,
    pending: VecDeque<u8>,
    requests: Vec<String>,
}
impl Read for Serial {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        if self.pending.is_empty() {
            return Err(std::io::Error::from(std::io::ErrorKind::UnexpectedEof));
        }
        let count = buffer.len().min(self.pending.len());
        for byte in &mut buffer[..count] {
            *byte = self.pending.pop_front().unwrap();
        }
        Ok(count)
    }
}
impl Write for Serial {
    fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
        self.requests
            .push(String::from_utf8(data.to_vec()).unwrap());
        self.pending.extend(
            self.responses
                .pop_front()
                .expect("one scripted reply per command"),
        );
        Ok(data.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
fn register(address: u32) -> Register {
    Register {
        mask: u32::MAX,
        address,
        name: format!("r{address}"),
    }
}
fn serial(responses: &[&str]) -> Serial {
    Serial {
        responses: responses.iter().map(|s| s.as_bytes().to_vec()).collect(),
        pending: VecDeque::new(),
        requests: Vec::new(),
    }
}
#[test]
fn discovered_unreadable_reads_are_not_reissued_in_the_measured_boot() {
    let mut io = serial(&["oer-vendor-calibration-register 00000004 00000009\n"]);
    let known = Readings {
        values: [(4, 1)].into(),
        unreadable: [8].into(),
    };
    let mut log = String::new();
    vendor_registers(
        &mut io,
        Space::Mmio,
        &[register(4), register(8)],
        Lifecycle::Cold,
        &mut log,
        Some(&known),
    )
    .unwrap();
    assert_eq!(io.requests, ["r 00000004\n"]);
    let readings = Readings::parse(Space::Mmio, &log).unwrap();
    assert_eq!(readings.values, [(4, 9)].into());
    assert_eq!(readings.unreadable, [8].into());
}
#[test]
fn a_new_boot_aborts_measurement_but_discovery_may_recover() {
    let mut io = serial(&["oer-vendor-calibration-end\n"]);
    let known = Readings {
        values: [(4, 1)].into(),
        unreadable: Default::default(),
    };
    let mut log = String::new();
    let result = vendor_registers(
        &mut io,
        Space::Mmio,
        &[register(4)],
        Lifecycle::Cold,
        &mut log,
        Some(&known),
    );
    assert!(
        result
            .unwrap_err()
            .to_string()
            .contains("capture would span boots")
    );
    assert!(log.contains("oer-vendor-calibration-end"));
    let mut io = serial(&["oer-vendor-calibration-end\n"]);
    let mut log = String::new();
    vendor_registers(
        &mut io,
        Space::Mmio,
        &[register(4)],
        Lifecycle::Cold,
        &mut log,
        None,
    )
    .unwrap();
    assert_eq!(
        Readings::parse(Space::Mmio, &log).unwrap().unreadable,
        [4].into()
    );
}
#[test]
fn discovery_failure_or_an_unknown_address_never_becomes_a_skipped_read() {
    let mut io = serial(&[]);
    let mut log = String::new();
    assert!(
        vendor_registers(
            &mut io,
            Space::Mmio,
            &[register(4)],
            Lifecycle::Cold,
            &mut log,
            Some(&Readings::default())
        )
        .is_err()
    );
    assert!(io.requests.is_empty());
    let known = Readings {
        values: [(4, 1)].into(),
        unreadable: [4].into(),
    };
    assert!(
        vendor_registers(
            &mut io,
            Space::Mmio,
            &[register(4)],
            Lifecycle::Cold,
            &mut log,
            Some(&known)
        )
        .is_err()
    );
    assert!(io.requests.is_empty());
}
