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
 *
 * Two consumers count the load. The `socket` variant reads a BSD socket in
 * a receiver task, which copies each datagram out of the IP stack. The
 * `fast` variant (OER_UDP_RX_FAST) counts in lwIP's own thread through the
 * raw UDP API and frees each received buffer there, so the driver gets its
 * buffers back without the socket queue, the task switch and the copy.
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
#include "freertos/queue.h"
#include "freertos/task.h"
#include "lwip/sockets.h"
#include "lwip/stats.h"
#include "nvs_flash.h"
#include "sdkconfig.h"

#if defined(OER_UDP_RX_FAST)
#include "lwip/pbuf.h"
#include "lwip/priv/tcpip_priv.h"
#include "lwip/udp.h"
#define VARIANT "fast"
#else
#define VARIANT "socket"
#endif

#define PROTOCOL_VERSION 2
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
#define TASK_PRIORITY 10
#define TASK_STACK 4096
/* How long after its end a run's counting may take to confirm it stopped. */
#define STOP_CONFIRM_MS 2000

static const char *TAG = "udp-rx";

#define BIT_GOT_IP BIT0
static EventGroupHandle_t s_events;
static esp_netif_t *s_netif;
static volatile bool s_joining;
static volatile bool s_joined;
static volatile uint32_t s_disconnects;
static volatile int32_t s_last_reason;
static bool s_started;

/* One run's count. Written only by its counter (the socket receiver task, or
 * lwIP's thread in the fast variant) while the run counts; read by the
 * console only after the run's completion named that run. */
typedef struct {
    uint64_t datagrams, bytes, window_datagrams, window_bytes;
    uint64_t reordered, duplicates, untracked, foreign;
    uint64_t echoed, echo_refused, receive_errors;
    bool any;
    uint64_t highest;
    int64_t first_us, last_us;
} counts_t;

typedef struct {
    STAT_COUNTER link_recv, link_drop, ip_recv, ip_drop, udp_recv, udp_drop, udp_memerr;
} lwip_counts_t;

/* The vendor's counters at one moment of a run. */
typedef struct {
    int64_t at_us;
    lwip_counts_t lwip;
    esp_test_rx_statistics_t rx;
    esp_err_t rx_error;
} snapshot_t;

enum { SNAP_RUN_START, SNAP_WINDOW_START, SNAP_WINDOW_END, SNAP_RUN_END, SNAPS };

/* What the run task reports when a run ended. */
typedef enum {
    /* The counter confirmed that it stopped: the run's state is final. */
    END_CONFIRMED,
    /* The counter did not confirm its stop: it may still write the state. */
    END_UNCONFIRMED,
} end_t;

typedef struct {
    uint32_t run;
    end_t end;
} completion_t;

/* The console's view of the last run; only the console task touches it. */
typedef enum {
    RUN_NONE,
    /* Started; its completion has not been taken yet. */
    RUN_ACTIVE,
    /* Its completion named it and confirmed the stop. */
    RUN_ENDED,
    /* Its counter did not confirm its stop: no run starts again. */
    RUN_FAULTED,
} run_state_t;

static run_state_t s_state = RUN_NONE;
static uint32_t s_run;

static uint64_t s_seen[TALLY_WORDS];
static counts_t s_counts;
/* Set by the console before a run's tasks start; read-only while it runs. */
static int64_t s_start_us, s_window_start_us, s_window_end_us, s_end_us;
/* Written by the run task, read by the console after the completion. */
static snapshot_t s_snaps[SNAPS];
static esp_test_hw_rx_statistics_t s_hw;
static esp_err_t s_hw_error;
/* The run task's completions, each naming its run. */
static QueueHandle_t s_completions;

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
    printf("@READY protocol=%d target=%s app=wifi-udp-rx variant=%s\n", PROTOCOL_VERSION,
           CONFIG_IDF_TARGET, VARIANT);
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

