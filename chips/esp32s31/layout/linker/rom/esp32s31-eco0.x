/* ESP32-S31 ECO0 ROM symbols used by the bootstrap and runtime images. */
ets_delay_us = 0x2f80003c;
ets_printf = 0x2f800024;
ets_install_usb_printf = 0x2f800034;
ets_get_cpu_frequency = 0x2f800040;
uart_tx_one_char = 0x2f800054;
esp_rom_spi_cmd_config = 0x2f80011c;
esp_rom_spi_cmd_start = 0x2f800120;
esp_rom_spi_set_op_mode = 0x2f800124;
ROM_Boot_Cache_Init = 0x2f8005e0;
esp_rom_spiflash_read = 0x2f80016c;
esp_rom_spiflash_config_param = 0x2f80017c;
esp_rom_spi_flash_update_id = 0x2f800194;
esp_rom_spiflash_config_clk = 0x2f800198;
esp_rom_spiflash_config_readmode = 0x2f80019c;
esp_rom_spiflash_attach = 0x2f800200;

/* The fatal paths' reset (the HAL's `cores` module): the pinned rev0 ROM's
 * entry veneers, which Blobray's function records of the pinned ROM ELF
 * resolve to `software_reset` and `ets_set_appcpu_boot_addr`.
 * SOURCE(esp32s31): ESP-IDF 4d59230d `components/esp_rom/esp32s31/ld/esp32s31.rom.ld`
 * (lines 52 and 57). */
software_reset = 0x2f800094;
ets_set_appcpu_boot_addr = 0x2f8000a8;

/* ROM data symbols referenced by libphy.a.  Keeping these in the link-time
 * symbol map prevents --unresolved-symbols=ignore-all from relaxing data
 * references into address-zero loads/stores in the trace oracle. */
_rom_eco_version = 0x2f800014;
rom_phyFuns = 0x2f07fc3c;
phy_param_rom = 0x2f07fc40;
