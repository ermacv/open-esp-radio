/*
 * IEEE 802.11 frame source for the open-esp-radio HIL.
 *
 * The vendor Wi-Fi driver owns the radio in station mode, never associated;
 * this application only drives its raw transmission
 * (`esp_wifi_80211_tx`) at one fixed PHY rate
 * (`esp_wifi_config_80211_tx`) through a line protocol on the console, so a
 * receiver under test hears unicast data frames of a chosen format from a
 * vendor transmitter and the vendor MAC reports whether each one was
 * acknowledged. Every protocol line starts with '@'; the host ignores any
 * other output. See README.md.
 */

#include <stdatomic.h>
#include <stdbool.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include "driver/usb_serial_jtag.h"
#include "driver/usb_serial_jtag_vfs.h"
#include "esp_err.h"
#include "esp_event.h"
#include "esp_mac.h"
#include "esp_timer.h"
#include "esp_wifi.h"
#include "freertos/FreeRTOS.h"
#include "freertos/task.h"
#include "nvs_flash.h"
#include "sdkconfig.h"

#if CONFIG_IDF_TARGET_ESP32C5
#include "hal/pmu_types.h"
#include "modem/modem_lpcon_struct.h"
#endif

/* The ROM's ACTIVE-only analog I2C master clock map keeps a USB Serial/JTAG
 * reset of the ESP32-C5 rev 1.0 out of download mode (see the IEEE 802.15.4
 * peer's README). Power save stays off, so the PMU never enters its MODEM
 * state. The ESP32-S31 vendor reference builds the same file and keeps
 * ESP-IDF's own map. */
static void keep_rom_i2c_master_clock_map(void)
{
#if CONFIG_IDF_TARGET_ESP32C5
    MODEM_LPCON.clk_conf_power_st.clk_i2c_mst_st_map = BIT(PMU_HP_ICG_MODEM_CODE_ACTIVE);
#endif
}

#define PROTOCOL_VERSION 1
#define LINE_CAPACITY 160
#define HEADER_LEN 24
/* esp_wifi_80211_tx takes at most 1500 bytes. */
#define MAX_FRAME 1500
#define MAX_DURATION_MS 600000
/* Frames handed to the driver whose completion was not reported yet. */
#define MAX_IN_FLIGHT 4
/* How long the last frames of a TX may take to complete. */
#define DRAIN_MS 500

typedef struct {
    const char *name;
    wifi_phy_rate_t rate;
    /* 0 DSSS/CCK, 1 OFDM, 2 HT. */
    uint8_t family;
} rate_name_t;

static const rate_name_t RATES[] = {
    { "1m", WIFI_PHY_RATE_1M_L, 0 },     { "2m", WIFI_PHY_RATE_2M_L, 0 },
    { "5.5m", WIFI_PHY_RATE_5M_L, 0 },   { "11m", WIFI_PHY_RATE_11M_L, 0 },
    { "6m", WIFI_PHY_RATE_6M, 1 },       { "9m", WIFI_PHY_RATE_9M, 1 },
    { "12m", WIFI_PHY_RATE_12M, 1 },     { "18m", WIFI_PHY_RATE_18M, 1 },
    { "24m", WIFI_PHY_RATE_24M, 1 },     { "36m", WIFI_PHY_RATE_36M, 1 },
    { "48m", WIFI_PHY_RATE_48M, 1 },     { "54m", WIFI_PHY_RATE_54M, 1 },
    { "mcs0l", WIFI_PHY_RATE_MCS0_LGI, 2 }, { "mcs1l", WIFI_PHY_RATE_MCS1_LGI, 2 },
    { "mcs2l", WIFI_PHY_RATE_MCS2_LGI, 2 }, { "mcs3l", WIFI_PHY_RATE_MCS3_LGI, 2 },
    { "mcs4l", WIFI_PHY_RATE_MCS4_LGI, 2 }, { "mcs5l", WIFI_PHY_RATE_MCS5_LGI, 2 },
    { "mcs6l", WIFI_PHY_RATE_MCS6_LGI, 2 }, { "mcs7l", WIFI_PHY_RATE_MCS7_LGI, 2 },
    { "mcs0s", WIFI_PHY_RATE_MCS0_SGI, 2 }, { "mcs1s", WIFI_PHY_RATE_MCS1_SGI, 2 },
    { "mcs2s", WIFI_PHY_RATE_MCS2_SGI, 2 }, { "mcs3s", WIFI_PHY_RATE_MCS3_SGI, 2 },
    { "mcs4s", WIFI_PHY_RATE_MCS4_SGI, 2 }, { "mcs5s", WIFI_PHY_RATE_MCS5_SGI, 2 },
    { "mcs6s", WIFI_PHY_RATE_MCS6_SGI, 2 }, { "mcs7s", WIFI_PHY_RATE_MCS7_SGI, 2 },
};

