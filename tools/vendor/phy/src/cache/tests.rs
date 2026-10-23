#![allow(clippy::unwrap_used)]
use super::*;
use oer_time::Clock as _;
use oer_time_virtual::VirtualClock;
use std::cell::RefCell;

struct Fake {
    clock: VirtualClock,
    requests: RefCell<Vec<(&'static str, u64, u64, Duration)>>,
    replies: RefCell<std::collections::VecDeque<serde_json::Value>>,
    cost_ms: u32,
}
impl Fake {
    fn new(replies: Vec<serde_json::Value>) -> Self {
        Self {
            clock: VirtualClock::new(),
            requests: RefCell::new(vec![]),
            replies: RefCell::new(replies.into()),
            cost_ms: 1,
        }
    }
}
impl Transport for Fake {
    fn now(&self) -> Duration {
        Duration::from_micros(self.clock.now().as_micros())
    }
    fn request<E: Endpoint>(
        &self,
        boot: u64,
        session: u64,
        _body: E,
        timeout: Duration,
    ) -> Result<E::Response> {
        self.requests
            .borrow_mut()
            .push((E::PATH, boot, session, timeout));
        self.clock
            .advance(oer_time::Duration::from_millis(self.cost_ms))
            .unwrap();
        Ok(serde_json::from_value(
            self.replies
                .borrow_mut()
                .pop_front()
                .ok_or("missing fake reply")?,
        )?)
    }
}
fn target() -> serde_json::Value {
    serde_json::json!({"identity":[1,2],"format":1,"schema":8,"algorithm_revision":1,"profile":1})
}
fn identity() -> serde_json::Value {
    serde_json::json!({"target":target(),"active_ticket":null})
}
fn awaiting() -> serde_json::Value {
    serde_json::json!({"ticket":7,"state":{"AwaitingCache":{"target":target(),"remaining_ms":500}}})
}
fn ready() -> serde_json::Value {
    serde_json::json!({"ticket":7,"state":{"Ready":{"decision":{"NoCache":"Missing"},"registration_us":100,
    "capture":{"epoch":1,"lifecycle":"Cold","full_calibration_performed":true,"path":"FullForCache","projection_schema":1,"calibration_words":92,"registers":1,"analog_bytes":1,"channel":1,"radio":"Wifi"},"snapshot":null}}})
}
fn start_fake(fake: &Fake, store: &Store) -> Result<(CacheKey, Started)> {
    start(
        fake,
        store,
        StartRequest {
            boot_id: 11,
            session_id: 22,
            chip: "esp32s31",
            board: oer_lab_board::DeviceId::parse("020000002564")?,
            capture: PrepareCapture {
                channel: 1,
                radio: CaptureRadio::Wifi,
            },
            source: CalibrationSource::HostCache,
        },
    )
}

#[test]
fn missing_file_seals_immediately_and_all_requests_keep_boot_and_session() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::at(dir.path().into());
    let fake = Fake::new(vec![
        identity(),
        awaiting(),
        serde_json::json!({"ticket":7,"decision":{"NoCache":"Missing"}}),
        ready(),
    ]);
    let (_, started) = start_fake(&fake, &store).unwrap();
    assert_eq!(started.boot_id, 11);
    let requests = fake.requests.borrow();
    assert_eq!(requests.len(), 4);
    assert!(
        requests
            .iter()
            .all(|(_, boot, session, _)| *boot == 11 && *session == 22)
    );
    assert_eq!(requests[2].3, Duration::from_millis(499));
}
#[test]
fn foreign_ticket_or_changed_target_is_not_used() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::at(dir.path().into());
    let mut wrong = awaiting();
    wrong["state"]["AwaitingCache"]["target"]["identity"] = serde_json::json!([9]);
    assert!(start_fake(&Fake::new(vec![identity(), wrong]), &store).is_err());
    assert!(
        start_fake(
            &Fake::new(vec![
                identity(),
                awaiting(),
                serde_json::json!({"ticket":8,"decision":{"NoCache":"Missing"}})
            ]),
            &store
        )
        .is_err()
    );
}
#[test]
fn start_transit_counts_against_the_fixed_window() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::at(dir.path().into());
    let mut fake = Fake::new(vec![identity(), awaiting()]);
    fake.cost_ms = 500;
    assert!(start_fake(&fake, &store).is_err());
    assert_eq!(fake.requests.borrow().len(), 2);
}
#[test]
fn failed_native_registration_is_terminal() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::at(dir.path().into());
    let fake = Fake::new(vec![
        identity(),
        awaiting(),
        serde_json::json!({"ticket":7,"decision":{"NoCache":"Missing"}}),
        serde_json::json!({"ticket":7,"state":{"Failed":{"decision":{"NoCache":"Missing"}}}}),
    ]);
    assert!(start_fake(&fake, &store).is_err());
    assert_eq!(fake.requests.borrow().len(), 4);
}

