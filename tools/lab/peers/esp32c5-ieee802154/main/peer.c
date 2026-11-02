/*
 * IEEE 802.15.4 reference peer for the open-esp-radio HIL.
 *
 * The vendor driver (`esp_ieee802154_*`) owns the radio; this application
 * only drives it through a line protocol on the console. Every protocol line
 * starts with '@'; the host ignores any other output. See README.md.
 */

#include <ctype.h>
#include <stdatomic.h>
#include <stdbool.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include "esp_attr.h"
#include "esp_err.h"
#include "esp_ieee802154.h"
#include "esp_private/regi2c_ctrl.h"
#include "esp_timer.h"
#include "freertos/FreeRTOS.h"
#include "freertos/queue.h"
#include "freertos/task.h"
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
 * map; the radio enable path does not rewrite it, but reassert it anyway.
 * The ESP32-S31 vendor reference builds the same file and keeps ESP-IDF's
 * own map. */
static void keep_rom_i2c_master_clock_map(void)
{
#if CONFIG_IDF_TARGET_ESP32C5
    MODEM_LPCON.clk_conf_power_st.clk_i2c_mst_st_map = BIT(PMU_HP_ICG_MODEM_CODE_ACTIVE);
#endif
}


#define PROTOCOL_VERSION 1
/* PHR length field: MAC bytes plus the two-byte FCS. */
#define MAX_PSDU 127
#define FCS_LEN 2
#define LINE_CAPACITY 320
#define EVENT_QUEUE_DEPTH 16

typedef enum {
    EVENT_RX,
    EVENT_TX_DONE,
    EVENT_TX_FAILED,
    EVENT_ED_DONE,
} event_kind_t;

typedef struct {
    event_kind_t kind;
    /* MAC bytes without PHR and FCS. */
    uint8_t length;
    uint8_t bytes[MAX_PSDU];
    /* RX: the frame; TX_DONE: the received acknowledgement, if any. */
    bool has_ack;
    bool pending;
    uint8_t channel;
    int8_t rssi;
    uint8_t lqi;
    int32_t code;
} peer_event_t;

static QueueHandle_t s_events;
static uint8_t s_tx_frame[1 + MAX_PSDU];

/* A burst repeats one frame back to back: the event task starts the next
 * transmission when the previous one ends, and counts the ends instead of
 * printing them. `s_burst_in_flight` marks a transmission the burst owns, so
 * the one that ends after BURST STOP is counted, not reported. */
static uint8_t s_burst_frame[1 + MAX_PSDU];
static atomic_bool s_burst_active;
static atomic_bool s_burst_in_flight;
static atomic_uint s_burst_sent;
static atomic_uint s_burst_failed;

static void copy_frame(peer_event_t *event, const uint8_t *frame)
{
    uint8_t psdu = frame[0];
    uint8_t length = psdu >= FCS_LEN ? psdu - FCS_LEN : 0;
    if (length > MAX_PSDU) {
        length = MAX_PSDU;
    }
    event->length = length;
    memcpy(event->bytes, &frame[1], length);
}

static void post(const peer_event_t *event)
{
    BaseType_t woken = pdFALSE;
    xQueueSendFromISR(s_events, event, &woken);
    portYIELD_FROM_ISR(woken);
}

/* Driver callbacks run in interrupt context: copy and defer. */

void IRAM_ATTR esp_ieee802154_receive_done(uint8_t *frame, esp_ieee802154_frame_info_t *frame_info)
{
    peer_event_t event = {
        .kind = EVENT_RX,
        .pending = frame_info->pending,
        .channel = frame_info->channel,
        .rssi = frame_info->rssi,
        .lqi = frame_info->lqi,
    };
    copy_frame(&event, frame);
    esp_ieee802154_receive_handle_done(frame);
    post(&event);
}

void IRAM_ATTR esp_ieee802154_transmit_done(const uint8_t *frame, const uint8_t *ack,
                                            esp_ieee802154_frame_info_t *ack_frame_info)
{
    peer_event_t event = { .kind = EVENT_TX_DONE };
    if (ack != NULL) {
        event.has_ack = true;
        event.pending = ack_frame_info->pending;
        event.rssi = ack_frame_info->rssi;
        event.lqi = ack_frame_info->lqi;
        copy_frame(&event, ack);
        esp_ieee802154_receive_handle_done(ack);
    }
    post(&event);
}

