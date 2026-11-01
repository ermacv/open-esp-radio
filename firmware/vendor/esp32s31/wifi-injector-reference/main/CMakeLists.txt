# The Wi-Fi frame source peer's application: the same line protocol over the
# vendor driver, built for the ESP32-S31.
idf_component_register(
    SRCS "../../../../peers/esp32c5-wifi-injector/main/injector.c"
    PRIV_REQUIRES esp_wifi esp_event nvs_flash esp_hw_support esp_timer esp_driver_usb_serial_jtag vfs
)