static void lwip_counts(lwip_counts_t *counts)
{
    /* Word reads of lwIP's own counters, which its thread writes. */
    counts->link_recv = lwip_stats.link.recv;
    counts->link_drop = lwip_stats.link.drop;
    counts->ip_recv = lwip_stats.ip.recv;
    counts->ip_drop = lwip_stats.ip.drop;
    counts->udp_recv = lwip_stats.udp.recv;
    counts->udp_drop = lwip_stats.udp.drop;
    counts->udp_memerr = lwip_stats.udp.memerr;
}

static void snapshot(snapshot_t *snap)
{
    snap->at_us = esp_timer_get_time();
    lwip_counts(&snap->lwip);
    memset(&snap->rx, 0, sizeof(snap->rx));
    snap->rx_error = esp_wifi_get_rx_statistics(0, &snap->rx);
}

static void wait_until(int64_t at_us)
{
    for (;;) {
        int64_t remaining = at_us - esp_timer_get_time();
        if (remaining <= 0) {
            return;
        }
        TickType_t ticks = pdMS_TO_TICKS(remaining / 1000);
        vTaskDelay(ticks > 0 ? ticks : 1);
    }
}

#if defined(OER_UDP_RX_FAST)

/* lwIP's thread alone touches these. */
static struct udp_pcb *s_pcb;
static uint16_t s_pcb_port;
static bool s_counting;
static uint32_t s_counting_run;

/* Runs in lwIP's thread: count one datagram and free its buffer at once. */
static void on_datagram(void *argument, struct udp_pcb *pcb, struct pbuf *datagram,
                        const ip_addr_t *from, u16_t port)
{
    (void)argument;
    int64_t now = esp_timer_get_time();
    if (!s_counting || now >= s_end_us) {
        pbuf_free(datagram);
        return;
    }
    uint8_t header[HEADER_LEN];
    uint16_t length = datagram->tot_len;
    uint8_t flags = 0;
    uint32_t run = 0;
    uint64_t sequence = 0;
    if (pbuf_copy_partial(datagram, header, HEADER_LEN, 0) != HEADER_LEN ||
        !read_header(header, length, &flags, &run, &sequence)) {
        s_counts.foreign++;
    } else if (flags & FLAG_ECHO_REQUEST) {
        /* A probe of any run goes back as its echo (the image's echo). */
        struct pbuf *echo = pbuf_alloc(PBUF_TRANSPORT, length, PBUF_RAM);
        if (echo != NULL && pbuf_copy_partial(datagram, echo->payload, length, 0) == length) {
            ((uint8_t *)echo->payload)[4] = FLAG_ECHO_REPLY;
            if (udp_sendto(pcb, echo, from, port) == ERR_OK) {
                s_counts.echoed++;
            } else {
                s_counts.echo_refused++;
            }
        } else {
            s_counts.echo_refused++;
        }
        if (echo != NULL) {
            pbuf_free(echo);
        }
    } else if (run == s_counting_run && !(flags & FLAG_ECHO_REPLY)) {
        count(sequence, length, now);
    } else {
        s_counts.foreign++;
    }
    pbuf_free(datagram);
}

typedef struct {
    struct tcpip_api_call_data base;
    bool attach;
    uint32_t run;
    uint16_t port;
} counting_call_t;

/* Runs in lwIP's thread: start counting `run` on `port`, or stop counting it. */
static err_t counting_call(struct tcpip_api_call_data *data)
{
    counting_call_t *call = (counting_call_t *)data;
    if (!call->attach) {
        if (!s_counting || s_counting_run != call->run) {
            return ERR_VAL;
        }
        s_counting = false;
        return ERR_OK;
    }
    if (s_pcb != NULL && s_pcb_port != call->port) {
        udp_remove(s_pcb);
        s_pcb = NULL;
    }
    if (s_pcb == NULL) {
        struct udp_pcb *pcb = udp_new();
        if (pcb == NULL) {
            return ERR_MEM;
        }
        err_t error = udp_bind(pcb, IP_ANY_TYPE, call->port);
        if (error != ERR_OK) {
            udp_remove(pcb);
            return error;
        }
        udp_recv(pcb, on_datagram, NULL);
        s_pcb = pcb;
        s_pcb_port = call->port;
    }
    s_counting_run = call->run;
    s_counting = true;
    return ERR_OK;
}

