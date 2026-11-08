# The vendor UDP receiver's source, built with its fast consumer.
idf_component_register(
    SRCS "../../esp32s31-wifi-udp-rx/main/udp_rx.c" "../../esp32s31-wifi-udp-rx/main/link.c"
    INCLUDE_DIRS "../../esp32s31-wifi-udp-rx/main"
    PRIV_REQUIRES esp_wifi esp_event esp_netif nvs_flash esp_hw_support esp_timer
                  esp_driver_usb_serial_jtag vfs lwip log
)
target_compile_definitions(${COMPONENT_LIB} PRIVATE OER_UDP_RX_FAST=1)
