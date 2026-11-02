/*
 * Thread reference peer for the open-esp-radio HIL.
 *
 * ESP-IDF's OpenThread stack runs on the native IEEE 802.15.4 radio; this
 * application only drives the OpenThread API through a line protocol on the
 * console. Every protocol line starts with '@'; the host ignores any other
 * output. See README.md.
 */

#include <stdbool.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include "driver/usb_serial_jtag.h"
#include "driver/usb_serial_jtag_vfs.h"
#include "esp_err.h"
#include "esp_event.h"
#include "esp_openthread.h"
#include "esp_openthread_lock.h"
#include "esp_openthread_types.h"
#include "esp_vfs_eventfd.h"
#include "freertos/FreeRTOS.h"
#include "freertos/task.h"
#include "hal/pmu_types.h"
#include "modem/modem_lpcon_struct.h"
#include "nvs_flash.h"
#include "openthread/dataset.h"
#include "openthread/dataset_ftd.h"
#include "openthread/instance.h"
#include "openthread/ip6.h"
#include "openthread/message.h"
#include "openthread/thread.h"
#include "openthread/udp.h"
#include "sdkconfig.h"

/* ESP-IDF enables the analog I2C master clock in the PMU's MODEM state as well
 * as ACTIVE. That map survives a USB Serial/JTAG (RTS) reset of the HP system,
 * after which the esp32c5 rev 1.0 ROM boots into UART/SDIO download with USB
 * dead. The peer never enters the MODEM state (it uses no power management),
 * so keep the ROM's ACTIVE-only map, as the IEEE 802.15.4 peer does. */
static void keep_rom_i2c_master_clock_map(void)
{
    MODEM_LPCON.clk_conf_power_st.clk_i2c_mst_st_map = BIT(PMU_HP_ICG_MODEM_CODE_ACTIVE);
}

#define PROTOCOL_VERSION 1
#define LINE_CAPACITY 320
/* UDP payloads the protocol carries, in bytes. */
#define MAX_PAYLOAD 128

static otUdpSocket s_socket;
static bool s_socket_open;

static void reply(otError error, const char *what)
{
    if (error == OT_ERROR_NONE) {
        printf("@OK %s\n", what);
    } else {
        printf("@ERR %s %s\n", what, otThreadErrorToString(error));
    }
    fflush(stdout);
}

static void print_hex(const uint8_t *bytes, size_t length)
{
    for (size_t index = 0; index < length; index++) {
        printf("%02x", bytes[index]);
    }
}

static int hex_value(char digit)
{
    if (digit >= '0' && digit <= '9') {
        return digit - '0';
    }
    if (digit >= 'a' && digit <= 'f') {
        return digit - 'a' + 10;
    }
    if (digit >= 'A' && digit <= 'F') {
        return digit - 'A' + 10;
    }
    return -1;
}

/* Decode hex into `out`; returns the byte count, or -1 on malformed text. */
static int decode_hex(const char *text, uint8_t *out, size_t capacity)
{
    size_t digits = strlen(text);
    if (digits % 2 != 0 || digits / 2 > capacity) {
        return -1;
    }
    for (size_t index = 0; index < digits / 2; index++) {
        int high = hex_value(text[2 * index]);
        int low = hex_value(text[2 * index + 1]);
        if (high < 0 || low < 0) {
            return -1;
        }
        out[index] = (uint8_t)(high << 4 | low);
    }
    return (int)(digits / 2);
}

static bool parse_u32(const char *text, uint32_t *out, uint32_t maximum)
{
    char *end = NULL;
    unsigned long value = strtoul(text, &end, 0);
    if (text[0] == '\0' || *end != '\0' || value > maximum) {
        return false;
    }
    *out = (uint32_t)value;
    return true;
}

