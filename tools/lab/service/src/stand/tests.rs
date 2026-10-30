use super::*;

#[test]
fn only_a_download_boot_over_usb_parks_a_board() {
    assert!(serves_usb_download(
        "rst:0x1 (POWERON),boot:0x8 (DOWNLOAD(USB/UART0))"
    ));
    assert!(!serves_usb_download(
        "rst:0x15 (USB_UART_HPSYS),boot:0x0 (DOWNLOAD(UART0/SDIO_FEI_FEO))"
    ));
    assert!(!serves_usb_download(
        "rst:0x15 (USB_UART_HPSYS),boot:0x18 (SPI_FAST_FLASH_BOOT)"
    ));
    assert!(!serves_usb_download("DOWNLOAD(USB) without a boot field"));
}