void IRAM_ATTR esp_ieee802154_transmit_failed(const uint8_t *frame, esp_ieee802154_tx_error_t error)
{
    peer_event_t event = { .kind = EVENT_TX_FAILED, .code = error };
    post(&event);
}

void IRAM_ATTR esp_ieee802154_energy_detect_done(int8_t power)
{
    peer_event_t event = { .kind = EVENT_ED_DONE, .rssi = power };
    post(&event);
}

static void print_hex(const uint8_t *bytes, uint8_t length)
{
    for (uint8_t index = 0; index < length; index++) {
        printf("%02x", bytes[index]);
    }
}

/* Count the end of a burst transmission and start the next one while the
 * burst runs. Returns false for a transmission the burst does not own. */
static bool burst_transmission_ended(bool sent)
{
    if (!atomic_exchange(&s_burst_in_flight, false)) {
        return false;
    }
    atomic_fetch_add(sent ? &s_burst_sent : &s_burst_failed, 1);
    if (atomic_load(&s_burst_active)) {
        atomic_store(&s_burst_in_flight, true);
        if (esp_ieee802154_transmit(s_burst_frame, false) != ESP_OK) {
            atomic_store(&s_burst_in_flight, false);
            atomic_store(&s_burst_active, false);
            atomic_fetch_add(&s_burst_failed, 1);
        }
    }
    return true;
}

static void event_task(void *arg)
{
    peer_event_t event;
    for (;;) {
        if (xQueueReceive(s_events, &event, portMAX_DELAY) != pdTRUE) {
            continue;
        }
        if ((event.kind == EVENT_TX_DONE || event.kind == EVENT_TX_FAILED)
            && burst_transmission_ended(event.kind == EVENT_TX_DONE)) {
            continue;
        }
        switch (event.kind) {
        case EVENT_RX:
            printf("@RX ");
            print_hex(event.bytes, event.length);
            printf(" rssi=%d lqi=%u pending=%d ch=%u\n", event.rssi, event.lqi, event.pending,
                   event.channel);
            break;
        case EVENT_TX_DONE:
            printf("@TXDONE ack=");
            if (event.has_ack) {
                print_hex(event.bytes, event.length);
                printf(" pending=%d rssi=%d lqi=%u\n", event.pending, event.rssi, event.lqi);
            } else {
                printf("-\n");
            }
            break;
        case EVENT_TX_FAILED:
            printf("@TXFAIL %ld\n", (long)event.code);
            break;
        case EVENT_ED_DONE:
            printf("@ED %d\n", event.rssi);
            break;
        }
        fflush(stdout);
    }
}

static int hex_value(char digit)
{
    if (digit >= '0' && digit <= '9') {
        return digit - '0';
    }
    digit = (char)tolower((unsigned char)digit);
    if (digit >= 'a' && digit <= 'f') {
        return digit - 'a' + 10;
    }
    return -1;
}

/* Decode exactly `length` bytes; the text must hold exactly 2 * length digits. */
static bool decode_hex(const char *text, uint8_t *out, size_t length)
{
    if (strlen(text) != 2 * length) {
        return false;
    }
    for (size_t index = 0; index < length; index++) {
        int high = hex_value(text[2 * index]);
        int low = hex_value(text[2 * index + 1]);
        if (high < 0 || low < 0) {
            return false;
        }
        out[index] = (uint8_t)(high << 4 | low);
    }
    return true;
}

static bool parse_u32(const char *text, uint32_t *out, uint32_t maximum)
{
    char *end = NULL;
    unsigned long value = strtoul(text, &end, 10);
    if (text[0] == '\0' || *end != '\0' || value > maximum) {
        return false;
    }
    *out = (uint32_t)value;
    return true;
}

