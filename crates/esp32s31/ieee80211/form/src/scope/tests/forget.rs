use super::*;
use core::future::pending;

struct BodyOwner(Count);
impl Drop for BodyOwner {
    fn drop(&mut self) {
        self.0.increment();
    }
}

#[test]
fn forgetting_owning_future_during_each_actor_retains_backing_without_any_drop() {
    for which in [Actor::Mac, Actor::Power, Actor::Driver] {
        let (route, slot, platform) = make_route();
        let block = Block::new();
        let mut hardware = hardware(&platform);
        let (work, audit) = work::DmaQueue::populated();
        hardware.work = work;
        let (stops, hardware_drops) = (hardware.stops.clone(), hardware.drops.clone());
        let mut irq = Interrupts::default();
        let (irq_drops, detached) = (irq.drops.clone(), irq.detached.clone());
        let resumed = match which {
            Actor::Driver => {
                hardware.block = Some(block.clone());
                hardware.accesses_after_pause.clone()
            }
            _ => {
                irq.block = Some(block.clone());
                irq.accesses_after_pause.clone()
            }
        };
        let body_drops = Count::default();
        let mut latch = Latch::new();
        let scope = new_scope(lease(), hardware, irq, route, &mut latch);
        let mut context = 13;
        let mut future = Box::pin(scope.run(
            (BodyOwner(body_drops.clone()), &mut context),
            async |(owner, context), _| {
                pending::<()>().await;
                *context += 1;
                drop(owner);
            },
        ));
        assert!(poll(future.as_mut()).is_pending());
        let registered = slot.entry.load(Ordering::Acquire);
        let actor = thread::spawn(move || actor(slot, which));
        block.entered.wait();
        core::mem::forget(future);
        assert_eq!(context, 13, "forgotten work cannot address caller context");
        assert!(
            latch.into_return().is_none(),
            "no lease was returned for reuse"
        );
        assert_eq!(slot.entry.load(Ordering::Acquire), registered);
        assert_eq!(slot.entered.load(Ordering::Acquire), 1);
        assert_eq!(body_drops.get(), 0);
        assert_eq!(hardware_drops.get(), 0);
        assert_eq!(irq_drops.get(), 0);
        assert_eq!(stops.get(), 0);
        assert_eq!(detached.get(), 0);
        assert_eq!(audit.buffer_drops.get(), 0);
        block.resume.wait();
        actor.join().unwrap();
        assert_eq!(
            resumed.get(),
            1,
            "entered work finishes in the same backing"
        );
        assert_eq!(slot.entry.load(Ordering::Acquire), registered);
        assert_eq!(slot.entered.load(Ordering::Acquire), 0);
        assert_eq!(audit.descriptors.get(), 2);
        assert_eq!(audit.queued.get(), 2);
        assert_eq!(audit.buffer_drops.get(), 0);
        assert_eq!(body_drops.get(), 0);
        assert_eq!(hardware_drops.get(), 0);
        assert_eq!(irq_drops.get(), 0);
    }
}