static bool s_started;
static uint8_t s_receiver[6];
static atomic_uint s_in_flight;
static atomic_uint s_acked;
static atomic_uint s_failed;
static atomic_uint s_other;

static void reply_error(const char *command, const char *reason)
{
    printf("@ERR %s %s\n", command, reason);
    fflush(stdout);
}

static void reply_esp_error(const char *command, const char *call, esp_err_t error)
{
    printf("@ERR %s %s=0x%x\n", command, call, (unsigned)error);
    fflush(stdout);
}

static void reply_ok(const char *command)
{
    printf("@OK %s\n", command);
    fflush(stdout);
}

/* Runs in the Wi-Fi task: count the end of one of our frames. */
static void tx_done(const esp_80211_tx_info_t *info)
{
    if (info == NULL || info->des_addr == NULL || memcmp(info->des_addr, s_receiver, 6) != 0) {
        atomic_fetch_add(&s_other, 1);
        return;
    }
    if (info->tx_status == WIFI_SEND_SUCCESS) {
        atomic_fetch_add(&s_acked, 1);
    } else {
        atomic_fetch_add(&s_failed, 1);
    }
    unsigned in_flight = atomic_load(&s_in_flight);
    while (in_flight > 0 && !atomic_compare_exchange_weak(&s_in_flight, &in_flight, in_flight - 1)) {
    }
}

static bool parse_address(const char *text, uint8_t address[6])
{
    unsigned bytes[6];
    char tail;
    if (sscanf(text, "%2x:%2x:%2x:%2x:%2x:%2x%c", &bytes[0], &bytes[1], &bytes[2], &bytes[3],
               &bytes[4], &bytes[5], &tail) != 6) {
        return false;
    }
    for (int index = 0; index < 6; index++) {
        address[index] = (uint8_t)bytes[index];
    }
    return true;
}

static bool parse_unsigned(const char *text, unsigned long maximum, unsigned long *value)
{
    char *end;
    unsigned long parsed = strtoul(text, &end, 10);
    if (*text == '\0' || *end != '\0' || parsed > maximum) {
        return false;
    }
    *value = parsed;
    return true;
}

static const rate_name_t *find_rate(const char *name)
{
    for (size_t index = 0; index < sizeof(RATES) / sizeof(RATES[0]); index++) {
        if (strcmp(RATES[index].name, name) == 0) {
            return &RATES[index];
        }
    }
    return NULL;
}

static void print_address(void)
{
    uint8_t mac[6];
    if (esp_wifi_get_mac(WIFI_IF_STA, mac) == ESP_OK) {
        printf("@ADDR %02x:%02x:%02x:%02x:%02x:%02x\n", mac[0], mac[1], mac[2], mac[3], mac[4],
               mac[5]);
        fflush(stdout);
    }
}

/* `MODE <channel> <width> <rate>`: tune, then fix the raw-transmission
 * rate. The pinned vendor refuses the rate (ESP_FAIL) until the station
 * interface exists, which esp_wifi_start creates, and an HT40 rate until
 * the interface is 40 MHz wide. */
