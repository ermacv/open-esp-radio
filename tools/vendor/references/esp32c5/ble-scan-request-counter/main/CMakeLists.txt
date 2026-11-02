idf_component_register(
    SRCS "counter.c"
    PRIV_REQUIRES bt nvs_flash esp_hw_support esp_driver_usb_serial_jtag vfs
)