static bool parse_i8(const char *text, int8_t *out)
{
    char *end = NULL;
    long value = strtol(text, &end, 10);
    if (text[0] == '\0' || *end != '\0' || value < INT8_MIN || value > INT8_MAX) {
        return false;
    }
    *out = (int8_t)value;
    return true;
}

static void reply(esp_err_t error, const char *what)
{
    if (error == ESP_OK) {
        printf("@OK %s\n", what);
    } else {
        printf("@ERR %s %s\n", what, esp_err_to_name(error));
    }
    fflush(stdout);
}

/* CFG <channel> <panid:4hex> <short:4hex> <ext:16hex> <promiscuous> <power_dbm>
 *
 * PAN ID and short address are big-endian numbers; the extended address is
 * in over-the-air byte order, which is also the driver's storage order. The
 * driver always generates automatic acknowledgements. */
static void command_cfg(char **argv, int argc)
{
    uint32_t channel, promiscuous;
    uint8_t panid[2], short_address[2], extended[8];
    int8_t power;
    if (argc != 7 || !parse_u32(argv[1], &channel, 26) || channel < 11
        || !decode_hex(argv[2], panid, 2) || !decode_hex(argv[3], short_address, 2)
        || !decode_hex(argv[4], extended, 8) || !parse_u32(argv[5], &promiscuous, 1)
        || !parse_i8(argv[6], &power)) {
        reply(ESP_ERR_INVALID_ARG, "CFG");
        return;
    }
    esp_err_t error = esp_ieee802154_set_channel((uint8_t)channel);
    if (error == ESP_OK) {
        error = esp_ieee802154_set_panid((uint16_t)(panid[0] << 8 | panid[1]));
    }
    if (error == ESP_OK) {
        error = esp_ieee802154_set_short_address(
            (uint16_t)(short_address[0] << 8 | short_address[1]));
    }
    if (error == ESP_OK) {
        error = esp_ieee802154_set_extended_address(extended);
    }
    if (error == ESP_OK) {
        error = esp_ieee802154_set_promiscuous(promiscuous != 0);
    }
    if (error == ESP_OK) {
        error = esp_ieee802154_set_rx_when_idle(true);
    }
    if (error == ESP_OK) {
        error = esp_ieee802154_set_txpower(power);
    }
    reply(error, "CFG");
}

/* TX <cca 0|1> <MAC bytes as hex, without FCS> */
static void command_tx(char **argv, int argc)
{
    uint32_t cca;
    size_t digits = argc == 3 ? strlen(argv[2]) : 0;
    size_t length = digits / 2;
    if (argc != 3 || !parse_u32(argv[1], &cca, 1) || digits % 2 != 0 || length == 0
        || length + FCS_LEN > MAX_PSDU || !decode_hex(argv[2], &s_tx_frame[1], length)) {
        reply(ESP_ERR_INVALID_ARG, "TX");
        return;
    }
    s_tx_frame[0] = (uint8_t)(length + FCS_LEN);
    reply(esp_ieee802154_transmit(s_tx_frame, cca != 0), "TX");
}

/* TXAT <delay_us> <MAC bytes as hex, without FCS>
 *
 * Transmit without CCA once `delay_us` microseconds have passed, through
 * esp_ieee802154_transmit_at on the esp_timer clock, as ESP-IDF's
 * OpenThread port schedules a delayed transmission. */
static void command_txat(char **argv, int argc)
{
    uint32_t delay;
    size_t digits = argc == 3 ? strlen(argv[2]) : 0;
    size_t length = digits / 2;
    if (argc != 3 || !parse_u32(argv[1], &delay, 1000000) || digits % 2 != 0 || length == 0
        || length + FCS_LEN > MAX_PSDU || !decode_hex(argv[2], &s_tx_frame[1], length)) {
        reply(ESP_ERR_INVALID_ARG, "TXAT");
        return;
    }
    s_tx_frame[0] = (uint8_t)(length + FCS_LEN);
    uint32_t at = (uint32_t)esp_timer_get_time() + delay;
    reply(esp_ieee802154_transmit_at(s_tx_frame, false, at), "TXAT");
}

/* Wait for the burst transmission in flight to end; one frame lasts at most
 * 4.3 ms. Whole ticks: at the default 100 Hz tick a millisecond rounds to no
 * delay at all. Returns whether it ended. */
