/*
 * Bluetooth LE Direct Test Mode reference peer for the open-esp-radio HIL.
 *
 * The vendor BLE Controller owns the radio; this application only issues the
 * standard HCI test commands to it over VHCI and reports their results
 * through a line protocol on the console. Every protocol line starts with
 * '@'; the host ignores any other output. See README.md.
 */

#include <stdbool.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include "esp_bt.h"
#include "esp_err.h"
#include "freertos/FreeRTOS.h"
#include "freertos/queue.h"
#include "freertos/semphr.h"
#include "freertos/task.h"
#include "nvs_flash.h"
#include "sdkconfig.h"

#if CONFIG_IDF_TARGET_ESP32C5
#include "hal/pmu_types.h"
#include "modem/modem_lpcon_struct.h"
#endif

#if CONFIG_ESP_CONSOLE_USB_SERIAL_JTAG
#include "driver/usb_serial_jtag.h"
#include "driver/usb_serial_jtag_vfs.h"
#else
#include "driver/uart.h"
#include "driver/uart_vfs.h"
#endif

/* ESP-IDF enables the analog I2C master clock in the PMU's MODEM state as well
 * as ACTIVE. That map survives a USB Serial/JTAG (RTS) reset of the HP system,
 * after which the esp32c5 rev 1.0 ROM boots into UART/SDIO download with USB
 * dead. The peer never enters the MODEM state, so keep the ROM's ACTIVE-only
 * map at start and after each radio enable. */
static void keep_rom_i2c_master_clock_map(void)
{
#if CONFIG_IDF_TARGET_ESP32C5
    MODEM_LPCON.clk_conf_power_st.clk_i2c_mst_st_map = BIT(PMU_HP_ICG_MODEM_CODE_ACTIVE);
#endif
}

#define PROTOCOL_VERSION 1
#define LINE_CAPACITY 128
#define COMMAND_TIMEOUT_MS 2000

#define HCI_COMMAND 0x01
#define HCI_EVENT 0x04
#define EVENT_COMMAND_COMPLETE 0x0e
#define EVENT_COMMAND_STATUS 0x0f

#define OP_RESET 0x0c03
#define OP_RECEIVER_TEST_V1 0x201d
#define OP_TRANSMITTER_TEST_V1 0x201e
#define OP_TEST_END 0x201f
#define OP_RECEIVER_TEST_V2 0x2033
#define OP_TRANSMITTER_TEST_V2 0x2034

/* The completion of the command in flight: its status and up to two return
 * parameter octets (the Test End packet count). */
typedef struct {
    uint16_t opcode;
    uint8_t status;
    uint8_t parameters[2];
    uint8_t parameter_length;
} completion_t;

static QueueHandle_t s_completions;
static SemaphoreHandle_t s_send_available;

static void notify_host_send_available(void)
{
    xSemaphoreGive(s_send_available);
}

/* Runs in the Controller's context: copy the completion and return. */
static int notify_host_recv(uint8_t *data, uint16_t length)
{
    if (length < 6 || data[0] != HCI_EVENT) {
        return 0;
    }
    completion_t completion = { 0 };
    if (data[1] == EVENT_COMMAND_COMPLETE && length >= 7) {
        completion.opcode = (uint16_t)(data[4] | data[5] << 8);
        completion.status = data[6];
        completion.parameter_length = (uint8_t)(length - 7 > 2 ? 2 : length - 7);
        memcpy(completion.parameters, &data[7], completion.parameter_length);
    } else if (data[1] == EVENT_COMMAND_STATUS && length >= 7) {
        completion.status = data[3];
        completion.opcode = (uint16_t)(data[5] | data[6] << 8);
    } else {
        return 0;
    }
    xQueueSend(s_completions, &completion, 0);
    return 0;
}

static const esp_vhci_host_callback_t s_vhci_callbacks = {
    .notify_host_send_available = notify_host_send_available,
    .notify_host_recv = notify_host_recv,
};

