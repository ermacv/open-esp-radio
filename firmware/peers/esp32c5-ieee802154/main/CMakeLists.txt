idf_component_register(
    SRCS "peer.c"
    PRIV_REQUIRES ieee802154 esp_hw_support esp_timer esp_driver_uart esp_driver_usb_serial_jtag vfs
)
