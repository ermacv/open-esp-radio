#![no_std]
#![forbid(unsafe_code)]

oer_chips_family_executor::declare_installations!(first);
oer_chips_family_executor::declare_installations!(second);

pub fn installations() {
    let first = first::take_static::<()>().expect("one take");
    #[cfg(ui_reject)]
    let second = first;
    #[cfg(not(ui_reject))]
    let second = second::take_static::<()>().expect("another declaration");
    let mut first = first; // ui-error: E0382
    let mut second = second;
    let _ = first.first();
    let _ = second.first();
}
