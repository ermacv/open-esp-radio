use oer_probe_macros::probe;

fn with_owner<R>(call: impl FnOnce(&mut [u32; 2]) -> R) -> R {
    let mut owner = [10, 20];
    call(&mut owner)
}
fn production(owner: &mut [u32; 2], delta: i8) -> Result<u32, ()> {
    if delta < 0 {
        return Err(());
    }
    owner[0] += delta as u32;
    Ok(owner[0] + owner[1])
}
probe! {
    pub fn scalar_buffer(delta: i8, output: &mut [u32; 2]) -> i32 =>
        with_owner(|owner| production(owner, delta).map_or(-1, |value| {
            *output = *owner;
            value as i32
        }));
}
probe! {
    pub fn explicit_adapter(value: u32) -> u32 {
        match value { 0 => 9, _ => value.wrapping_add(1) }
    }
}
#[test]
fn setup_call_and_projection_preserve_success_and_failure() {
    let mut output = [91, 92];
    assert_eq!(scalar_buffer(-1, &mut output), -1);
    assert_eq!(output, [91, 92]);
    assert_eq!(scalar_buffer(5, &mut output), 35);
    assert_eq!(output, [15, 20]);
    assert_eq!(scalar_buffer(0, &mut output), 30);
    assert_eq!(explicit_adapter(u32::MAX), 0);
}
