#![no_std]
use oer_radio_port::{PortBackend, PortDriver};

#[cfg(ui_reject)]
pub fn retain_for_p2_drop<'scope, 'id, B: PortBackend + 'static>(
    driver: PortDriver<'scope, 'id, B>,
) {
    fn permanent<T: 'static>(_: T) {}
    permanent(driver.run()); // ui-error: E0521
}

#[cfg(not(ui_reject))]
pub fn join_before_scope_ends<'scope, 'id, B: PortBackend>(
    driver: PortDriver<'scope, 'id, B>,
) -> impl core::future::Future<Output = oer_radio_port::DriverExit<B::State, B::Fault>>
+ use<'scope, 'id, B> {
    use oer_radio_port::PortExecution;
    oer_radio_port::LocalExecution.start(driver)
}