/* Send one command and wait for its completion. Returns false on timeout. */
static bool hci_command(uint16_t opcode, const uint8_t *parameters, uint8_t length,
                        completion_t *completion)
{
    uint8_t packet[4 + 8];
    if (length > sizeof(packet) - 4) {
        return false;
    }
    packet[0] = HCI_COMMAND;
    packet[1] = (uint8_t)opcode;
    packet[2] = (uint8_t)(opcode >> 8);
    packet[3] = length;
    memcpy(&packet[4], parameters, length);
    TickType_t deadline = xTaskGetTickCount() + pdMS_TO_TICKS(COMMAND_TIMEOUT_MS);
    while (!esp_vhci_host_check_send_available()) {
        TickType_t now = xTaskGetTickCount();
        if (now >= deadline || xSemaphoreTake(s_send_available, deadline - now) != pdTRUE) {
            return false;
        }
    }
    xQueueReset(s_completions);
    esp_vhci_host_send_packet(packet, (uint16_t)(4 + length));
    for (;;) {
        TickType_t now = xTaskGetTickCount();
        if (now >= deadline || xQueueReceive(s_completions, completion, deadline - now) != pdTRUE) {
            return false;
        }
        if (completion->opcode == opcode) {
            return true;
        }
    }
}

static void reply_ok(const char *command)
{
    printf("@OK %s\n", command);
    fflush(stdout);
}

static void reply_error(const char *command, const char *reason)
{
    printf("@ERR %s %s\n", command, reason);
    fflush(stdout);
}

/* Run one command and answer the protocol line; true when it succeeded. */
static bool run(const char *command, uint16_t opcode, const uint8_t *parameters, uint8_t length,
                completion_t *completion)
{
    if (!hci_command(opcode, parameters, length, completion)) {
        reply_error(command, "timeout");
        return false;
    }
    if (completion->status != 0) {
        char reason[16];
        snprintf(reason, sizeof(reason), "status=0x%02x", completion->status);
        reply_error(command, reason);
        return false;
    }
    return true;
}

static bool parse_u32(const char *text, uint32_t *value, uint32_t maximum)
{
    char *end;
    unsigned long parsed = strtoul(text, &end, 10);
    if (*text == '\0' || *end != '\0' || parsed > maximum) {
        return false;
    }
    *value = (uint32_t)parsed;
    return true;
}

/* LE Receiver Test [v2] PHY: 1M, 2M or Coded (either coding). */
static bool parse_receive_phy(const char *text, uint8_t *phy)
{
    if (strcmp(text, "1M") == 0) {
        *phy = 1;
    } else if (strcmp(text, "2M") == 0) {
        *phy = 2;
    } else if (strcmp(text, "CODED") == 0) {
        *phy = 3;
    } else {
        return false;
    }
    return true;
}

/* LE Transmitter Test [v2] PHY: 1M, 2M, Coded S=8 or Coded S=2. */
static bool parse_transmit_phy(const char *text, uint8_t *phy)
{
    if (strcmp(text, "1M") == 0) {
        *phy = 1;
    } else if (strcmp(text, "2M") == 0) {
        *phy = 2;
    } else if (strcmp(text, "S8") == 0) {
        *phy = 3;
    } else if (strcmp(text, "S2") == 0) {
        *phy = 4;
    } else {
        return false;
    }
    return true;
}

/* RX <version 1|2> <channel 0..39> <phy 1M|2M|CODED> */
static void command_rx(char **argv, int argc)
{
    uint32_t version, channel;
    uint8_t phy;
    if (argc != 4 || !parse_u32(argv[1], &version, 2) || version == 0
        || !parse_u32(argv[2], &channel, 39) || !parse_receive_phy(argv[3], &phy)
        || (version == 1 && phy != 1)) {
        reply_error("RX", "invalid");
        return;
    }
    completion_t completion;
    uint8_t parameters[3] = { (uint8_t)channel, phy, 0 };
    bool ok = version == 1 ? run("RX", OP_RECEIVER_TEST_V1, parameters, 1, &completion)
                           : run("RX", OP_RECEIVER_TEST_V2, parameters, 3, &completion);
    if (ok) {
        reply_ok("RX");
    }
}

/* TX <version 1|2> <channel 0..39> <length 0..255> <pattern 0..7> <phy 1M|2M|S8|S2> */
static void command_tx(char **argv, int argc)
{
    uint32_t version, channel, length, pattern;
    uint8_t phy;
    if (argc != 6 || !parse_u32(argv[1], &version, 2) || version == 0
        || !parse_u32(argv[2], &channel, 39) || !parse_u32(argv[3], &length, 255)
        || !parse_u32(argv[4], &pattern, 7) || !parse_transmit_phy(argv[5], &phy)
        || (version == 1 && (phy != 1 || length > 37))) {
        reply_error("TX", "invalid");
        return;
    }
    completion_t completion;
    uint8_t parameters[4] = { (uint8_t)channel, (uint8_t)length, (uint8_t)pattern, phy };
    bool ok = version == 1 ? run("TX", OP_TRANSMITTER_TEST_V1, parameters, 3, &completion)
                           : run("TX", OP_TRANSMITTER_TEST_V2, parameters, 4, &completion);
    if (ok) {
        reply_ok("TX");
    }
}

