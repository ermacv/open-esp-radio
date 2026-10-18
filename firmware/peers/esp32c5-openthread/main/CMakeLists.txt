idf_component_register(
    SRCS "ot_peer.c"
    PRIV_REQUIRES openthread nvs_flash esp_event vfs esp_driver_usb_serial_jtag
)
