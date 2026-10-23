//! A handler owner can move only while every route reaching it is masked.
use core::marker::PhantomData;

pub trait Control {
    fn enable(&mut self) -> bool;
    fn disable_radio(&mut self);
    fn disable_doorbell(&mut self);
}
pub enum Off {}
pub enum Live {}
pub enum Draining {}
pub struct Routes<C, State> {
    control: C,
    state: PhantomData<State>,
}
impl<C: Control> Routes<C, Off> {
    pub fn new(mut control: C) -> Self {
        control.disable_radio();
        control.disable_doorbell();
        Self {
            control,
            state: PhantomData,
        }
    }
    pub fn handover<R>(&self, transfer: impl FnOnce() -> R) -> R {
        transfer()
    }
    pub fn enable(mut self) -> Result<Routes<C, Live>, Self> {
        if !self.control.enable() {
            self.control.disable_radio();
            self.control.disable_doorbell();
            return Err(self);
        }
        Ok(Routes {
            control: self.control,
            state: PhantomData,
        })
    }
}
impl<C: Control> Routes<C, Live> {
    /// The doorbell stays live until outstanding timer transfers have returned.
    pub fn drain(mut self) -> Routes<C, Draining> {
        self.control.disable_radio();
        Routes {
            control: self.control,
            state: PhantomData,
        }
    }
}
impl<C: Control> Routes<C, Draining> {
    /// This consumes the draining phase before any handler-state handover.
    ///
    /// ```compile_fail
    /// use oer_firmware_apps_b0::routes::{Control, Draining, Routes};
    /// fn transfer<C: Control>(routes: Routes<C, Draining>) {
    ///     routes.handover(|| ());
    /// }
    /// ```
    pub fn finish(mut self) -> Routes<C, Off> {
        self.control.disable_doorbell();
        Routes {
            control: self.control,
            state: PhantomData,
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use core::cell::Cell;
    struct State {
        radio: Cell<bool>,
        bell: Cell<bool>,
        pending: Cell<bool>,
        serviced: Cell<bool>,
    }
    struct Hardware<'a> {
        state: &'a State,
        fail: bool,
    }
    impl Control for Hardware<'_> {
        fn enable(&mut self) -> bool {
            self.state.radio.set(true);
            self.state.bell.set(true);
            if !self.fail && self.state.pending.replace(false) {
                self.state.serviced.set(true);
            }
            !self.fail
        }
        fn disable_radio(&mut self) {
            self.state.radio.set(false);
        }
        fn disable_doorbell(&mut self) {
            self.state.bell.set(false);
        }
    }
    fn state() -> State {
        State {
            radio: Cell::new(true),
            bell: Cell::new(true),
            pending: Cell::new(false),
            serviced: Cell::new(false),
        }
    }
    #[test]
    fn all_routes_are_off_for_handover_and_a_pending_doorbell_survives() {
        let state = state();
        let routes = Routes::new(Hardware {
            state: &state,
            fail: false,
        });
        routes.handover(|| {
            assert!(!state.radio.get() && !state.bell.get());
            state.pending.set(true);
        });
        let active = match routes.enable() {
            Ok(active) => active,
            Err(_) => panic!("valid routes"),
        };
        assert!(state.serviced.get());
        let draining = active.drain();
        assert!(!state.radio.get());
        assert!(state.bell.get(), "timer return still has a handler");
        let routes = draining.finish();
        routes.handover(|| assert!(!state.radio.get() && !state.bell.get()));
    }
    #[test]
    fn partial_enable_failure_leaves_every_route_off() {
        let state = state();
        let routes = Routes::new(Hardware {
            state: &state,
            fail: true,
        });
        let off = match routes.enable() {
            Err(off) => off,
            Ok(_) => panic!("failed routing"),
        };
        off.handover(|| assert!(!state.radio.get() && !state.bell.get()));
    }
}
