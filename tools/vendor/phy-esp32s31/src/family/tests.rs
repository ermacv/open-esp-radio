use super::*;

#[test]
fn the_composition_reads_the_published_images_of_its_chip() {
    let repository = crate::registers::repository();
    let root = repository.path();
    let images = Esp32s31::images(root).unwrap();
    assert!(!images.registers.is_empty() && !images.analog.is_empty());
}