/* Runs in the OpenThread task, which holds the OpenThread lock. */
static void udp_received(void *context, otMessage *message, const otMessageInfo *info)
{
    (void)context;
    uint8_t payload[MAX_PAYLOAD];
    uint16_t length = otMessageRead(message, otMessageGetOffset(message), payload,
                                    sizeof(payload));
    char source[OT_IP6_ADDRESS_STRING_SIZE];
    otIp6AddressToString(&info->mPeerAddr, source, sizeof(source));
    printf("@UDPRX %s %u ", source, info->mPeerPort);
    print_hex(payload, length);
    printf("\n");
    fflush(stdout);
}

/* FORM <channel> <panid> : a new network, this device its leader. */
static otError command_form(otInstance *instance, char **argv, int argc)
{
    uint32_t channel, pan_id;
    if (argc != 3 || !parse_u32(argv[1], &channel, 26) || channel < 11
        || !parse_u32(argv[2], &pan_id, 0xfffe)) {
        return OT_ERROR_INVALID_ARGS;
    }
    /* Each network starts from nothing the previous one stored. */
    otThreadSetEnabled(instance, false);
    otIp6SetEnabled(instance, false);
    otError error = otInstanceErasePersistentInfo(instance);
    otOperationalDataset dataset;
    if (error == OT_ERROR_NONE) {
        error = otDatasetCreateNewNetwork(instance, &dataset);
    }
    if (error == OT_ERROR_NONE) {
        dataset.mChannel = (uint16_t)channel;
        dataset.mPanId = (otPanId)pan_id;
        dataset.mChannelMask = 1u << channel;
        error = otDatasetSetActive(instance, &dataset);
    }
    if (error == OT_ERROR_NONE) {
        error = otIp6SetEnabled(instance, true);
    }
    if (error == OT_ERROR_NONE) {
        error = otThreadSetEnabled(instance, true);
    }
    return error;
}

/* DATASET : the active operational dataset as TLV hex. */
static otError command_dataset(otInstance *instance)
{
    otOperationalDatasetTlvs tlvs;
    otError error = otDatasetGetActiveTlvs(instance, &tlvs);
    if (error == OT_ERROR_NONE) {
        printf("@DATASET ");
        print_hex(tlvs.mTlvs, tlvs.mLength);
        printf("\n");
    }
    return error;
}

/* STATE : role, RLOC16 and mesh-local EID. */
static otError command_state(otInstance *instance)
{
    char eid[OT_IP6_ADDRESS_STRING_SIZE];
    otIp6AddressToString(otThreadGetMeshLocalEid(instance), eid, sizeof(eid));
    printf("@STATE role=%s rloc16=%04x eid=%s\n",
           otThreadDeviceRoleToString(otThreadGetDeviceRole(instance)),
           otThreadGetRloc16(instance), eid);
    return OT_ERROR_NONE;
}

/* UDP OPEN <port> | UDP SEND <address> <port> <hex> */
static otError command_udp(otInstance *instance, char **argv, int argc)
{
    uint32_t port;
    if (argc == 3 && strcmp(argv[1], "OPEN") == 0 && parse_u32(argv[2], &port, UINT16_MAX)) {
        if (s_socket_open) {
            otUdpClose(instance, &s_socket);
            s_socket_open = false;
        }
        otError error = otUdpOpen(instance, &s_socket, udp_received, NULL);
        if (error != OT_ERROR_NONE) {
            return error;
        }
        s_socket_open = true;
        otSockAddr name = { .mPort = (uint16_t)port };
        return otUdpBind(instance, &s_socket, &name, OT_NETIF_THREAD_INTERNAL);
    }
    if (argc == 5 && strcmp(argv[1], "SEND") == 0 && s_socket_open
        && parse_u32(argv[3], &port, UINT16_MAX)) {
        otMessageInfo info;
        memset(&info, 0, sizeof(info));
        uint8_t payload[MAX_PAYLOAD];
        int length = decode_hex(argv[4], payload, sizeof(payload));
        if (length <= 0 || otIp6AddressFromString(argv[2], &info.mPeerAddr) != OT_ERROR_NONE) {
            return OT_ERROR_INVALID_ARGS;
        }
        info.mPeerPort = (uint16_t)port;
        otMessage *message = otUdpNewMessage(instance, NULL);
        if (message == NULL) {
            return OT_ERROR_NO_BUFS;
        }
        otError error = otMessageAppend(message, payload, (uint16_t)length);
        if (error == OT_ERROR_NONE) {
            error = otUdpSend(instance, &s_socket, message, &info);
        }
        if (error != OT_ERROR_NONE) {
            otMessageFree(message);
        }
        return error;
    }
    return OT_ERROR_INVALID_ARGS;
}