static bool start_counting(uint32_t run, uint16_t port)
{
    counting_call_t call = { .attach = true, .run = run, .port = port };
    return tcpip_api_call(counting_call, &call.base) == ERR_OK;
}

/* Called by the run task once the run ended; true when lwIP's thread
 * confirmed that it stopped counting `run`. */
static bool stop_counting(uint32_t run)
{
    counting_call_t call = { .attach = false, .run = run };
    return tcpip_api_call(counting_call, &call.base) == ERR_OK;
}

#else

static int s_socket = -1;
static uint16_t s_port;
/* The receiver task's ends, each naming its run. */
static QueueHandle_t s_receiver_ends;

static void receiver(void *argument)
{
    uint32_t run = (uint32_t)(uintptr_t)argument;
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
        uint32_t datagram_run = 0;
        uint64_t sequence = 0;
        if (!read_header(buffer, (size_t)length, &flags, &datagram_run, &sequence)) {
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
        } else if (datagram_run == run && !(flags & FLAG_ECHO_REPLY) && now < s_end_us) {
            count(sequence, (size_t)length, now);
        } else {
            s_counts.foreign++;
        }
    }
    xQueueSend(s_receiver_ends, &run, portMAX_DELAY);
    vTaskDelete(NULL);
}

static bool open_socket(uint16_t port)
{
    if (s_socket >= 0 && s_port != port) {
        close(s_socket);
        s_socket = -1;
    }
    if (s_socket >= 0) {
        return true;
    }
    int socket_fd = socket(AF_INET, SOCK_DGRAM, IPPROTO_UDP);
    if (socket_fd < 0) {
        return false;
    }
    struct sockaddr_in local = {
        .sin_family = AF_INET,
        .sin_port = htons(port),
        .sin_addr.s_addr = htonl(INADDR_ANY),
    };
    struct timeval timeout = { .tv_sec = 0, .tv_usec = 50000 };
    if (bind(socket_fd, (struct sockaddr *)&local, sizeof(local)) != 0 ||
        setsockopt(socket_fd, SOL_SOCKET, SO_RCVTIMEO, &timeout, sizeof(timeout)) != 0) {
        close(socket_fd);
        return false;
    }
    s_socket = socket_fd;
    s_port = port;
    return true;
}

static bool start_counting(uint32_t run, uint16_t port)
{
    if (!open_socket(port)) {
        return false;
    }
    /* Whatever an earlier run left queued on the socket is not this run's. */
    static uint8_t stale[DATAGRAM_CAPACITY];
    while (recv(s_socket, stale, sizeof(stale), MSG_DONTWAIT) >= 0) {
    }
    return xTaskCreate(receiver, "udp-rx", TASK_STACK, (void *)(uintptr_t)run, TASK_PRIORITY,
                       NULL) == pdPASS;
}

/* Called by the run task once the run ended; true when the receiver task
 * of `run` said it ended. */
static bool stop_counting(uint32_t run)
{
    uint32_t ended = 0;
    return xQueueReceive(s_receiver_ends, &ended, pdMS_TO_TICKS(STOP_CONFIRM_MS)) == pdTRUE &&
           ended == run;
}

#endif

/* One run's phases: the counters at the window's start and end and at the
 * run's end, then its completion. The hardware counters are cleared at the
 * window's start and read at its end. */
