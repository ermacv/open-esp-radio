/*
 * Vendor link environment of the ESP32-S31 vendor scenarios.
 *
 * The scenarios never run this application. They link vendor archive
 * sections against its image and bootloader, which define the PHY, Wi-Fi,
 * network-stack and logging symbols those sections reference. A station
 * that initializes the network interface, enables the PHY and starts Wi-Fi
 * keeps every such symbol in the image.
 */
#include "esp_event.h"
#include "esp_netif.h"
#include "esp_wifi.h"

void app_main(void)
{
    ESP_ERROR_CHECK(esp_netif_init());
    ESP_ERROR_CHECK(esp_event_loop_create_default());
    esp_netif_create_default_wifi_sta();
    wifi_init_config_t wifi = WIFI_INIT_CONFIG_DEFAULT();
    ESP_ERROR_CHECK(esp_wifi_init(&wifi));
    ESP_ERROR_CHECK(esp_wifi_set_mode(WIFI_MODE_STA));
    ESP_ERROR_CHECK(esp_wifi_start());
}
