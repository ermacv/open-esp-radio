use oer_espressif_interrupt_table::signal::host;

pub fn check() {
    let (signal, raiser) = host::deferred_signal();
    #[cfg(ui_reject)]
    let _worker = std::thread::spawn(move || { // ui-error: E0277
        let _ = signal.take();
        signal.service();
    });
    #[cfg(not(ui_reject))]
    let _worker = std::thread::spawn(move || { raiser.raise(1); });
    #[cfg(not(ui_reject))]
    let _ = signal.take();
}
