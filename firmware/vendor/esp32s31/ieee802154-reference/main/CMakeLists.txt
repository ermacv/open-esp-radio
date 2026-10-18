# The reference peer's application: the same line protocol over the vendor
# driver, built for the ESP32-S31.
idf_component_register(
    SRCS "../../../../../hil/peers/esp32c5-ieee802154/main/peer.c"
    PRIV_REQUIRES ieee802154 esp_hw_support esp_timer esp_driver_uart esp_driver_usb_serial_jtag vfs
)
