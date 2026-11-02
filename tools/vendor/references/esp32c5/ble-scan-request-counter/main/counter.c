/*
 * Scan-request counter for the open Bluetooth LE scanner of the ESP32-S31.
 *
 * The pinned vendor BLE Controller advertises one scannable legacy set
 * (ADV_SCAN_IND) through the extended advertising commands with scan-request
 * notification enabled, and counts the LE Scan Request Received events per
 * scanner address. An active scanner that transmits SCAN_REQ therefore shows
 * up here whether or not its own receiver takes the SCAN_RSP. Every protocol
 * line starts with '@'; the host ignores any other output.
 *
 * Commands:
 * - `ADV <ms>`: advertise for `ms` milliseconds, then print
 *   `@SCANREQ total=<n>` and one `@SCANNER <type> <address> <count>` line per
 *   scanner before `@OK ADV`;
 * - `SYNC`: HCI Reset, then `@READY` and `@ADDR <public address>` again.
 */

#include <stdbool.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include "driver/usb_serial_jtag.h"
#include "driver/usb_serial_jtag_vfs.h"
#include "esp_bt.h"
#include "esp_err.h"
#include "freertos/FreeRTOS.h"
#include "freertos/queue.h"
#include "freertos/semphr.h"
#include "freertos/task.h"
#include "hal/pmu_types.h"
#include "modem/modem_lpcon_struct.h"
#include "nvs_flash.h"
#include "sdkconfig.h"

/* The ROM's ACTIVE-only analog I2C master clock map keeps a USB Serial/JTAG
 * reset of rev 1.0 out of download mode (see the DTM peer's README). */
static void keep_rom_i2c_master_clock_map(void)
{
    MODEM_LPCON.clk_conf_power_st.clk_i2c_mst_st_map = BIT(PMU_HP_ICG_MODEM_CODE_ACTIVE);
}

#define PROTOCOL_VERSION 1
#define LINE_CAPACITY 64
#define COMMAND_TIMEOUT_MS 2000
#define SCANNERS 16

#define HCI_COMMAND 0x01
#define HCI_EVENT 0x04
#define EVENT_COMMAND_COMPLETE 0x0e
#define EVENT_COMMAND_STATUS 0x0f
#define EVENT_LE_META 0x3e
#define LE_SCAN_REQUEST_RECEIVED 0x13

#define OP_SET_EVENT_MASK 0x0c01
#define OP_RESET 0x0c03
#define OP_READ_BD_ADDR 0x1009
#define OP_LE_SET_EVENT_MASK 0x2001
#define OP_LE_SET_EXT_ADV_PARAMETERS 0x2036
#define OP_LE_SET_EXT_ADV_DATA 0x2037
#define OP_LE_SET_EXT_SCAN_RESPONSE_DATA 0x2038
#define OP_LE_SET_EXT_ADV_ENABLE 0x2039

typedef struct {
    uint16_t opcode;
    uint8_t status;
    uint8_t parameters[8];
    uint8_t parameter_length;
} completion_t;

typedef struct {
    uint8_t type;
    uint8_t address[6];
} scan_request_t;

typedef struct {
    scan_request_t scanner;
    uint32_t count;
} scanner_count_t;

static QueueHandle_t s_completions;
static QueueHandle_t s_scan_requests;
static SemaphoreHandle_t s_send_available;

static void notify_host_send_available(void)
{
    xSemaphoreGive(s_send_available);
}

