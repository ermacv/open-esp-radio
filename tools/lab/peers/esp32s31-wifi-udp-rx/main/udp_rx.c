/*
 * Vendor UDP load receiver for the open-esp-radio stand.
 *
 * The vendor Wi-Fi station and IP stack join the stand's access point with
 * the network a run generated and count the host's UDP load by the load
 * header of hil/services/load/src/header.rs (run, sequence), so the host's
 * load levels run against the vendor driver on the same board as against the
 * open station. A line protocol on the console drives it; every protocol
 * line starts with '@' and the host ignores any other output. The network's
 * name and passphrase arrive in one JOIN line, stay in RAM and are never
 * printed. See README.md.
 */

#include <errno.h>
#include <inttypes.h>
#include <stdbool.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include "driver/usb_serial_jtag.h"
#include "driver/usb_serial_jtag_vfs.h"
#include "esp_err.h"
#include "esp_event.h"
#include "esp_log.h"
#include "esp_mac.h"
#include "esp_netif.h"
#include "esp_private/esp_wifi_he_private.h"
#include "esp_timer.h"
#include "esp_wifi.h"
#include "esp_wifi_he.h"
#include "freertos/FreeRTOS.h"
#include "freertos/event_groups.h"
#include "freertos/semphr.h"
#include "freertos/task.h"
#include "lwip/sockets.h"
#include "lwip/stats.h"
#include "nvs_flash.h"
#include "sdkconfig.h"

#define PROTOCOL_VERSION 1
#define LINE_CAPACITY 192
#define JOIN_TIMEOUT_MS 30000
/* The load header (hil/services/load/src/header.rs), big-endian. */
#define HEADER_LEN 28
#define FLAG_ECHO_REQUEST 1
#define FLAG_ECHO_REPLY 2
/* 2^18 sequences: a 20 s run at 100 Mbit/s of 1472-byte datagrams sends
 * about 170 000. A sequence at or past it counts as untracked. */
#define TALLY_WORDS 4096
#define TALLY_CAPACITY ((uint64_t)TALLY_WORDS * 64)
#define DATAGRAM_CAPACITY 2048
#define MAX_PHASE_MS 600000
#define RECEIVER_PRIORITY 10
#define RECEIVER_STACK 4096

static const char *TAG = "udp-rx";

#define BIT_GOT_IP BIT0
static EventGroupHandle_t s_events;
static esp_netif_t *s_netif;
static volatile bool s_joining;
static volatile bool s_joined;
static volatile uint32_t s_disconnects;
static volatile int32_t s_last_reason;
static bool s_started;

/* One run's count, written by the receiver task alone while it runs and
 * read by the console task once it ended (s_done). */
typedef struct {
    uint32_t run;
    uint64_t datagrams, bytes, window_datagrams, window_bytes;
    uint64_t reordered, duplicates, untracked, foreign;
    uint64_t echoed, echo_refused, receive_errors;
    bool any;
    uint64_t highest;
    int64_t first_us, last_us;
} counts_t;

typedef struct {
    uint32_t link_recv, link_drop, ip_recv, ip_drop, udp_recv, udp_drop, udp_memerr;
} lwip_counts_t;

static uint64_t s_seen[TALLY_WORDS];
static counts_t s_counts;
static int s_socket = -1;
static uint16_t s_port;
static int64_t s_start_us, s_window_start_us, s_window_end_us, s_end_us;
static SemaphoreHandle_t s_done;
static volatile bool s_running;
static lwip_counts_t s_lwip_start;

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

static void print_ready(void)
{
    uint8_t mac[6] = { 0 };
    printf("@READY protocol=%d target=%s app=wifi-udp-rx\n", PROTOCOL_VERSION, CONFIG_IDF_TARGET);
    if (esp_read_mac(mac, ESP_MAC_WIFI_STA) == ESP_OK) {
        printf("@ADDR %02x:%02x:%02x:%02x:%02x:%02x\n", mac[0], mac[1], mac[2], mac[3], mac[4],
               mac[5]);
    }
}

static bool parse_unsigned(const char *text, unsigned long maximum, unsigned long *value)
{
    char *end = NULL;
    unsigned long parsed = strtoul(text, &end, 10);
    if (end == text || *end != '\0' || parsed > maximum) {
        return false;
    }
    *value = parsed;
    return true;
}