static void command_mode(char **argv, int argc)
{
    unsigned long channel;
    if (argc != 4 || !parse_unsigned(argv[1], 13, &channel) || channel == 0) {
        reply_error("MODE", "invalid");
        return;
    }
    wifi_second_chan_t second;
    if (strcmp(argv[2], "20") == 0) {
        second = WIFI_SECOND_CHAN_NONE;
    } else if (strcmp(argv[2], "40-") == 0) {
        second = WIFI_SECOND_CHAN_BELOW;
    } else if (strcmp(argv[2], "40+") == 0) {
        second = WIFI_SECOND_CHAN_ABOVE;
    } else {
        reply_error("MODE", "width");
        return;
    }
    const rate_name_t *rate = find_rate(argv[3]);
    if (rate == NULL) {
        reply_error("MODE", "rate");
        return;
    }
    wifi_tx_rate_config_t config = {
        .phymode = rate->family == 0   ? WIFI_PHY_MODE_11B
                   : rate->family == 1 ? WIFI_PHY_MODE_11G
                   : second == WIFI_SECOND_CHAN_NONE ? WIFI_PHY_MODE_HT20
                                                     : WIFI_PHY_MODE_HT40,
        .rate = rate->rate,
        .ersu = false,
        .dcm = false,
    };
    esp_err_t error;
    if (!s_started) {
        if ((error = esp_wifi_start()) != ESP_OK) {
            reply_esp_error("MODE", "start", error);
            return;
        }
        s_started = true;
        keep_rom_i2c_master_clock_map();
        if ((error = esp_wifi_set_ps(WIFI_PS_NONE)) != ESP_OK) {
            reply_esp_error("MODE", "set_ps", error);
            return;
        }
    }
    wifi_bandwidth_t width = second == WIFI_SECOND_CHAN_NONE ? WIFI_BW20 : WIFI_BW40;
    if ((error = esp_wifi_set_bandwidth(WIFI_IF_STA, width)) != ESP_OK) {
        reply_esp_error("MODE", "set_bandwidth", error);
        return;
    }
    if ((error = esp_wifi_set_channel((uint8_t)channel, second)) != ESP_OK) {
        reply_esp_error("MODE", "set_channel", error);
        return;
    }
    if ((error = esp_wifi_config_80211_tx(WIFI_IF_STA, &config)) != ESP_OK) {
        reply_esp_error("MODE", "config_80211_tx", error);
        return;
    }
    uint8_t primary = 0;
    wifi_second_chan_t tuned = WIFI_SECOND_CHAN_NONE;
    if (esp_wifi_get_channel(&primary, &tuned) == ESP_OK) {
        printf("@MODE channel=%u second=%d phymode=%d rate=0x%02x\n", primary, (int)tuned,
               (int)config.phymode, (unsigned)config.rate);
    }
    reply_ok("MODE");
}

/* `TX <ms> <interval_us> <length> <addr1> <addr2> <addr3> <ds>`: send
 * non-QoS data frames of `length` bytes (header included, FCS excluded) every
 * `interval_us` for `ms` milliseconds, at most MAX_IN_FLIGHT unfinished. The
 * pinned vendor refuses QoS data on this path ("unsupport QoS frame type"). */
