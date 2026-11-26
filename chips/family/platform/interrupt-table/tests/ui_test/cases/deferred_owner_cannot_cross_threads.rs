//@revisions: accept reject
//@[accept] check-pass
//@[reject] compile-flags: --cfg ui_reject
use oer_chips_family_platform_interrupt_table::signal::host;

pub fn check() {
    let (signal, raiser) = host::deferred_signal();
    #[cfg(ui_reject)]
    let _worker = std::thread::spawn(move || { //~[reject] E0277
        let _ = signal.take();
        signal.service();
    });
    #[cfg(not(ui_reject))]
    let _worker = std::thread::spawn(move || { raiser.raise(1); });
    #[cfg(not(ui_reject))]
    let _ = signal.take();
}
