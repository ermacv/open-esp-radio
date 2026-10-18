/*
 * Cold vendor PHY calibration for the hardware calibration cross-check.
 *
 * Registers the PHY once with full calibration (calibration storage is
 * disabled, so no retained data applies), then reports every generated
 * vendor object as one line per object:
 *
 *   oer-vendor-calibration <object> <hex bytes>
 *
 * then starts the Wi-Fi client without a connection, receiving on its home
 * channel, and prints
 * `oer-vendor-calibration-end`. It then answers register reads
 * the host requests from the published register model:
 *
 *   r <hex address>   ->   oer-vendor-calibration-register <address> <value>
 *
 *   a <hex block> <hex register>
 *       ->   oer-vendor-calibration-analog <block> <register> <value>
 *
 * and restarts the Wi-Fi radio on request, as production's idle radio
 * restart does, so the host can read the registers after one RF close and
 * wake:
 *
 *   w   ->   oer-vendor-calibration-restarted <PHY modem flags while stopped>
 *
 * The registration and the client share the Wi-Fi modem flag, so stopping
 * the client clears the last flag and closes RF: the flags read zero. Starting
 * it again wakes the PHY.
 *
 * Analog registers are read through the ESP-IDF analog-I2C driver, which
 * selects the block's host from the host map the PHY configured. The
 * firmware holds no register list of its own. The host resets the board
 * for each repetition.
 */
#include <stdio.h>
#include <stdlib.h>

#include "driver/usb_serial_jtag.h"

#include "esp_event.h"
#include "esp_phy_init.h"
#include "esp_private/phy.h"
#include "esp_private/regi2c_ctrl.h"
#include "esp_wifi.h"
#include "freertos/FreeRTOS.h"
#include "freertos/task.h"
#include "reported.h"

#define REPORT_PREFIX "oer-vendor-calibration"
/* Longest request line: "r " and eight hex digits. */
#define REQUEST_BYTES 16

/* Stop the Wi-Fi client, releasing its PHY modem, and start it again in
 * the boot's receive state; reply with the modem flags while stopped. */
static void restart(void)
{
    ESP_ERROR_CHECK(esp_wifi_stop());
    unsigned flags = phy_get_modem_flag();
    ESP_ERROR_CHECK(esp_wifi_start());
    ESP_ERROR_CHECK(esp_wifi_set_promiscuous(true));
    char reply[64];
    int length = snprintf(reply, sizeof(reply), REPORT_PREFIX "-restarted %08x\n", flags);
    usb_serial_jtag_write_bytes(reply, length, portMAX_DELAY);
}

/* Answer one `a <hex block> <hex register>` request with the analog
 * register's value; every block the host names reports host id zero. */
static void answer_analog(const char *request)
{
    char *end;
    uint8_t block = (uint8_t)strtoul(request + 2, &end, 16);
    uint8_t reg = (uint8_t)strtoul(end, NULL, 16);
    uint8_t value = regi2c_ctrl_read_reg(block, 0, reg);
    char reply[64];
    int length = snprintf(reply, sizeof(reply), REPORT_PREFIX "-analog %02x %02x %02x\n", block,
                          reg, value);
    usb_serial_jtag_write_bytes(reply, length, portMAX_DELAY);
}

/* Answer one `r <hex address>`, `a <hex block> <hex register>` or `w`
 * request. */
static void answer(const char *request)
{
    if (request[0] == 'w' && request[1] == '\0') {
        restart();
        return;
    }
    if (request[0] == 'a' && request[1] == ' ') {
        answer_analog(request);
        return;
    }
    if (request[0] != 'r' || request[1] != ' ') {
        return;
    }
    uint32_t address = strtoul(request + 2, NULL, 16);
    uint32_t value = *(volatile const uint32_t *)address;
    char reply[64];
    int length = snprintf(reply, sizeof(reply), REPORT_PREFIX "-register %08lx %08lx\n",
                          (unsigned long)address, (unsigned long)value);
    usb_serial_jtag_write_bytes(reply, length, portMAX_DELAY);
}

void app_main(void)
{
    esp_phy_enable(PHY_MODEM_WIFI);
    for (size_t i = 0; i < sizeof(REPORTED_OBJECTS) / sizeof(REPORTED_OBJECTS[0]); i++) {
        const reported_object_t *object = &REPORTED_OBJECTS[i];
        printf(REPORT_PREFIX " %s ", object->name);
        for (size_t j = 0; j < object->length; j++) {
            printf("%02x", object->bytes[j]);
        }
        printf("\n");
    }
    /* Bring the Wi-Fi client up without a connection, as production's
     * role-neutral initialization does, so the register state is read at
     * the same lifecycle point: RX enabled on the default home channel. */
    ESP_ERROR_CHECK(esp_event_loop_create_default());
    wifi_init_config_t wifi = WIFI_INIT_CONFIG_DEFAULT();
    ESP_ERROR_CHECK(esp_wifi_init(&wifi));
    ESP_ERROR_CHECK(esp_wifi_set_mode(WIFI_MODE_STA));
    /* Production has no modem sleep and keeps RF open; the vendor default
     * power save would close RF before the registers are read. */
    ESP_ERROR_CHECK(esp_wifi_set_ps(WIFI_PS_NONE));
    ESP_ERROR_CHECK(esp_wifi_start());
    /* An unconnected station holds no PHY client; production's bring-up
     * enables Wi-Fi RX on its initial channel. Promiscuous RX holds the
     * PHY and RX on the current home channel in the same way. */
    ESP_ERROR_CHECK(esp_wifi_set_promiscuous(true));
    printf(REPORT_PREFIX "-end\n");
    fflush(stdout);
    usb_serial_jtag_driver_config_t config = USB_SERIAL_JTAG_DRIVER_CONFIG_DEFAULT();
    ESP_ERROR_CHECK(usb_serial_jtag_driver_install(&config));
    char request[REQUEST_BYTES];
    size_t length = 0;
    for (;;) {
        char byte;
        if (usb_serial_jtag_read_bytes(&byte, 1, portMAX_DELAY) != 1) {
            continue;
        }
        if (byte == '\n') {
            request[length] = '\0';
            answer(request);
            length = 0;
        } else if (length + 1 < sizeof(request)) {
            request[length++] = byte;
        }
    }
}