static void on_event(void *argument, esp_event_base_t base, int32_t id, void *data)
{
    (void)argument;
    if (base == WIFI_EVENT && id == WIFI_EVENT_STA_DISCONNECTED) {
        const wifi_event_sta_disconnected_t *event = data;
        s_disconnects++;
        s_last_reason = event != NULL ? (int32_t)event->reason : -1;
        if (s_joined) {
            /* The link of a run ended: reported, never rejoined here. */
            s_joined = false;
            printf("@LOST reason=%" PRId32 "\n", s_last_reason);
            fflush(stdout);
        } else if (s_joining) {
            esp_wifi_connect();
        }
    } else if (base == IP_EVENT && id == IP_EVENT_STA_GOT_IP) {
        xEventGroupSetBits(s_events, BIT_GOT_IP);
    }
}

static uint32_t be32(const uint8_t *bytes)
{
    return ((uint32_t)bytes[0] << 24) | ((uint32_t)bytes[1] << 16) | ((uint32_t)bytes[2] << 8) |
           bytes[3];
}

static uint64_t be64(const uint8_t *bytes)
{
    return ((uint64_t)be32(bytes) << 32) | be32(bytes + 4);
}

/* The header of `payload`: false when it holds none (the same refusals as
 * Header::read). */
static bool read_header(const uint8_t *payload, size_t length, uint8_t *flags, uint32_t *run,
                        uint64_t *sequence)
{
    if (length < HEADER_LEN || memcmp(payload, "OERL", 4) != 0) {
        return false;
    }
    if ((payload[4] & ~(FLAG_ECHO_REQUEST | FLAG_ECHO_REPLY)) != 0 || payload[5] != 0 ||
        payload[6] != 0 || payload[7] != 0) {
        return false;
    }
    *flags = payload[4];
    *run = be32(payload + 8);
    *sequence = be64(payload + 12);
    return true;
}

/* Count the arrival of `sequence` (the tally of hil/services/load/src/tally.rs). */
static void count(uint64_t sequence, size_t length, int64_t now)
{
    counts_t *counts = &s_counts;
    if (sequence >= TALLY_CAPACITY) {
        counts->untracked++;
        return;
    }
    uint64_t bit = (uint64_t)1 << (sequence % 64);
    uint64_t *word = &s_seen[sequence / 64];
    if (*word & bit) {
        counts->duplicates++;
        return;
    }
    *word |= bit;
    counts->datagrams++;
    counts->bytes += length;
    if (now >= s_window_start_us && now < s_window_end_us) {
        counts->window_datagrams++;
        counts->window_bytes += length;
    }
    if (counts->first_us < 0) {
        counts->first_us = now - s_start_us;
    }
    counts->last_us = now - s_start_us;
    if (counts->any && sequence < counts->highest) {
        counts->reordered++;
    } else {
        counts->highest = sequence;
        counts->any = true;
    }
}

static void receiver(void *argument)
{
    (void)argument;
    static uint8_t buffer[DATAGRAM_CAPACITY];
    while (esp_timer_get_time() < s_end_us) {
        struct sockaddr_in from;
        socklen_t from_length = sizeof(from);
        int length = recvfrom(s_socket, buffer, sizeof(buffer), 0, (struct sockaddr *)&from,
                              &from_length);
        int64_t now = esp_timer_get_time();
        if (length < 0) {
            if (errno != EAGAIN && errno != EWOULDBLOCK) {
                s_counts.receive_errors++;
                vTaskDelay(1);
            }
            continue;
        }
        uint8_t flags = 0;
        uint32_t run = 0;
        uint64_t sequence = 0;
        if (!read_header(buffer, (size_t)length, &flags, &run, &sequence)) {
            s_counts.foreign++;
        } else if (flags & FLAG_ECHO_REQUEST) {
            /* A probe of any run goes back as its echo (the image's echo). */
            buffer[4] = FLAG_ECHO_REPLY;
            if (sendto(s_socket, buffer, (size_t)length, 0, (struct sockaddr *)&from,
                       from_length) == length) {
                s_counts.echoed++;
            } else {
                s_counts.echo_refused++;
            }
        } else if (run == s_counts.run && !(flags & FLAG_ECHO_REPLY)) {
            count(sequence, (size_t)length, now);
        } else {
            s_counts.foreign++;
        }
    }
    s_running = false;
    xSemaphoreGive(s_done);
    vTaskDelete(NULL);
}

