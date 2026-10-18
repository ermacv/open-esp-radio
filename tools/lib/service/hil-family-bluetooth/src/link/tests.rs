use oer_hil_protocol::system::StackWatermark;

use super::validate_irq_stack;

#[test]
fn the_bluetooth_irq_stack_is_one_hart_with_its_reserve() {
    let healthy = StackWatermark {
        capacity_bytes: 32768,
        free_bytes: 4096,
        used_bytes: 28672,
        minimum_free_bytes: 4096,
    };
    assert!(validate_irq_stack(Some(healthy), None).is_ok());
    assert!(validate_irq_stack(None, None).is_err());
    assert!(validate_irq_stack(Some(healthy), Some(healthy)).is_err());
    let bad = StackWatermark {
        free_bytes: 4095,
        used_bytes: 28673,
        ..healthy
    };
    assert!(validate_irq_stack(Some(bad), None).is_err());
}