static void run_task(void *argument)
{
    uint32_t run = (uint32_t)(uintptr_t)argument;
    wait_until(s_window_start_us);
    snapshot(&s_snaps[SNAP_WINDOW_START]);
    esp_test_clr_hw_statistics();
    wait_until(s_window_end_us);
    snapshot(&s_snaps[SNAP_WINDOW_END]);
    memset(&s_hw, 0, sizeof(s_hw));
    s_hw_error = esp_test_get_hw_rx_statistics(&s_hw);
    wait_until(s_end_us);
    completion_t completion = {
        .run = run,
        .end = stop_counting(run) ? END_CONFIRMED : END_UNCONFIRMED,
    };
    snapshot(&s_snaps[SNAP_RUN_END]);
    xQueueSend(s_completions, &completion, portMAX_DELAY);
    vTaskDelete(NULL);
}

/* Take the active run's completion within `ticks`: true once the run ended
 * and its state is final. A completion naming another run, or a stop its
 * counter did not confirm, faults the receiver for good. */
static bool take_completion(TickType_t ticks)
{
    completion_t completion;
    if (xQueueReceive(s_completions, &completion, ticks) != pdTRUE) {
        return false;
    }
    if (completion.run != s_run || completion.end != END_CONFIRMED) {
        s_state = RUN_FAULTED;
        return false;
    }
    s_state = RUN_ENDED;
    return true;
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
 * and echo every probe. Answers once counting started. */
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
    if (s_state == RUN_ACTIVE) {
        take_completion(0);
    }
    if (s_state == RUN_ACTIVE) {
        reply_error("RX", "running");
        return;
    }
    if (s_state == RUN_FAULTED) {
        reply_error("RX", "counter-unconfirmed");
        return;
    }
    /* No counter runs: the last run's completion confirmed its stop. */
    memset(s_seen, 0, sizeof(s_seen));
    memset(&s_counts, 0, sizeof(s_counts));
    memset(s_snaps, 0, sizeof(s_snaps));
    s_counts.first_us = -1;
    s_counts.last_us = -1;
    s_run = (uint32_t)run;
    esp_wifi_clr_rx_statistics(8);
    esp_test_clr_hw_statistics();
    snapshot(&s_snaps[SNAP_RUN_START]);
    s_start_us = s_snaps[SNAP_RUN_START].at_us;
    s_window_start_us = s_start_us + (int64_t)delay * 1000;
    s_window_end_us = s_window_start_us + (int64_t)window * 1000;
    s_end_us = s_window_end_us + (int64_t)drain * 1000;
    if (!start_counting(s_run, (uint16_t)port)) {
        s_state = RUN_NONE;
        reply_error("RX", "start");
        return;
    }
    if (xTaskCreate(run_task, "udp-rx-run", TASK_STACK, (void *)(uintptr_t)s_run,
                    TASK_PRIORITY + 1, NULL) != pdPASS) {
        /* The counter runs to the run's end with nobody to confirm it. */
        s_state = RUN_FAULTED;
        reply_error("RX", "task");
        return;
    }
    s_state = RUN_ACTIVE;
    reply_ok("RX");
}

static void print_lwip(const char *span, const lwip_counts_t *from, const lwip_counts_t *to)
{
    printf("@LWIP run=%" PRIu32 " span=%s link_recv=%u link_drop=%u ip_recv=%u ip_drop=%u "
           "udp_recv=%u udp_drop=%u udp_memerr=%u\n",
           s_run, span, (unsigned)(STAT_COUNTER)(to->link_recv - from->link_recv),
           (unsigned)(STAT_COUNTER)(to->link_drop - from->link_drop),
           (unsigned)(STAT_COUNTER)(to->ip_recv - from->ip_recv),
           (unsigned)(STAT_COUNTER)(to->ip_drop - from->ip_drop),
           (unsigned)(STAT_COUNTER)(to->udp_recv - from->udp_recv),
           (unsigned)(STAT_COUNTER)(to->udp_drop - from->udp_drop),
           (unsigned)(STAT_COUNTER)(to->udp_memerr - from->udp_memerr));
}