static void lwip_counts(lwip_counts_t *counts)
{
    counts->link_recv = lwip_stats.link.recv;
    counts->link_drop = lwip_stats.link.drop;
    counts->ip_recv = lwip_stats.ip.recv;
    counts->ip_drop = lwip_stats.ip.drop;
    counts->udp_recv = lwip_stats.udp.recv;
    counts->udp_drop = lwip_stats.udp.drop;
    counts->udp_memerr = lwip_stats.udp.memerr;
}

/* `JOIN <channel> <ssid> <passphrase>`: join the run's WPA2 network HT40,
 * power save off, and wait for its DHCP address. */
static void command_join(char **argv, int argc)
{
    unsigned long channel;
    if (argc != 4 || !parse_unsigned(argv[1], 13, &channel) || channel == 0 ||
        strlen(argv[2]) > 32 || strlen(argv[3]) < 8 || strlen(argv[3]) > 63) {
        reply_error("JOIN", "invalid");
        return;
    }
    if (s_started) {
        reply_error("JOIN", "joined-before");
        return;
    }
    wifi_config_t config = { 0 };
    memcpy(config.sta.ssid, argv[2], strlen(argv[2]));
    memcpy(config.sta.password, argv[3], strlen(argv[3]));
    config.sta.channel = (uint8_t)channel;
    config.sta.scan_method = WIFI_FAST_SCAN;
    config.sta.threshold.authmode = WIFI_AUTH_WPA2_PSK;
    esp_err_t error = esp_wifi_set_config(WIFI_IF_STA, &config);
    memset(&config, 0, sizeof(config));
    if (error != ESP_OK) {
        reply_esp_error("JOIN", "set_config", error);
        return;
    }
    if ((error = esp_wifi_start()) != ESP_OK) {
        reply_esp_error("JOIN", "start", error);
        return;
    }
    s_started = true;
    /* HT only, as the open station: the access point runs HE off. */
    if ((error = esp_wifi_set_protocol(WIFI_IF_STA, WIFI_PROTOCOL_11B | WIFI_PROTOCOL_11G |
                                                        WIFI_PROTOCOL_11N)) != ESP_OK) {
        reply_esp_error("JOIN", "set_protocol", error);
        return;
    }
    if ((error = esp_wifi_set_bandwidth(WIFI_IF_STA, WIFI_BW40)) != ESP_OK) {
        reply_esp_error("JOIN", "set_bandwidth", error);
        return;
    }
    if ((error = esp_wifi_set_ps(WIFI_PS_NONE)) != ESP_OK) {
        reply_esp_error("JOIN", "set_ps", error);
        return;
    }
    if ((error = esp_wifi_enable_rx_statistics(true, false)) != ESP_OK) {
        reply_esp_error("JOIN", "enable_rx_statistics", error);
        return;
    }
    s_joining = true;
    if ((error = esp_wifi_connect()) != ESP_OK) {
        s_joining = false;
        reply_esp_error("JOIN", "connect", error);
        return;
    }
    EventBits_t bits = xEventGroupWaitBits(s_events, BIT_GOT_IP, pdTRUE, pdTRUE,
                                           pdMS_TO_TICKS(JOIN_TIMEOUT_MS));
    s_joining = false;
    if (!(bits & BIT_GOT_IP)) {
        printf("@ERR JOIN timeout disconnects=%" PRIu32 " reason=%" PRId32 "\n", s_disconnects,
               s_last_reason);
        fflush(stdout);
        return;
    }
    s_joined = true;
    wifi_ap_record_t ap = { 0 };
    wifi_phy_mode_t phymode = 0;
    esp_netif_ip_info_t ip = { 0 };
    esp_wifi_sta_get_ap_info(&ap);
    esp_wifi_sta_get_negotiated_phymode(&phymode);
    esp_netif_get_ip_info(s_netif, &ip);
    printf("@JOINED channel=%u second=%d rssi=%d phymode=%d ip=" IPSTR " disconnects=%" PRIu32
           "\n",
           ap.primary, (int)ap.second, ap.rssi, (int)phymode, IP2STR(&ip.ip), s_disconnects);
    reply_ok("JOIN");
}

/* `RX <run> <port> <delay_ms> <window_ms> <drain_ms>`: count run `run`'s
 * datagrams on `port` until the drain ends, those within the window apart,
 * and echo every probe. Answers at once. */
