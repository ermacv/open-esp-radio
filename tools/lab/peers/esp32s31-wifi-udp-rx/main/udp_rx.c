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
#include "esp_log.h"
#include "esp_mac.h"
#include "esp_netif.h"
#include "esp_private/esp_wifi_he_private.h"
#include "esp_timer.h"
#include "esp_wifi.h"
#include "esp_wifi_he.h"
#include "freertos/FreeRTOS.h"
#include "freertos/queue.h"
#include "freertos/task.h"
#include "link.h"
#include "lwip/priv/tcpip_priv.h"
#include "lwip/sockets.h"
#include "lwip/stats.h"
#include "nvs_flash.h"
#include "sdkconfig.h"

#if defined(OER_UDP_RX_FAST)
#include "lwip/pbuf.h"
#include "lwip/udp.h"
#define VARIANT "fast"
#else
#define VARIANT "socket"
#endif

#define PROTOCOL_VERSION 3
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
/* The run task samples lwIP's 16-bit counters this often and adds their
 * differences up in 64 bits: far fewer than 2^16 datagrams arrive between
 * two samples. */
#define LWIP_SAMPLE_MS 100
#define LINK_EVENTS 32

static const char *TAG = "udp-rx";

static esp_netif_t *s_netif;
static bool s_started;
/* The link's state: only the console task touches it. */
static link_t s_link;
/* Link events from the Wi-Fi and IP handlers to the console task, in the
 * event loop's order; a handler that cannot post one raises the overflow,
 * which the console takes as a lost event. */
static QueueHandle_t s_link_events;
static atomic_bool s_link_overflow;

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

/* lwIP's counters as its thread holds them (16 bits each). */
typedef struct {
    STAT_COUNTER link_recv, link_drop, ip_recv, ip_drop, udp_recv, udp_drop, udp_memerr;
} lwip_counts_t;

/* lwIP's counters added up over a run, in 64 bits. */
typedef struct {
    uint64_t link_recv, link_drop, ip_recv, ip_drop, udp_recv, udp_drop, udp_memerr;
} lwip_totals_t;

