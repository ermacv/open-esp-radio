use super::*;

const FLASH_BOOT: &str = "rst:0x1 (POWERON),boot:0x18 (SPI_FAST_FLASH_BOOT)";
const DOWNLOAD: &str = "rst:0x1 (POWERON),boot:0x8 (DOWNLOAD(USB/UART0))";

#[test]
fn the_last_reset_line_of_a_console_is_the_roms_answer() {
    assert!(is_reset_line(DOWNLOAD) && !is_reset_line("garbled output"));
    let console = format!("noise\n{FLASH_BOOT}\nmore\n  {DOWNLOAD}  \nafter\n");
    assert_eq!(reset_line(&console).as_deref(), Some(DOWNLOAD));
    assert_eq!(reset_line("no line here"), None);
}