static bool burst_drain(void)
{
    TickType_t bound = pdMS_TO_TICKS(50) + 1;
    for (TickType_t waited = 0; waited < bound && atomic_load(&s_burst_in_flight); waited++) {
        vTaskDelay(1);
    }
    return !atomic_load(&s_burst_in_flight);
}

/* BURST <MAC bytes as hex, without FCS> | BURST STOP
 *
 * Start transmitting the frame back to back without CCA until BURST STOP,
 * which waits for the last transmission and reports
 * `@BURST sent=<n> failed=<n>` before its reply. */
static void command_burst(char **argv, int argc)
{
    if (argc == 2 && strcmp(argv[1], "STOP") == 0) {
        if (!atomic_exchange(&s_burst_active, false) && !atomic_load(&s_burst_in_flight)) {
            reply(ESP_ERR_INVALID_STATE, "BURST");
            return;
        }
        bool drained = burst_drain();
        printf("@BURST sent=%u failed=%u\n", atomic_load(&s_burst_sent),
               atomic_load(&s_burst_failed));
        reply(drained ? ESP_OK : ESP_ERR_TIMEOUT, "BURST");
        return;
    }
    size_t digits = argc == 2 ? strlen(argv[1]) : 0;
    size_t length = digits / 2;
    if (argc != 2 || digits % 2 != 0 || length == 0 || length + FCS_LEN > MAX_PSDU
        || atomic_load(&s_burst_active) || atomic_load(&s_burst_in_flight)
        || !decode_hex(argv[1], &s_burst_frame[1], length)) {
        reply(ESP_ERR_INVALID_ARG, "BURST");
        return;
    }
    s_burst_frame[0] = (uint8_t)(length + FCS_LEN);
    atomic_store(&s_burst_sent, 0);
    atomic_store(&s_burst_failed, 0);
    atomic_store(&s_burst_active, true);
    atomic_store(&s_burst_in_flight, true);
    esp_err_t error = esp_ieee802154_transmit(s_burst_frame, false);
    if (error != ESP_OK) {
        atomic_store(&s_burst_in_flight, false);
        atomic_store(&s_burst_active, false);
    }
    reply(error, "BURST");
}

/* SYNC: return the driver to its defaults and report ready again, so the
 * host takes over a running peer without resetting the chip. */
static void command_sync(void)
{
    if (atomic_exchange(&s_burst_active, false)) {
        burst_drain();
    }
    esp_err_t error = esp_ieee802154_disable();
    if (error == ESP_OK) {
        error = esp_ieee802154_enable();
        keep_rom_i2c_master_clock_map();
    }
    if (error == ESP_OK) {
        error = esp_ieee802154_reset_pending_table(true);
    }
    if (error == ESP_OK) {
        error = esp_ieee802154_reset_pending_table(false);
    }
    /* Reports of the previous user end here. */
    xQueueReset(s_events);
    atomic_store(&s_burst_in_flight, false);
    if (error == ESP_OK) {
        printf("@READY protocol=%d target=%s\n", PROTOCOL_VERSION, CONFIG_IDF_TARGET);
    }
    reply(error, "SYNC");
}

/* PENDING <mode 0..3> | PENDING ADD <short:4hex> | PENDING CLEAR */
static void command_pending(char **argv, int argc)
{
    uint32_t mode;
    uint8_t short_address[2];
    esp_err_t error = ESP_ERR_INVALID_ARG;
    if (argc == 2 && strcmp(argv[1], "CLEAR") == 0) {
        error = esp_ieee802154_reset_pending_table(true);
    } else if (argc == 3 && strcmp(argv[1], "ADD") == 0 && decode_hex(argv[2], short_address, 2)) {
        /* The driver stores short addresses little-endian. */
        uint8_t stored[2] = { short_address[1], short_address[0] };
        error = esp_ieee802154_add_pending_addr(stored, true);
    } else if (argc == 2 && parse_u32(argv[1], &mode, 3)) {
        error = esp_ieee802154_set_pending_mode((esp_ieee802154_pending_mode_t)mode);
    }
    reply(error, "PENDING");
}