/* The vendor's counters at one moment of a run. */
typedef struct {
    int64_t at_us;
    lwip_totals_t lwip;
    /* Every lwIP sample up to this moment was taken through lwIP's thread. */
    bool lwip_ok;
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
/* The run's lwIP sampling: set by the console before the run task starts,
 * then the run task's alone. */
static lwip_counts_t s_lwip_last;
static lwip_totals_t s_lwip_totals;
static bool s_lwip_ok;

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

/* Runs in the event loop task: post the event to the link's owner. */
static void post_link_event(link_event_kind_t kind, int32_t reason)
{
    link_event_t event = { .kind = kind, .reason = reason };
    if (xQueueSend(s_link_events, &event, 0) != pdTRUE) {
        atomic_store(&s_link_overflow, true);
    }
}

static void on_event(void *argument, esp_event_base_t base, int32_t id, void *data)
{
    (void)argument;
    if (base == WIFI_EVENT && id == WIFI_EVENT_STA_CONNECTED) {
        post_link_event(LINK_EVENT_CONNECTED, 0);
    } else if (base == WIFI_EVENT && id == WIFI_EVENT_STA_DISCONNECTED) {
        const wifi_event_sta_disconnected_t *event = data;
        post_link_event(LINK_EVENT_DISCONNECTED, event != NULL ? (int32_t)event->reason : -1);
    } else if (base == IP_EVENT && id == IP_EVENT_STA_GOT_IP) {
        post_link_event(LINK_EVENT_GOT_IP, 0);
    } else if (base == IP_EVENT && id == IP_EVENT_STA_LOST_IP) {
        post_link_event(LINK_EVENT_LOST_IP, 0);
    }
}

static const char *link_state_name(link_state_t state)
{
    switch (state) {
    case LINK_IDLE:
        return "idle";
    case LINK_JOINING:
        return "joining";
    case LINK_UP:
        return "up";
    case LINK_DOWN:
        return "down";
    case LINK_FAULTED:
        return "faulted";
    }
    return "unknown";
}

/* The console's side of the link events (link.h). */
static bool next_link_event(void *context, link_event_t *event)
{
    (void)context;
    if (xQueueReceive(s_link_events, event, 0) == pdTRUE) {
        return true;
    }
    if (atomic_exchange(&s_link_overflow, false)) {
        event->kind = LINK_EVENT_OVERFLOW;
        event->reason = 0;
        return true;
    }
    return false;
}

/* The last reconnect the driver refused, for the JOIN refusal. */
static esp_err_t s_connect_error = ESP_OK;

static bool act_on_link(void *context, link_action_t action)
{
    (void)context;
    if (action == LINK_ACT_RECONNECT) {
        s_connect_error = esp_wifi_connect();
        return s_connect_error == ESP_OK;
    }
    if (action == LINK_ACT_REPORT_LOST) {
        printf("@LOST state=%s reason=%" PRId32 " disconnects=%" PRIu32 "\n",
               link_state_name(s_link.state), s_link.last_reason, s_link.disconnects);
        fflush(stdout);
    }
    return true;
}

/* Apply every pending link event. */
static void drain_link(void)
{
    link_drain(&s_link, next_link_event, act_on_link, NULL);
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

typedef struct {
    struct tcpip_api_call_data base;
    lwip_counts_t counts;
} lwip_read_t;

/* Runs in lwIP's thread, the owner of its counters. */
static err_t read_lwip(struct tcpip_api_call_data *data)
{
    lwip_counts_t *counts = &((lwip_read_t *)data)->counts;
    counts->link_recv = lwip_stats.link.recv;
    counts->link_drop = lwip_stats.link.drop;
    counts->ip_recv = lwip_stats.ip.recv;
    counts->ip_drop = lwip_stats.ip.drop;
    counts->udp_recv = lwip_stats.udp.recv;
    counts->udp_drop = lwip_stats.udp.drop;
    counts->udp_memerr = lwip_stats.udp.memerr;
    return ERR_OK;
}

/* lwIP's counters, read in lwIP's thread. */
static bool lwip_sample(lwip_counts_t *counts)
{
    lwip_read_t read = { 0 };
    if (tcpip_api_call(read_lwip, &read.base) != ERR_OK) {
        return false;
    }
    *counts = read.counts;
    return true;
}

/* Add what lwIP counted since the last sample to the run's totals. */
static void lwip_advance(void)
{
    lwip_counts_t now;
    if (!lwip_sample(&now)) {
        s_lwip_ok = false;
        return;
    }
    lwip_counts_t *last = &s_lwip_last;
    lwip_totals_t *totals = &s_lwip_totals;
    totals->link_recv += (STAT_COUNTER)(now.link_recv - last->link_recv);
    totals->link_drop += (STAT_COUNTER)(now.link_drop - last->link_drop);
    totals->ip_recv += (STAT_COUNTER)(now.ip_recv - last->ip_recv);
    totals->ip_drop += (STAT_COUNTER)(now.ip_drop - last->ip_drop);
    totals->udp_recv += (STAT_COUNTER)(now.udp_recv - last->udp_recv);
    totals->udp_drop += (STAT_COUNTER)(now.udp_drop - last->udp_drop);
    totals->udp_memerr += (STAT_COUNTER)(now.udp_memerr - last->udp_memerr);
    *last = now;
}

/* The run's counters now; the run task takes it, after a final lwIP
 * sample. */
static void snapshot(snapshot_t *snap)
{
    lwip_advance();
    snap->at_us = esp_timer_get_time();
    snap->lwip = s_lwip_totals;
    snap->lwip_ok = s_lwip_ok;
    memset(&snap->rx, 0, sizeof(snap->rx));
    snap->rx_error = esp_wifi_get_rx_statistics(0, &snap->rx);
}

/* Wait for `at_us`, sampling lwIP's counters on the way. */
static void wait_until(int64_t at_us)
{
    for (;;) {
        int64_t remaining = at_us - esp_timer_get_time();
        if (remaining <= 0) {
            return;
        }
        int64_t step_ms = remaining / 1000 < LWIP_SAMPLE_MS ? remaining / 1000 : LWIP_SAMPLE_MS;
        TickType_t ticks = pdMS_TO_TICKS(step_ms);
        vTaskDelay(ticks > 0 ? ticks : 1);
        lwip_advance();
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
/* The joined link's facts, read while confirming the join. */
typedef struct {
    wifi_ap_record_t ap;
    wifi_phy_mode_t phymode;
    esp_netif_ip_info_t ip;
    const char *failed;
    esp_err_t error;
} join_facts_t;

static bool read_join_facts(void *context)
{
    join_facts_t *facts = context;
    facts->failed = NULL;
    if ((facts->error = esp_wifi_sta_get_ap_info(&facts->ap)) != ESP_OK) {
        facts->failed = "get_ap_info";
    } else if ((facts->error = esp_wifi_sta_get_negotiated_phymode(&facts->phymode)) != ESP_OK) {
        facts->failed = "get_negotiated_phymode";
    } else if ((facts->error = esp_netif_get_ip_info(s_netif, &facts->ip)) != ESP_OK) {
        facts->failed = "get_ip_info";
    } else if (facts->ip.ip.addr == 0) {
        facts->failed = "get_ip_info";
        facts->error = ESP_ERR_INVALID_STATE;
    }
    return facts->failed == NULL;
}

/* The JOIN refusal for a failed vendor call: the join is over. */
static void join_failed(const char *call, esp_err_t error)
{
    link_fail(&s_link);
    reply_esp_error("JOIN", call, error);
}

/* `JOIN <channel> <ssid> <passphrase>`: join the run's WPA2 network HT40,
 * power save off, and wait for the link to be up: associated, addressed and
 * read back, with no later event taking either. */
static void command_join(char **argv, int argc)
{
    unsigned long channel;
    if (argc != 4 || !parse_unsigned(argv[1], 13, &channel) || channel == 0 ||
        strlen(argv[2]) > 32 || strlen(argv[3]) < 8 || strlen(argv[3]) > 63) {
        reply_error("JOIN", "invalid");
        return;
    }
    drain_link();
    if (s_started || !link_begin_join(&s_link)) {
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
        join_failed("set_config", error);
        return;
    }
    if ((error = esp_wifi_start()) != ESP_OK) {
        join_failed("start", error);
        return;
    }
    s_started = true;
    /* HT only, as the open station: the access point runs HE off. */
    if ((error = esp_wifi_set_protocol(WIFI_IF_STA, WIFI_PROTOCOL_11B | WIFI_PROTOCOL_11G |
                                                        WIFI_PROTOCOL_11N)) != ESP_OK) {
        join_failed("set_protocol", error);
        return;
    }
    if ((error = esp_wifi_set_bandwidth(WIFI_IF_STA, WIFI_BW40)) != ESP_OK) {
        join_failed("set_bandwidth", error);
        return;
    }
    if ((error = esp_wifi_set_ps(WIFI_PS_NONE)) != ESP_OK) {
        join_failed("set_ps", error);
        return;
    }
    if ((error = esp_wifi_enable_rx_statistics(true, false)) != ESP_OK) {
        join_failed("enable_rx_statistics", error);
        return;
    }
    if ((error = esp_wifi_connect()) != ESP_OK) {
        join_failed("connect", error);
        return;
    }
    int64_t deadline = esp_timer_get_time() + (int64_t)JOIN_TIMEOUT_MS * 1000;
    join_facts_t facts = { 0 };
    for (;;) {
        int64_t remaining_ms = (deadline - esp_timer_get_time()) / 1000;
        if (remaining_ms <= 0) {
            link_fail(&s_link);
            printf("@ERR JOIN timeout disconnects=%" PRIu32 " reason=%" PRId32 "\n",
                   s_link.disconnects, s_link.last_reason);
            fflush(stdout);
            return;
        }
        link_event_t event;
        if (xQueueReceive(s_link_events, &event, pdMS_TO_TICKS(remaining_ms > 100 ? 100
                                                                                  : remaining_ms)) ==
            pdTRUE) {
            link_action_t action = link_apply(&s_link, &event);
            if (action != LINK_ACT_NONE && !act_on_link(NULL, action)) {
                join_failed("connect", s_connect_error);
                return;
            }
        }
        if (atomic_load(&s_link_overflow) || link_candidate(&s_link)) {
            link_confirm_t confirmed =
                link_confirm(&s_link, next_link_event, act_on_link, read_join_facts, &facts);
            if (confirmed == LINK_CONFIRM_UP) {
                break;
            }
            if (confirmed == LINK_CONFIRM_FAULTED) {
                reply_error("JOIN", "events-lost");
                return;
            }
            if (confirmed == LINK_CONFIRM_FAILED) {
                if (facts.failed != NULL) {
                    reply_esp_error("JOIN", facts.failed, facts.error);
                } else {
                    reply_esp_error("JOIN", "connect", s_connect_error);
                }
                return;
            }
        }
    }
    printf("@JOINED channel=%u second=%d rssi=%d phymode=%d ip=" IPSTR " disconnects=%" PRIu32
           "\n",
           facts.ap.primary, (int)facts.ap.second, facts.ap.rssi, (int)facts.phymode,
           IP2STR(&facts.ip.ip), s_link.disconnects);
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
    drain_link();
    if (s_link.state != LINK_UP) {
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
    memset(&s_lwip_totals, 0, sizeof(s_lwip_totals));
    if (!lwip_sample(&s_lwip_last)) {
        reply_error("RX", "lwip");
        return;
    }
    s_lwip_ok = true;
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

static void print_lwip(const char *span, const snapshot_t *from, const snapshot_t *to)
{
    if (!from->lwip_ok || !to->lwip_ok) {
        printf("@LWIP run=%" PRIu32 " span=%s error=sample\n", s_run, span);
        return;
    }
    const lwip_totals_t *a = &from->lwip, *b = &to->lwip;
    printf("@LWIP run=%" PRIu32 " span=%s link_recv=%" PRIu64 " link_drop=%" PRIu64
           " ip_recv=%" PRIu64 " ip_drop=%" PRIu64 " udp_recv=%" PRIu64 " udp_drop=%" PRIu64
           " udp_memerr=%" PRIu64 "\n",
           s_run, span, b->link_recv - a->link_recv, b->link_drop - a->link_drop,
           b->ip_recv - a->ip_recv, b->ip_drop - a->ip_drop, b->udp_recv - a->udp_recv,
           b->udp_drop - a->udp_drop, b->udp_memerr - a->udp_memerr);
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
    print_lwip("window", &s_snaps[SNAP_WINDOW_START], &s_snaps[SNAP_WINDOW_END]);
    print_lwip("run", &s_snaps[SNAP_RUN_START], &s_snaps[SNAP_RUN_END]);
    print_wifi_rx("window", &s_snaps[SNAP_WINDOW_START], &s_snaps[SNAP_WINDOW_END]);
    print_wifi_rx("run", &s_snaps[SNAP_RUN_START], &s_snaps[SNAP_RUN_END]);
    print_hw_statistics();
    drain_link();
    wifi_ap_record_t ap = { 0 };
    esp_err_t ap_error = s_link.state == LINK_UP ? esp_wifi_sta_get_ap_info(&ap) : ESP_OK;
    if (s_link.state != LINK_UP) {
        printf("@LINK run=%" PRIu32 " state=%s disconnects=%" PRIu32 "\n", s_run,
               link_state_name(s_link.state), s_link.disconnects);
    } else if (ap_error != ESP_OK) {
        printf("@LINK run=%" PRIu32 " state=up error=0x%x\n", s_run, (unsigned)ap_error);
    } else {
        printf("@LINK run=%" PRIu32 " state=up channel=%u second=%d rssi=%d disconnects=%" PRIu32
               "\n",
               s_run, ap.primary, (int)ap.second, ap.rssi, s_link.disconnects);
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
    link_init(&s_link);
    s_link_events = xQueueCreate(LINK_EVENTS, sizeof(link_event_t));
    ESP_ERROR_CHECK(s_link_events != NULL ? ESP_OK : ESP_ERR_NO_MEM);
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
    ESP_ERROR_CHECK(esp_event_handler_register(IP_EVENT, IP_EVENT_STA_LOST_IP, on_event, NULL));
    ESP_LOGW(TAG, "ready; the radio starts on JOIN");
    print_ready();
    fflush(stdout);

    static char line[LINE_CAPACITY];
    size_t length = 0;
    for (;;) {
        drain_link();
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
