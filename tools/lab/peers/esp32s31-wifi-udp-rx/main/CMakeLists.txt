idf_component_register(
    SRCS "udp_rx.c"
    PRIV_REQUIRES esp_wifi esp_event esp_netif nvs_flash esp_hw_support esp_timer
                  esp_driver_usb_serial_jtag vfs lwip log
)