static void command_rx(char **argv, int argc)
{
    unsigned long run, port, delay, window, drain;
    if (argc != 6 || !parse_unsigned(argv[1], UINT32_MAX, &run) ||
        !parse_unsigned(argv[2], 65535, &port) || port == 0 ||
        !parse_unsigned(argv[3], MAX_PHASE_MS, &delay) ||
        !parse_unsigned(argv[4], MAX_PHASE_MS, &window) ||
        !parse_unsigned(argv[5], MAX_PHASE_MS, &drain)) {
        reply_error("RX", "invalid");
        return;
    }
    if (!s_joined) {
        reply_error("RX", "no-link");
        return;
    }
    if (s_running) {
        reply_error("RX", "running");
        return;
    }
    if (s_socket >= 0 && s_port != port) {
        close(s_socket);
        s_socket = -1;
    }
    if (s_socket < 0) {
        int socket_fd = socket(AF_INET, SOCK_DGRAM, IPPROTO_UDP);
        if (socket_fd < 0) {
            reply_error("RX", "socket");
            return;
        }
        struct sockaddr_in local = {
            .sin_family = AF_INET,
            .sin_port = htons((uint16_t)port),
            .sin_addr.s_addr = htonl(INADDR_ANY),
        };
        struct timeval timeout = { .tv_sec = 0, .tv_usec = 50000 };
        if (bind(socket_fd, (struct sockaddr *)&local, sizeof(local)) != 0 ||
            setsockopt(socket_fd, SOL_SOCKET, SO_RCVTIMEO, &timeout, sizeof(timeout)) != 0) {
            close(socket_fd);
            reply_error("RX", "bind");
            return;
        }
        s_socket = socket_fd;
        s_port = (uint16_t)port;
    }
    /* Whatever an earlier run left queued on the socket is not this run's. */
    static uint8_t stale[DATAGRAM_CAPACITY];
    while (recv(s_socket, stale, sizeof(stale), MSG_DONTWAIT) >= 0) {
    }
    memset(s_seen, 0, sizeof(s_seen));
    memset(&s_counts, 0, sizeof(s_counts));
    s_counts.run = (uint32_t)run;
    s_counts.first_us = -1;
    s_counts.last_us = -1;
    esp_test_clr_hw_statistics();
    esp_wifi_clr_rx_statistics(8);
    lwip_counts(&s_lwip_start);
    xSemaphoreTake(s_done, 0);
    s_start_us = esp_timer_get_time();
    s_window_start_us = s_start_us + (int64_t)delay * 1000;
    s_window_end_us = s_window_start_us + (int64_t)window * 1000;
    s_end_us = s_window_end_us + (int64_t)drain * 1000;
    s_running = true;
    if (xTaskCreate(receiver, "udp-rx", RECEIVER_STACK, NULL, RECEIVER_PRIORITY, NULL) != pdPASS) {
        s_running = false;
        reply_error("RX", "task");
        return;
    }
    reply_ok("RX");
}

static void print_rx_statistics(uint8_t tid)
{
    esp_test_rx_statistics_t stats;
    memset(&stats, 0, sizeof(stats));
    esp_err_t error = esp_wifi_get_rx_statistics(tid, &stats);
    if (error != ESP_OK) {
        printf("@WIFIRX tid=%u error=0x%x\n", tid, (unsigned)error);
        return;
    }
    printf("@WIFIRX tid=%u legacy=%" PRIu32 " legacy_noeb=%" PRIu32 " ht=%" PRIu32
           " ht_noeb=%" PRIu32 " ht_stbc=%" PRIu32 " ht_retry=%" PRIu32 " su=%" PRIu32
           " su_retry=%" PRIu32 " rx_isr=%" PRIu32 " rx_nblks=%" PRIu32
           " rx_reset_rxbase_cnt=%" PRIu32 " rx_base_null_cnt=%" PRIu32 "\n",
           tid, stats.legacy, stats.legacy_noeb, stats.ht, stats.ht_noeb, stats.ht_stbc,
           stats.ht_retry, stats.su, stats.su_retry, stats.rx_isr, stats.rx_nblks,
           stats.rx_reset_rxbase_cnt, stats.rx_base_null_cnt);
}