/* ED <duration in 16 us symbols> */
static void command_ed(char **argv, int argc)
{
    uint32_t duration;
    if (argc != 2 || !parse_u32(argv[1], &duration, UINT16_MAX) || duration == 0) {
        reply(ESP_ERR_INVALID_ARG, "ED");
        return;
    }
    reply(esp_ieee802154_energy_detect(duration), "ED");
}

/* PEEK <address:8hex>: the 32-bit word at a device address, for register
 * cross-checks. A register whose clock domain is off may reset the chip. */
static void command_peek(char **argv, int argc)
{
    uint8_t address[4];
    if (argc != 2 || !decode_hex(argv[1], address, 4)) {
        reply(ESP_ERR_INVALID_ARG, "PEEK");
        return;
    }
    uint32_t word = (uint32_t)address[0] << 24 | (uint32_t)address[1] << 16
        | (uint32_t)address[2] << 8 | address[3];
    if (word % 4 != 0) {
        reply(ESP_ERR_INVALID_ARG, "PEEK");
        return;
    }
    uint32_t value = *(volatile const uint32_t *)word;
    printf("@PEEK %08lx %08lx\n", (unsigned long)word, (unsigned long)value);
    fflush(stdout);
}

/* ANALOG <block:2hex> <register:2hex>: one analog-I2C register, read through
 * ESP-IDF's analog-I2C driver, which selects the block's host itself. */
static void command_analog(char **argv, int argc)
{
    uint8_t block, reg;
    if (argc != 3 || !decode_hex(argv[1], &block, 1) || !decode_hex(argv[2], &reg, 1)) {
        reply(ESP_ERR_INVALID_ARG, "ANALOG");
        return;
    }
    uint8_t value = regi2c_ctrl_read_reg(block, 0, reg);
    printf("@ANALOG %02x %02x %02x\n", block, reg, value);
    fflush(stdout);
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
    if (strcmp(argv[0], "CFG") == 0) {
        command_cfg(argv, argc);
    } else if (strcmp(argv[0], "RX") == 0 && argc == 1) {
        reply(esp_ieee802154_receive(), "RX");
    } else if (strcmp(argv[0], "SLEEP") == 0 && argc == 1) {
        reply(esp_ieee802154_sleep(), "SLEEP");
    } else if (strcmp(argv[0], "TX") == 0) {
        command_tx(argv, argc);
    } else if (strcmp(argv[0], "BURST") == 0) {
        command_burst(argv, argc);
    } else if (strcmp(argv[0], "TXAT") == 0) {
        command_txat(argv, argc);
    } else if (strcmp(argv[0], "PENDING") == 0) {
        command_pending(argv, argc);
    } else if (strcmp(argv[0], "ED") == 0) {
        command_ed(argv, argc);
    } else if (strcmp(argv[0], "SYNC") == 0 && argc == 1) {
        command_sync();
    } else if (strcmp(argv[0], "PEEK") == 0) {
        command_peek(argv, argc);
    } else if (strcmp(argv[0], "ANALOG") == 0) {
        command_analog(argv, argc);
    } else if (strcmp(argv[0], "OFF") == 0 && argc == 1) {
        reply(esp_ieee802154_disable(), "OFF");
    } else if (strcmp(argv[0], "ON") == 0 && argc == 1) {
        esp_err_t enabled = esp_ieee802154_enable();
        keep_rom_i2c_master_clock_map();
        reply(enabled, "ON");
    } else {
        printf("@ERR %s unknown\n", argv[0]);
        fflush(stdout);
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

void app_main(void)
{
    keep_rom_i2c_master_clock_map();
    s_events = xQueueCreate(EVENT_QUEUE_DEPTH, sizeof(peer_event_t));
    configASSERT(s_events != NULL);
    console_init();
    ESP_ERROR_CHECK(esp_ieee802154_enable());
    keep_rom_i2c_master_clock_map();
    xTaskCreate(event_task, "peer_events", 4096, NULL, 5, NULL);
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
            printf("@ERR LINE overflow\n");
            fflush(stdout);
        }
    }
}
