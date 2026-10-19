/* ESP32-C5 ECO2 ROM symbols used by the bootstrap and runtime images: the
 * ROM of revision v1.0 (`esp32c5-eco2-20250121`, the stand's chip), whose
 * interface ESP-IDF links for every revision below v1.2 (the ECO3 script
 * applies from CONFIG_ESP32C5_REV_MIN_FULL 102 on).
 *
 * SOURCE(esp32c5): ESP-IDF 4d59230d `components/esp_rom/esp32c5/ld/esp32c5.rom.ld`
 * ("Compatible with ROM where ECO version equal or greater to 2"; lines 24-36
 * ets_*, 109-111 esp_rom_spi_*, 135-146 esp_rom_spiflash_*, 171
 * spi_flash_attach, 190 ROM_Boot_Cache_Init), `esp32c5.rom.api.ld:59`
 * (esp_rom_spiflash_attach = spi_flash_attach), `esp32c5.rom.version.ld:13`
 * and `esp32c5.rom.phy.ld:27`; `components/esp_rom/CMakeLists.txt:311` selects
 * `esp32c5.rom.eco3.ld` only from revision v1.2. */
ets_delay_us = 0x4000003c;
ets_printf = 0x40000024;
ets_install_usb_printf = 0x40000034;
ets_get_cpu_frequency = 0x40000040;
uart_tx_one_char = 0x40000054;
esp_rom_spi_cmd_config = 0x40000114;
esp_rom_spi_cmd_start = 0x40000118;
esp_rom_spi_set_op_mode = 0x4000011c;
ROM_Boot_Cache_Init = 0x40000644;
esp_rom_spiflash_read = 0x40000160;
esp_rom_spiflash_config_param = 0x40000170;
esp_rom_spi_flash_update_id = 0x40000184;
esp_rom_spiflash_config_clk = 0x40000188;
esp_rom_spiflash_config_readmode = 0x4000018c;
esp_rom_spiflash_attach = 0x400001f0;

/* ROM data symbols the PHY library references. The ESP32-C5 ROM has no
 * rom_phyFuns. */
_rom_eco_version = 0x40000014;
phy_param_rom = 0x4085fc70;