/* Runs in the Controller's context: copy the event and return. */
static int notify_host_recv(uint8_t *data, uint16_t length)
{
    if (length < 3 || data[0] != HCI_EVENT) {
        return 0;
    }
    if (data[1] == EVENT_LE_META && length >= 12 && data[3] == LE_SCAN_REQUEST_RECEIVED) {
        scan_request_t request = { .type = data[5] };
        memcpy(request.address, &data[6], 6);
        xQueueSend(s_scan_requests, &request, 0);
        return 0;
    }
    completion_t completion = { 0 };
    if (data[1] == EVENT_COMMAND_COMPLETE && length >= 7) {
        completion.opcode = (uint16_t)(data[4] | data[5] << 8);
        completion.status = data[6];
        uint16_t available = (uint16_t)(length - 7);
        completion.parameter_length = (uint8_t)(available > 8 ? 8 : available);
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
    uint8_t packet[4 + 40];
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

static void reply_error(const char *command, const char *reason)
{
    printf("@ERR %s %s\n", command, reason);
    fflush(stdout);
}

/* Run one command; answers `@ERR` and returns false when it failed. */
static bool run(const char *command, uint16_t opcode, const uint8_t *parameters, uint8_t length)
{
    completion_t completion;
    if (!hci_command(opcode, parameters, length, &completion)) {
        reply_error(command, "timeout");
        return false;
    }
    if (completion.status != 0) {
        char reason[24];
        snprintf(reason, sizeof(reason), "0x%04x:status=0x%02x", opcode, completion.status);
        reply_error(command, reason);
        return false;
    }
    return true;
}

static void print_address(void)
{
    completion_t completion;
    if (hci_command(OP_READ_BD_ADDR, NULL, 0, &completion) && completion.status == 0 &&
        completion.parameter_length >= 6) {
        const uint8_t *a = completion.parameters;
        printf("@ADDR %02x:%02x:%02x:%02x:%02x:%02x\n", a[5], a[4], a[3], a[2], a[1], a[0]);
        fflush(stdout);
    }
}

static bool reset(const char *command)
{
    return run(command, OP_RESET, NULL, 0);
}

static void count(scanner_count_t *scanners, size_t *used, const scan_request_t *request)
{
    for (size_t index = 0; index < *used; index++) {
        if (scanners[index].scanner.type == request->type &&
            memcmp(scanners[index].scanner.address, request->address, 6) == 0) {
            scanners[index].count++;
            return;
        }
    }
    if (*used < SCANNERS) {
        scanners[*used].scanner = *request;
        scanners[*used].count = 1;
        (*used)++;
    }
}

static void command_adv(char **argv, int argc)
{
    char *end;
    unsigned long duration = argc == 2 ? strtoul(argv[1], &end, 10) : 0;
    if (argc != 2 || *end != '\0' || duration == 0 || duration > 600000) {
        reply_error("ADV", "invalid");
        return;
    }
    /* The default mask plus LE Meta; LE: the default plus Scan Request
     * Received (bit 18). */
    static const uint8_t event_mask[] = { 0xff, 0xff, 0xff, 0xff, 0xff, 0x1f, 0x00, 0x20 };
    static const uint8_t le_event_mask[] = { 0x1f, 0x00, 0x04, 0x00, 0x00, 0x00, 0x00, 0x00 };
    /* Set 0: legacy ADV_SCAN_IND (scannable | legacy), 30 ms on all three
     * primary channels, public address, scan-request notification. */
    static const uint8_t parameters[] = {
        0x00, 0x12, 0x00, 0x30, 0x00, 0x00, 0x30, 0x00, 0x00, 0x07, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x7f, 0x01, 0x00, 0x01, 0x00, 0x01,
    };
    static const uint8_t data[] = { 0x00, 0x03, 0x01, 0x03, 0x02, 0x01, 0x06 };
    static const uint8_t scan_response[] = {
        0x00, 0x03, 0x01, 0x07, 0x06, 0x09, 'O', 'E', 'R', 'S', 'R',
    };
    static const uint8_t enable[] = { 0x01, 0x01, 0x00, 0x00, 0x00, 0x00 };
    static const uint8_t disable[] = { 0x00, 0x01, 0x00, 0x00, 0x00, 0x00 };
    if (!run("ADV", OP_SET_EVENT_MASK, event_mask, sizeof(event_mask)) ||
        !run("ADV", OP_LE_SET_EVENT_MASK, le_event_mask, sizeof(le_event_mask)) ||
        !run("ADV", OP_LE_SET_EXT_ADV_PARAMETERS, parameters, sizeof(parameters)) ||
        !run("ADV", OP_LE_SET_EXT_ADV_DATA, data, sizeof(data)) ||
        !run("ADV", OP_LE_SET_EXT_SCAN_RESPONSE_DATA, scan_response, sizeof(scan_response))) {
        return;
    }
    xQueueReset(s_scan_requests);
    if (!run("ADV", OP_LE_SET_EXT_ADV_ENABLE, enable, sizeof(enable))) {
        return;
    }
    static scanner_count_t scanners[SCANNERS];
    size_t used = 0;
    uint32_t total = 0;
    TickType_t deadline = xTaskGetTickCount() + pdMS_TO_TICKS(duration);
    for (;;) {
        TickType_t now = xTaskGetTickCount();
        if (now >= deadline) {
            break;
        }
        scan_request_t request;
        if (xQueueReceive(s_scan_requests, &request, deadline - now) == pdTRUE) {
            total++;
            count(scanners, &used, &request);
        }
    }
    run("ADV", OP_LE_SET_EXT_ADV_ENABLE, disable, sizeof(disable));
    printf("@SCANREQ total=%lu\n", (unsigned long)total);
    for (size_t index = 0; index < used; index++) {
        const uint8_t *a = scanners[index].scanner.address;
        printf("@SCANNER %u %02x:%02x:%02x:%02x:%02x:%02x %lu\n", scanners[index].scanner.type,
               a[5], a[4], a[3], a[2], a[1], a[0], (unsigned long)scanners[index].count);
    }
    printf("@OK ADV\n");
    fflush(stdout);
}

static void dispatch(char *line)
{
    char *argv[4];
    int argc = 0;
    for (char *token = strtok(line, " "); token != NULL && argc < 4; token = strtok(NULL, " ")) {
        argv[argc++] = token;
    }
    if (argc == 0) {
        return;
    }
    if (strcmp(argv[0], "ADV") == 0) {
        command_adv(argv, argc);
    } else if (strcmp(argv[0], "SYNC") == 0 && argc == 1) {
        if (reset("SYNC")) {
            printf("@READY protocol=%d target=%s\n", PROTOCOL_VERSION, CONFIG_IDF_TARGET);
            print_address();
            printf("@OK SYNC\n");
            fflush(stdout);
        }
    } else {
        reply_error(argv[0], "unknown");
    }
}

void app_main(void)
{
    keep_rom_i2c_master_clock_map();
    s_completions = xQueueCreate(4, sizeof(completion_t));
    s_scan_requests = xQueueCreate(64, sizeof(scan_request_t));
    s_send_available = xSemaphoreCreateBinary();
    configASSERT(s_completions != NULL && s_scan_requests != NULL && s_send_available != NULL);

    usb_serial_jtag_driver_config_t console = USB_SERIAL_JTAG_DRIVER_CONFIG_DEFAULT();
    ESP_ERROR_CHECK(usb_serial_jtag_driver_install(&console));
    usb_serial_jtag_vfs_use_driver();
    setvbuf(stdin, NULL, _IONBF, 0);

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
    if (!reset("START")) {
        printf("@ERR START reset\n");
    }
    printf("@READY protocol=%d target=%s\n", PROTOCOL_VERSION, CONFIG_IDF_TARGET);
    print_address();
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