static void command_tx(char **argv, int argc)
{
    unsigned long duration, interval, length, ds;
    uint8_t addr1[6], addr2[6], addr3[6];
    if (argc != 8 || !parse_unsigned(argv[1], MAX_DURATION_MS, &duration) ||
        !parse_unsigned(argv[2], 1000000, &interval) ||
        !parse_unsigned(argv[3], MAX_FRAME, &length) || length < HEADER_LEN + 8 ||
        !parse_address(argv[4], addr1) || !parse_address(argv[5], addr2) ||
        !parse_address(argv[6], addr3) || !parse_unsigned(argv[7], 3, &ds)) {
        reply_error("TX", "invalid");
        return;
    }
    if (!s_started) {
        reply_error("TX", "no-mode");
        return;
    }
    static uint8_t frame[MAX_FRAME];
    memset(frame, 0, sizeof(frame));
    /* Data, subtype 0; ToDS and FromDS in the flags. */
    frame[0] = 0x08;
    frame[1] = (uint8_t)ds;
    memcpy(&frame[4], addr1, 6);
    memcpy(&frame[10], addr2, 6);
    memcpy(&frame[16], addr3, 6);
    /* LLC/SNAP with the local experimental EtherType 0x88b5. */
    static const uint8_t snap[] = { 0xaa, 0xaa, 0x03, 0x00, 0x00, 0x00, 0x88, 0xb5 };
    memcpy(&frame[HEADER_LEN], snap, sizeof(snap));
    for (size_t index = HEADER_LEN + sizeof(snap); index < length; index++) {
        frame[index] = (uint8_t)index;
    }
    memcpy(s_receiver, addr1, 6);
    atomic_store(&s_in_flight, 0);
    atomic_store(&s_acked, 0);
    atomic_store(&s_failed, 0);
    atomic_store(&s_other, 0);
    uint32_t sent = 0, refused = 0, waited = 0;
    esp_err_t last_refusal = ESP_OK;
    int64_t start = esp_timer_get_time();
    int64_t end = start + (int64_t)duration * 1000;
    int64_t next = start;
    for (;;) {
        int64_t now = esp_timer_get_time();
        if (now >= end) {
            break;
        }
        if (now < next || atomic_load(&s_in_flight) >= MAX_IN_FLIGHT) {
            if (now < next && next - now > 2 * portTICK_PERIOD_MS * 1000) {
                vTaskDelay(1);
            } else {
                waited++;
                taskYIELD();
            }
            continue;
        }
        next += (int64_t)interval;
        atomic_fetch_add(&s_in_flight, 1);
        esp_err_t error = esp_wifi_80211_tx(WIFI_IF_STA, frame, (int)length, true);
        if (error == ESP_OK) {
            sent++;
        } else {
            unsigned in_flight = atomic_load(&s_in_flight);
            while (in_flight > 0 &&
                   !atomic_compare_exchange_weak(&s_in_flight, &in_flight, in_flight - 1)) {
            }
            refused++;
            last_refusal = error;
            vTaskDelay(1);
        }
    }
    int64_t drain = esp_timer_get_time() + DRAIN_MS * 1000;
    while (atomic_load(&s_in_flight) > 0 && esp_timer_get_time() < drain) {
        vTaskDelay(1);
    }
    printf("@TX sent=%lu refused=%lu acked=%u failed=%u pending=%u other=%u last_refusal=0x%x\n",
           (unsigned long)sent, (unsigned long)refused, atomic_load(&s_acked),
           atomic_load(&s_failed), atomic_load(&s_in_flight), atomic_load(&s_other),
           (unsigned)last_refusal);
    reply_ok("TX");
}

static void dispatch(char *line)
{
    char *argv[9];
    int argc = 0;
    for (char *token = strtok(line, " "); token != NULL && argc < 9; token = strtok(NULL, " ")) {
        argv[argc++] = token;
    }
    if (argc == 0) {
        return;
    }
    if (strcmp(argv[0], "MODE") == 0) {
        command_mode(argv, argc);
    } else if (strcmp(argv[0], "TX") == 0) {
        command_tx(argv, argc);
    } else if (strcmp(argv[0], "SYNC") == 0 && argc == 1) {
        printf("@READY protocol=%d target=%s\n", PROTOCOL_VERSION, CONFIG_IDF_TARGET);
        print_address();
        reply_ok("SYNC");
    } else {
        reply_error(argv[0], "unknown");
    }
}

void app_main(void)
{
    keep_rom_i2c_master_clock_map();
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
    ESP_ERROR_CHECK(esp_event_loop_create_default());
    wifi_init_config_t init = WIFI_INIT_CONFIG_DEFAULT();
    ESP_ERROR_CHECK(esp_wifi_init(&init));
    ESP_ERROR_CHECK(esp_wifi_set_storage(WIFI_STORAGE_RAM));
    ESP_ERROR_CHECK(esp_wifi_set_mode(WIFI_MODE_STA));
#if CONFIG_SOC_WIFI_SUPPORT_5G
    ESP_ERROR_CHECK(esp_wifi_set_band_mode(WIFI_BAND_MODE_2G_ONLY));
#endif
    /* Channels 1 to 13. */
    ESP_ERROR_CHECK(esp_wifi_set_country_code("DE", false));
    ESP_ERROR_CHECK(esp_wifi_set_protocol(WIFI_IF_STA, WIFI_PROTOCOL_11B | WIFI_PROTOCOL_11G |
                                                           WIFI_PROTOCOL_11N));
    ESP_ERROR_CHECK(esp_wifi_register_80211_tx_cb(tx_done));
    keep_rom_i2c_master_clock_map();
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