static void print_wifi_rx(const char *span, const snapshot_t *from, const snapshot_t *to)
{
    if (from->rx_error != ESP_OK || to->rx_error != ESP_OK) {
        printf("@WIFIRX run=%" PRIu32 " span=%s error=0x%x\n", s_run, span,
               (unsigned)(from->rx_error != ESP_OK ? from->rx_error : to->rx_error));
        return;
    }
    const esp_test_rx_statistics_t *a = &from->rx, *b = &to->rx;
    printf("@WIFIRX run=%" PRIu32 " span=%s tid=0 legacy=%" PRIu32 " legacy_noeb=%" PRIu32
           " ht=%" PRIu32 " ht_noeb=%" PRIu32 " ht_stbc=%" PRIu32 " ht_retry=%" PRIu32
           " su=%" PRIu32 " su_retry=%" PRIu32 " rx_isr=%" PRIu32 " rx_nblks=%" PRIu32
           " rx_reset_rxbase_cnt=%" PRIu32 " rx_base_null_cnt=%" PRIu32 "\n",
           s_run, span, b->legacy - a->legacy, b->legacy_noeb - a->legacy_noeb, b->ht - a->ht,
           b->ht_noeb - a->ht_noeb, b->ht_stbc - a->ht_stbc, b->ht_retry - a->ht_retry,
           b->su - a->su, b->su_retry - a->su_retry, b->rx_isr - a->rx_isr,
           b->rx_nblks - a->rx_nblks, b->rx_reset_rxbase_cnt - a->rx_reset_rxbase_cnt,
           b->rx_base_null_cnt - a->rx_base_null_cnt);
}

static void print_hw_statistics(void)
{
    const esp_test_hw_rx_statistics_t *hw = &s_hw;
    if (s_hw_error != ESP_OK) {
        printf("@HWRX run=%" PRIu32 " span=window error=0x%x\n", s_run, (unsigned)s_hw_error);
        return;
    }
    printf("@HWRX run=%" PRIu32 " span=window rx_fcs_err=%u rx_abort=%u rx_abort_fcs_pass=%u "
           "nrx_err_pwrdrop=%u rx_samebm_errcnt=%u rx_mpdu=%u rx_end_cnt=%u rx_datasuc=%u "
           "rx_cfo_hz=%d rx_sf=%u rx_other_ucast=%u rx_buf_fullcnt=%u rx_fifo_ovfcnt=%u "
           "rx_tkip_errcnt=%u rx_btblock_err=%u rx_freqhop_err=%u rx_lastunmatch_err=%u "
           "rx_ack_int_cnt=%u rx_rts_int_cnt=%u brx_err_agc=%u brx_err=%u nrx_err=%u "
           "nrx_err_abort=%u nrx_err_agcexit=%u nrx_err_bboff=%u nrx_err_fdm_wdg=%u "
           "nrx_err_restart=%u nrx_err_serv=%u nrx_err_txover=%u nrx_err_unsupport=%u "
           "nrx_htsig_err=%u rxhung_statis=%u txhung_statis=%u rxtxhung=%" PRIu32
           " rxtxpanic=%" PRIu32 "\n",
           s_run, hw->rx_fcs_err, hw->rx_abort, hw->rx_abort_fcs_pass, hw->nrx_err_pwrdrop,
           hw->rx_samebm_errcnt, hw->rx_mpdu, hw->rx_end_cnt, hw->rx_datasuc, hw->rx_cfo_hz,
           hw->rx_sf, hw->rx_other_ucast, hw->rx_buf_fullcnt, hw->rx_fifo_ovfcnt,
           hw->rx_tkip_errcnt, hw->rx_btblock_err, hw->rx_freqhop_err, hw->rx_lastunmatch_err,
           hw->rx_ack_int_cnt, hw->rx_rts_int_cnt, hw->brx_err_agc, hw->brx_err, hw->nrx_err,
           hw->nrx_err_abort, hw->nrx_err_agcexit, hw->nrx_err_bboff, hw->nrx_err_fdm_wdg,
           hw->nrx_err_restart, hw->nrx_err_serv, hw->nrx_err_txover, hw->nrx_err_unsupport,
           hw->nrx_htsig_err, hw->rxhung_statis, hw->txhung_statis, hw->rxtxhung,
           hw->rxtxpanic);
}