fn seeded_store(store: &Store) -> CacheKey {
    let key = CacheKey::new(
        "esp32s31",
        oer_lab_board::DeviceId::parse("020000002564").unwrap(),
        serde_json::from_value(target()).unwrap(),
    )
    .unwrap();
    let bytes = vec![42; 261];
    let metadata = CalibrationCacheMetadata {
        format: CalibrationCacheFormat(1),
        schema: 8,
        algorithm_revision: 1,
        profile: 1,
        length: 261,
        crc32c: calibration_cache_crc32c(&bytes),
    };
    let mut value = ready();
    value["state"]["Ready"]["snapshot"] = serde_json::to_value(metadata).unwrap();
    let status: CalibrationStartStatus = serde_json::from_value(value).unwrap();
    let mut download = SnapshotDownload::new(10, key.target().clone(), &status).unwrap();
    while let Some(request) = download.next_request() {
        let first = usize::from(request.first);
        download
            .push(CalibrationSnapshot {
                ticket: 7,
                first: request.first,
                metadata,
                temperature: CalibrationTemperature {
                    value: 23,
                    scale: CalibrationTemperatureScale::DegreesCelsius,
                },
                bytes: bytes[first..first + usize::from(request.count)]
                    .try_into()
                    .unwrap(),
            })
            .unwrap();
    }
    store
        .promote(
            &key,
            &download.finish().unwrap(),
            Provenance {
                image_commit: "1".repeat(40),
                application_sha256: "2".repeat(64),
                run_id: "seed".into(),
                captured_unix_seconds: 1,
            },
        )
        .unwrap();
    key
}
#[test]
fn a_cache_hit_uploads_nine_bounded_pages_and_a_partial_never_replaces_last_full() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::at(dir.path().join("store"));
    let key = seeded_store(&store);
    let previous = std::fs::read(store.path(&key)).unwrap();
    let mut replies = vec![identity(), awaiting()];
    for next in [32, 64, 96, 128, 160, 192, 224, 256, 261] {
        replies.push(serde_json::json!({"ticket":7,"next":next}));
    }
    replies.push(serde_json::json!({"ticket":7,"decision":"CandidateSupplied"}));
    let mut partial = ready();
    partial["state"]["Ready"]["decision"] = serde_json::json!("CandidateSupplied");
    partial["state"]["Ready"]["capture"]["full_calibration_performed"] = serde_json::json!(false);
    partial["state"]["Ready"]["capture"]["path"] = serde_json::json!("PartialFromCache");
    replies.push(partial);
    let fake = Fake::new(replies);
    let (_, started) = start_fake(&fake, &store).unwrap();
    assert_eq!(fake.requests.borrow().len(), 13);
    assert!(
        save(
            &fake,
            22,
            &store,
            &key,
            &started,
            Provenance {
                image_commit: "1".repeat(40),
                application_sha256: "2".repeat(64),
                run_id: "partial".into(),
                captured_unix_seconds: 2
            },
            &dir.path().join("partial.json")
        )
        .is_err()
    );
    assert_eq!(std::fs::read(store.path(&key)).unwrap(), previous);
}
#[test]
fn slow_upload_stops_at_the_original_deadline_without_extending_it() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::at(dir.path().into());
    seeded_store(&store);
    let mut fake = Fake::new(vec![
        identity(),
        awaiting(),
        serde_json::json!({"ticket":7,"next":32}),
        serde_json::json!({"ticket":7,"next":64}),
    ]);
    fake.cost_ms = 200;
    assert!(start_fake(&fake, &store).is_err());
    let requests = fake.requests.borrow();
    assert_eq!(requests.len(), 4);
    assert_eq!(requests[2].3, Duration::from_millis(300));
    assert_eq!(requests[3].3, Duration::from_millis(100));
}

#[test]
fn explicit_full_uses_no_cache_upload_and_reports_one_host_clock() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::at(dir.path().into());
    let mut status = ready();
    status["state"]["Ready"]["decision"] = serde_json::json!("FullRequested");
    let fake = Fake::new(vec![
        identity(),
        serde_json::json!({"ticket":7,"state":{"Registering":{"decision":"FullRequested"}}}),
        status,
    ]);
    let (_, started) = start(
        &fake,
        &store,
        StartRequest {
            boot_id: 11,
            session_id: 0,
            chip: "esp32s31",
            board: oer_lab_board::DeviceId::parse("020000002564").unwrap(),
            capture: PrepareCapture {
                channel: 1,
                radio: CaptureRadio::Wifi,
            },
            source: CalibrationSource::Full,
        },
    )
    .unwrap();
    assert_eq!(fake.requests.borrow().len(), 3);
    assert_eq!(started.host_load_us, 0);
    assert_eq!(started.host_transfer_us, 0);
    assert_eq!(started.host_start_us, 3000);
}
