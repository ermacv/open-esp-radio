use super::*;
use oer_esp32s31_hal::dma_static::Placed;
use std::boxed::Box;

#[test]
fn a_role_recipe_cannot_overwrite_an_already_prepared_event() {
    let storage = Box::leak(Box::new(SchedulerRolePoolStorage::<DtmStorage, 1>::new()));
    let numbers = SchedulerAllocationConfig::new(0, 0, 0)
        .expect("numbers fit")
        .direct_test_mode();
    with_pool(|pool| {
        with_epoch(|epoch| {
            let mut pool = pool
                .place::<HostDma<0x2f00_1000>, _, 1>(epoch, Placed::host(storage), numbers)
                .expect("one DTM allocation");
            let lease = pool.acquire().expect("one slot");
            let initial = pool.words(&lease).expect("CPU value seed");
            let builder = DtmImageBuilder::receiver(
                DtmEventPhase::Initial,
                DtmSchedulerReceiverPhy::Le1M,
                LeTxPower::from_dbm(0).expect("supported power"),
                0,
                DtmSchedulerFrequency::from_controller_image(24).expect("valid frequency"),
                SchedulerRawWindow::from_projected_scheduler_window(100, 200)
                    .expect("nonempty window"),
                100,
            );
            builder
                .prepare(&mut pool, &lease)
                .expect("CPU graph accepts its own bound seed");
            let committed = pool.words(&lease).expect("detached value snapshot");
            assert_ne!(committed, initial);
            assert_eq!(
                builder.prepare(&mut pool, &lease),
                Err(DtmPrepareError::State)
            );
            assert_eq!(pool.words(&lease), Some(committed));
            pool.release(lease)
                .expect("unpublished graph remains CPU owned");
        })
    });
}