/* `REPORT <run>`: once run `run`'s completion confirmed its end, print its
 * counts and the vendor's counters over its window and over the run. */
static void command_report(char **argv, int argc)
{
    unsigned long run;
    if (argc != 2 || !parse_unsigned(argv[1], UINT32_MAX, &run)) {
        reply_error("REPORT", "invalid");
        return;
    }
    if (s_state == RUN_NONE || run != s_run) {
        reply_error("REPORT", "unknown-run");
        return;
    }
    if (s_state == RUN_ACTIVE) {
        int64_t remaining_ms =
            (s_end_us - esp_timer_get_time()) / 1000 + STOP_CONFIRM_MS + 1000;
        take_completion(pdMS_TO_TICKS(remaining_ms > 0 ? remaining_ms : 1));
    }
    if (s_state == RUN_ACTIVE) {
        reply_error("REPORT", "running");
        return;
    }
    if (s_state == RUN_FAULTED) {
        reply_error("REPORT", "counter-unconfirmed");
        return;
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
           s_run, counts->datagrams, counts->bytes, counts->window_datagrams,
           counts->window_bytes, lost, counts->reordered, counts->duplicates, counts->untracked,
           counts->any ? (int64_t)counts->highest : (int64_t)-1, counts->foreign, counts->echoed,
           counts->echo_refused, counts->receive_errors, counts->first_us, counts->last_us,
           s_window_end_us - s_window_start_us);
    printf("@SPAN run=%" PRIu32 " window_from_us=%" PRId64 " window_to_us=%" PRId64
           " run_to_us=%" PRId64 "\n",
           s_run, s_snaps[SNAP_WINDOW_START].at_us - s_start_us,
           s_snaps[SNAP_WINDOW_END].at_us - s_start_us,
           s_snaps[SNAP_RUN_END].at_us - s_start_us);
    print_lwip("window", &s_snaps[SNAP_WINDOW_START].lwip, &s_snaps[SNAP_WINDOW_END].lwip);
    print_lwip("run", &s_snaps[SNAP_RUN_START].lwip, &s_snaps[SNAP_RUN_END].lwip);
    print_wifi_rx("window", &s_snaps[SNAP_WINDOW_START], &s_snaps[SNAP_WINDOW_END]);
    print_wifi_rx("run", &s_snaps[SNAP_RUN_START], &s_snaps[SNAP_RUN_END]);
    print_hw_statistics();
    wifi_ap_record_t ap = { 0 };
    if (esp_wifi_sta_get_ap_info(&ap) == ESP_OK) {
        printf("@LINK run=%" PRIu32 " channel=%u second=%d rssi=%d disconnects=%" PRIu32 "\n",
               s_run, ap.primary, (int)ap.second, ap.rssi, s_disconnects);
    } else {
        printf("@LINK run=%" PRIu32 " down disconnects=%" PRIu32 "\n", s_run, s_disconnects);
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
    s_completions = xQueueCreate(1, sizeof(completion_t));
    ESP_ERROR_CHECK(s_completions != NULL ? ESP_OK : ESP_ERR_NO_MEM);
#if !defined(OER_UDP_RX_FAST)
    s_receiver_ends = xQueueCreate(1, sizeof(uint32_t));
    ESP_ERROR_CHECK(s_receiver_ends != NULL ? ESP_OK : ESP_ERR_NO_MEM);
#endif
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
