idf_component_register(
    SRCS "injector.c"
    PRIV_REQUIRES esp_wifi esp_event nvs_flash esp_hw_support esp_timer esp_driver_usb_serial_jtag vfs
)