/* END: LE Test End, printing the receiver's packet count. */
static void command_end(void)
{
    completion_t completion;
    if (!run("END", OP_TEST_END, NULL, 0, &completion)) {
        return;
    }
    unsigned count = completion.parameter_length == 2
        ? (unsigned)(completion.parameters[0] | completion.parameters[1] << 8)
        : 0;
    printf("@END packets=%u\n", count);
    reply_ok("END");
}

/* RESET: HCI Reset, which also ends a test without a count. */
static bool reset(const char *command)
{
    completion_t completion;
    return run(command, OP_RESET, NULL, 0, &completion);
}

static void dispatch(char *line)
{
    char *argv[8];
    int argc = 0;
    for (char *token = strtok(line, " "); token != NULL && argc < 8; token = strtok(NULL, " ")) {
        argv[argc++] = token;
    }
    if (argc == 0) {
        return;
    }
    if (strcmp(argv[0], "RX") == 0) {
        command_rx(argv, argc);
    } else if (strcmp(argv[0], "TX") == 0) {
        command_tx(argv, argc);
    } else if (strcmp(argv[0], "END") == 0 && argc == 1) {
        command_end();
    } else if (strcmp(argv[0], "RESET") == 0 && argc == 1) {
        if (reset("RESET")) {
            reply_ok("RESET");
        }
    } else if (strcmp(argv[0], "SYNC") == 0 && argc == 1) {
        /* The host takes over a running peer without resetting the chip. */
        if (reset("SYNC")) {
            printf("@READY protocol=%d target=%s\n", PROTOCOL_VERSION, CONFIG_IDF_TARGET);
            reply_ok("SYNC");
        }
    } else {
        reply_error(argv[0], "unknown");
    }
}

static void console_init(void)
{
#if CONFIG_ESP_CONSOLE_USB_SERIAL_JTAG
    usb_serial_jtag_driver_config_t config = USB_SERIAL_JTAG_DRIVER_CONFIG_DEFAULT();
    ESP_ERROR_CHECK(usb_serial_jtag_driver_install(&config));
    usb_serial_jtag_vfs_use_driver();
#else
    ESP_ERROR_CHECK(uart_driver_install(CONFIG_ESP_CONSOLE_UART_NUM, 2 * LINE_CAPACITY, 0, 0, NULL, 0));
    uart_vfs_dev_use_driver(CONFIG_ESP_CONSOLE_UART_NUM);
#endif
    setvbuf(stdin, NULL, _IONBF, 0);
}

static void controller_init(void)
{
    esp_err_t error = nvs_flash_init();
    if (error == ESP_ERR_NVS_NO_FREE_PAGES || error == ESP_ERR_NVS_NEW_VERSION_FOUND) {
        ESP_ERROR_CHECK(nvs_flash_erase());
        error = nvs_flash_init();
    }
    ESP_ERROR_CHECK(error);
    esp_bt_controller_config_t config = BT_CONTROLLER_INIT_CONFIG_DEFAULT();
    ESP_ERROR_CHECK(esp_bt_controller_init(&config));
    ESP_ERROR_CHECK(esp_bt_controller_enable(ESP_BT_MODE_BLE));
    keep_rom_i2c_master_clock_map();
    ESP_ERROR_CHECK(esp_vhci_host_register_callback(&s_vhci_callbacks));
}

void app_main(void)
{
    keep_rom_i2c_master_clock_map();
    s_completions = xQueueCreate(4, sizeof(completion_t));
    s_send_available = xSemaphoreCreateBinary();
    configASSERT(s_completions != NULL && s_send_available != NULL);
    console_init();
    controller_init();
    completion_t completion;
    if (!hci_command(OP_RESET, NULL, 0, &completion) || completion.status != 0) {
        printf("@ERR START reset\n");
        fflush(stdout);
    }
    printf("@READY protocol=%d target=%s\n", PROTOCOL_VERSION, CONFIG_IDF_TARGET);
    fflush(stdout);

    static char line[LINE_CAPACITY];
    size_t length = 0;
    for (;;) {
        int character = getchar();
        if (character == EOF) {
            vTaskDelay(1);
            continue;
        }
        if (character == '\r') {
            continue;
        }
        if (character == '\n') {
            line[length] = '\0';
            dispatch(line);
            length = 0;
            continue;
        }
        if (length + 1 < sizeof(line)) {
            line[length++] = (char)character;
        } else {
            length = 0;
            reply_error("LINE", "overflow");
        }
    }
}