static void print_hw_statistics(void)
{
    esp_test_hw_rx_statistics_t hw;
    memset(&hw, 0, sizeof(hw));
    esp_err_t error = esp_test_get_hw_rx_statistics(&hw);
    if (error != ESP_OK) {
        printf("@HWRX error=0x%x\n", (unsigned)error);
        return;
    }
    printf("@HWRX rx_fcs_err=%u rx_abort=%u rx_abort_fcs_pass=%u nrx_err_pwrdrop=%u "
           "rx_samebm_errcnt=%u rx_mpdu=%u rx_end_cnt=%u rx_datasuc=%u rx_cfo_hz=%d rx_sf=%u "
           "rx_other_ucast=%u rx_buf_fullcnt=%u rx_fifo_ovfcnt=%u rx_tkip_errcnt=%u "
           "rx_btblock_err=%u rx_freqhop_err=%u rx_lastunmatch_err=%u rx_ack_int_cnt=%u "
           "rx_rts_int_cnt=%u brx_err_agc=%u brx_err=%u nrx_err=%u nrx_err_abort=%u "
           "nrx_err_agcexit=%u nrx_err_bboff=%u nrx_err_fdm_wdg=%u nrx_err_restart=%u "
           "nrx_err_serv=%u nrx_err_txover=%u nrx_err_unsupport=%u nrx_htsig_err=%u "
           "rxhung_statis=%u txhung_statis=%u rxtxhung=%" PRIu32 " rxtxpanic=%" PRIu32 "\n",
           hw.rx_fcs_err, hw.rx_abort, hw.rx_abort_fcs_pass, hw.nrx_err_pwrdrop,
           hw.rx_samebm_errcnt, hw.rx_mpdu, hw.rx_end_cnt, hw.rx_datasuc, hw.rx_cfo_hz, hw.rx_sf,
           hw.rx_other_ucast, hw.rx_buf_fullcnt, hw.rx_fifo_ovfcnt, hw.rx_tkip_errcnt,
           hw.rx_btblock_err, hw.rx_freqhop_err, hw.rx_lastunmatch_err, hw.rx_ack_int_cnt,
           hw.rx_rts_int_cnt, hw.brx_err_agc, hw.brx_err, hw.nrx_err, hw.nrx_err_abort,
           hw.nrx_err_agcexit, hw.nrx_err_bboff, hw.nrx_err_fdm_wdg, hw.nrx_err_restart,
           hw.nrx_err_serv, hw.nrx_err_txover, hw.nrx_err_unsupport, hw.nrx_htsig_err,
           hw.rxhung_statis, hw.txhung_statis, hw.rxtxhung, hw.rxtxpanic);
}

/* `REPORT <run>`: wait for run `run` to end and print its counts, the
 * vendor's receive statistics since its start and the link. */
static void command_report(char **argv, int argc)
{
    unsigned long run;
    if (argc != 2 || !parse_unsigned(argv[1], UINT32_MAX, &run)) {
        reply_error("REPORT", "invalid");
        return;
    }
    if (run != s_counts.run) {
        reply_error("REPORT", "unknown-run");
        return;
    }
    if (s_running) {
        int64_t remaining_ms = (s_end_us - esp_timer_get_time()) / 1000 + 1000;
        if (xSemaphoreTake(s_done, pdMS_TO_TICKS(remaining_ms > 0 ? remaining_ms : 1000)) !=
            pdTRUE) {
            reply_error("REPORT", "running");
            return;
        }
        /* The run stays reportable. */
        xSemaphoreGive(s_done);
    }
    const counts_t *counts = &s_counts;
    uint64_t lost = 0;
    if (counts->any) {
        uint64_t below = counts->highest + 1 < TALLY_CAPACITY ? counts->highest + 1
                                                              : TALLY_CAPACITY;
        lost = below > counts->datagrams ? below - counts->datagrams : 0;
    }
    printf("@RX run=%" PRIu32 " datagrams=%" PRIu64 " bytes=%" PRIu64
           " window_datagrams=%" PRIu64 " window_bytes=%" PRIu64 " lost=%" PRIu64
           " reordered=%" PRIu64 " duplicates=%" PRIu64 " untracked=%" PRIu64 " highest=%" PRId64
           " foreign=%" PRIu64 " echoed=%" PRIu64 " echo_refused=%" PRIu64
           " receive_errors=%" PRIu64 " first_us=%" PRId64 " last_us=%" PRId64
           " window_us=%" PRId64 "\n",
           counts->run, counts->datagrams, counts->bytes, counts->window_datagrams,
           counts->window_bytes, lost, counts->reordered, counts->duplicates, counts->untracked,
           counts->any ? (int64_t)counts->highest : (int64_t)-1, counts->foreign, counts->echoed,
           counts->echo_refused, counts->receive_errors, counts->first_us, counts->last_us,
           s_window_end_us - s_window_start_us);
    lwip_counts_t now;
    lwip_counts(&now);
    printf("@LWIP link_recv=%" PRIu32 " link_drop=%" PRIu32 " ip_recv=%" PRIu32
           " ip_drop=%" PRIu32 " udp_recv=%" PRIu32 " udp_drop=%" PRIu32 " udp_memerr=%" PRIu32
           "\n",
           now.link_recv - s_lwip_start.link_recv, now.link_drop - s_lwip_start.link_drop,
           now.ip_recv - s_lwip_start.ip_recv, now.ip_drop - s_lwip_start.ip_drop,
           now.udp_recv - s_lwip_start.udp_recv, now.udp_drop - s_lwip_start.udp_drop,
           now.udp_memerr - s_lwip_start.udp_memerr);
    print_rx_statistics(0);
    print_hw_statistics();
    wifi_ap_record_t ap = { 0 };
    if (esp_wifi_sta_get_ap_info(&ap) == ESP_OK) {
        printf("@LINK channel=%u second=%d rssi=%d disconnects=%" PRIu32 "\n", ap.primary,
               (int)ap.second, ap.rssi, s_disconnects);
    } else {
        printf("@LINK down disconnects=%" PRIu32 "\n", s_disconnects);
    }
    reply_ok("REPORT");
}