/* SYNC: leave the network and close the socket, then report ready again, so
 * the host takes over a running peer without resetting the chip. */
static otError command_sync(otInstance *instance)
{
    if (s_socket_open) {
        otUdpClose(instance, &s_socket);
        s_socket_open = false;
    }
    otThreadSetEnabled(instance, false);
    return otIp6SetEnabled(instance, false);
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
    if (strcmp(argv[0], "OFF") == 0 && argc == 1) {
        /* Stop OpenThread, which disables the IEEE 802.15.4 driver and
         * closes RF; the peer then only answers after a reset. */
        esp_openthread_lock_acquire(portMAX_DELAY);
        command_sync(esp_openthread_get_instance());
        esp_openthread_lock_release();
        esp_err_t stopped = esp_openthread_stop();
        reply(stopped == ESP_OK ? OT_ERROR_NONE : OT_ERROR_FAILED, "OFF");
        return;
    }
    otInstance *instance = esp_openthread_get_instance();
    otError error = OT_ERROR_INVALID_COMMAND;
    esp_openthread_lock_acquire(portMAX_DELAY);
    if (strcmp(argv[0], "FORM") == 0) {
        error = command_form(instance, argv, argc);
    } else if (strcmp(argv[0], "DATASET") == 0 && argc == 1) {
        error = command_dataset(instance);
    } else if (strcmp(argv[0], "STATE") == 0 && argc == 1) {
        error = command_state(instance);
    } else if (strcmp(argv[0], "UDP") == 0) {
        error = command_udp(instance, argv, argc);
    } else if (strcmp(argv[0], "SYNC") == 0 && argc == 1) {
        error = command_sync(instance);
        if (error == OT_ERROR_NONE) {
            printf("@READY protocol=%d target=%s stack=openthread\n", PROTOCOL_VERSION,
                   CONFIG_IDF_TARGET);
        }
    }
    esp_openthread_lock_release();
    reply(error, argv[0]);
}

static void console_init(void)
{
    usb_serial_jtag_driver_config_t config = USB_SERIAL_JTAG_DRIVER_CONFIG_DEFAULT();
    ESP_ERROR_CHECK(usb_serial_jtag_driver_install(&config));
    usb_serial_jtag_vfs_use_driver();
    setvbuf(stdin, NULL, _IONBF, 0);
}

void app_main(void)
{
    /* The OpenThread task queue and the radio driver each use an eventfd. */
    keep_rom_i2c_master_clock_map();
    esp_vfs_eventfd_config_t eventfd_config = { .max_fds = 3 };
    ESP_ERROR_CHECK(nvs_flash_init());
    ESP_ERROR_CHECK(esp_event_loop_create_default());
    ESP_ERROR_CHECK(esp_vfs_eventfd_register(&eventfd_config));
    console_init();

    static esp_openthread_config_t config = {
        .platform_config = {
            .radio_config = { .radio_mode = RADIO_MODE_NATIVE },
            .host_config = { .host_connection_mode = HOST_CONNECTION_MODE_NONE },
            .port_config = {
                .storage_partition_name = "nvs",
                .netif_queue_size = 10,
                .task_queue_size = 10,
            },
        },
    };
    ESP_ERROR_CHECK(esp_openthread_start(&config));
    /* OpenThread enabled the radio while it started. */
    keep_rom_i2c_master_clock_map();
    printf("@READY protocol=%d target=%s stack=openthread\n", PROTOCOL_VERSION,
           CONFIG_IDF_TARGET);
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
        } else if (length + 1 < LINE_CAPACITY) {
            line[length++] = (char)character;
        }
    }
}