/* `DUMP`: the vendor's own statistics dump, between `@DUMP begin` and
 * `@DUMP end`; INFO logging only around it. */
static void command_dump(void)
{
    printf("@DUMP begin\n");
    fflush(stdout);
    esp_log_level_set("*", ESP_LOG_INFO);
    esp_err_t error = esp_wifi_statis_dump(WIFI_STATIS_ALL);
    vTaskDelay(pdMS_TO_TICKS(200));
    esp_log_level_set("*", ESP_LOG_WARN);
    fflush(stdout);
    printf("@DUMP end error=0x%x\n", (unsigned)error);
    reply_ok("DUMP");
}

static void dispatch(char *line)
{
    char *argv[7];
    int argc = 0;
    for (char *token = strtok(line, " "); token != NULL && argc < 7; token = strtok(NULL, " ")) {
        argv[argc++] = token;
    }
    if (argc == 0) {
        return;
    }
    if (strcmp(argv[0], "JOIN") == 0) {
        command_join(argv, argc);
    } else if (strcmp(argv[0], "RX") == 0) {
        command_rx(argv, argc);
    } else if (strcmp(argv[0], "REPORT") == 0) {
        command_report(argv, argc);
    } else if (strcmp(argv[0], "DUMP") == 0 && argc == 1) {
        command_dump();
    } else if (strcmp(argv[0], "SYNC") == 0 && argc == 1) {
        print_ready();
        reply_ok("SYNC");
    } else {
        reply_error(argv[0], "unknown");
    }
}

void app_main(void)
{
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
    s_events = xEventGroupCreate();
    s_done = xSemaphoreCreateBinary();
    ESP_ERROR_CHECK(esp_netif_init());
    ESP_ERROR_CHECK(esp_event_loop_create_default());
    s_netif = esp_netif_create_default_wifi_sta();
    wifi_init_config_t init = WIFI_INIT_CONFIG_DEFAULT();
    ESP_ERROR_CHECK(esp_wifi_init(&init));
    ESP_ERROR_CHECK(esp_wifi_set_storage(WIFI_STORAGE_RAM));
    ESP_ERROR_CHECK(esp_wifi_set_mode(WIFI_MODE_STA));
    /* Channels 1 to 13. */
    ESP_ERROR_CHECK(esp_wifi_set_country_code("DE", false));
    ESP_ERROR_CHECK(esp_event_handler_register(WIFI_EVENT, ESP_EVENT_ANY_ID, on_event, NULL));
    ESP_ERROR_CHECK(esp_event_handler_register(IP_EVENT, IP_EVENT_STA_GOT_IP, on_event, NULL));
    ESP_LOGW(TAG, "ready; the radio starts on JOIN");
    print_ready();
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
            /* A JOIN line holds the network's passphrase. */
            memset(line, 0, sizeof(line));
            length = 0;
            continue;
        }
        if (length + 1 < sizeof(line)) {
            line[length++] = (char)character;
        } else {
            memset(line, 0, sizeof(line));
            length = 0;
            reply_error("LINE", "overflow");
        }
    }
}
