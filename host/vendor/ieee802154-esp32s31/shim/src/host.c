/* Host bindings for the calls the ESP-IDF IEEE 802.15.4 driver makes outside
 * its own sources. Closed PHY, BTBB and coexistence calls, modem clocks, the
 * interrupt allocator and the application callbacks are forwarded to the Rust
 * recorder; none of them models hardware beyond what the scenario supplies. */

#include <stdbool.h>
#include <stdint.h>
#include <string.h>

#include "esp_coex_i154.h"
#include "esp_ieee802154.h"
#include "esp_intr_alloc.h"
#include "esp_phy_init.h"
#include "esp_private/esp_modem_clock.h"
#include "esp_private/sleep_modem.h"
#include "esp_rom_sys.h"
#include "esp_timer.h"

/* Implemented by the Rust recorder. `name` is a static C string. */
uint64_t oer_host_record(const char *name, const uint64_t *arguments, uint32_t count,
                        uint32_t pointers);
void oer_host_event(const char *name, const uint64_t *arguments, uint32_t count,
                    const uint8_t *first, uint32_t first_len, const uint8_t *second,
                    uint32_t second_len);
int32_t oer_host_enh_ack(const uint8_t *frame, uint32_t frame_len, uint8_t *enhack_frame,
                         uint8_t *key);
void oer_host_interrupt_handler(intr_handler_t handler, void *arg);

#define RECORD0(name) (void)oer_host_record(name, 0, 0, 0)
#define RECORD1(name, a) do { uint64_t args_[1] = { (uint64_t)(a) }; (void)oer_host_record(name, args_, 1, 0); } while (0)

void esp_phy_enable(esp_phy_modem_t modem) { RECORD1("esp_phy_enable", modem); }
void esp_phy_disable(esp_phy_modem_t modem) { RECORD1("esp_phy_disable", modem); }
void esp_btbb_enable(void) { RECORD0("esp_btbb_enable"); }
void esp_btbb_disable(void) { RECORD0("esp_btbb_disable"); }
void esp_phy_modem_init(sleep_modem_t modem) { RECORD1("esp_phy_modem_init", modem); }
void esp_phy_modem_deinit(sleep_modem_t modem) { RECORD1("esp_phy_modem_deinit", modem); }

void modem_clock_module_enable(shared_periph_module_t module) { RECORD1("modem_clock_module_enable", module); }
void modem_clock_module_disable(shared_periph_module_t module) { RECORD1("modem_clock_module_disable", module); }
void modem_clock_module_mac_reset(shared_periph_module_t module) { RECORD1("modem_clock_module_mac_reset", module); }

void esp_coex_ieee802154_txrx_pti_set(ieee802154_coex_event_t event) { RECORD1("esp_coex_ieee802154_txrx_pti_set", event); }
void esp_coex_ieee802154_ack_pti_set(ieee802154_coex_event_t event) { RECORD1("esp_coex_ieee802154_ack_pti_set", event); }
void esp_coex_ieee802154_coex_break_notify(void) { RECORD0("esp_coex_ieee802154_coex_break_notify"); }
void esp_coex_ieee802154_extcoex_tx_stage(void) { RECORD0("esp_coex_ieee802154_extcoex_tx_stage"); }
void esp_coex_ieee802154_extcoex_rx_stage(void) { RECORD0("esp_coex_ieee802154_extcoex_rx_stage"); }
void esp_coex_ieee802154_status_enable(void) { RECORD0("esp_coex_ieee802154_status_enable"); }
void esp_coex_ieee802154_status_disable(void) { RECORD0("esp_coex_ieee802154_status_disable"); }
void esp_coex_ieee802154_force_rx_enable(bool enable) { RECORD1("esp_coex_ieee802154_force_rx_enable", enable); }

void ieee802154_txon_delay_set(void) { RECORD0("ieee802154_txon_delay_set"); }
uint32_t bt_bb_get_cur_rx_info(void) { return (uint32_t)oer_host_record("bt_bb_get_cur_rx_info", 0, 0, 0); }

int64_t esp_timer_get_time(void) { return (int64_t)oer_host_record("esp_timer_get_time", 0, 0, 0); }
void esp_rom_delay_us(uint32_t us) { RECORD1("esp_rom_delay_us", us); }

void oer_host_enter_critical(void) { RECORD0("enter_critical"); }
void oer_host_exit_critical(void) { RECORD0("exit_critical"); }

void oer_host_assert_failed(const char *expression, const char *file, int line)
{
    uint64_t args[1] = { (uint64_t)line };
    oer_host_event("assert_failed", args, 1, (const uint8_t *)expression,
                   (uint32_t)strlen(expression), (const uint8_t *)file, (uint32_t)strlen(file));
}

esp_err_t esp_intr_alloc(int source, int flags, intr_handler_t handler, void *arg,
                         intr_handle_t *ret_handle)
{
    uint64_t args[2] = { (uint64_t)source, (uint64_t)flags };
    (void)oer_host_record("esp_intr_alloc", args, 2, 0);
    oer_host_interrupt_handler(handler, arg);
    *ret_handle = (intr_handle_t)(uintptr_t)1;
    return ESP_OK;
}

esp_err_t esp_intr_free(intr_handle_t handle)
{
    (void)handle;
    RECORD0("esp_intr_free");
    oer_host_interrupt_handler(0, 0);
    return ESP_OK;
}

/* Received and transmitted frames are `[length, psdu...]`; the PHR length
 * counts the two trailing bytes, which the receiver replaces by RSSI/LQI. */
static uint32_t frame_bytes(const uint8_t *frame)
{
    return frame ? (uint32_t)frame[0] + 1 : 0;
}

static void frame_info_arguments(const esp_ieee802154_frame_info_t *info, uint64_t *args)
{
    args[0] = info ? info->pending : 0;
    args[1] = info ? info->process : 0;
    args[2] = info ? info->channel : 0;
    args[3] = info ? (uint64_t)(int64_t)info->rssi : 0;
    args[4] = info ? info->lqi : 0;
    args[5] = info ? (uint64_t)info->timestamp : 0;
    args[6] = info != 0;
    args[7] = info ? (uint64_t)info->mpf_index : 0;
}

void esp_ieee802154_receive_done(uint8_t *data, esp_ieee802154_frame_info_t *frame_info)
{
    uint64_t args[8];
    frame_info_arguments(frame_info, args);
    oer_host_event("receive_done", args, 8, data, frame_bytes(data), 0, 0);
}

void esp_ieee802154_receive_sfd_done(void) { oer_host_event("receive_sfd_done", 0, 0, 0, 0, 0, 0); }

void esp_ieee802154_receive_failed(uint16_t error)
{
    uint64_t args[1] = { error };
    oer_host_event("receive_failed", args, 1, 0, 0, 0, 0);
}

void esp_ieee802154_transmit_done(const uint8_t *frame, const uint8_t *ack,
                                  esp_ieee802154_frame_info_t *ack_frame_info)
{
    uint64_t args[8];
    frame_info_arguments(ack_frame_info, args);
    oer_host_event("transmit_done", args, 8, frame, frame_bytes(frame), ack, frame_bytes(ack));
}

void esp_ieee802154_transmit_failed(const uint8_t *frame, esp_ieee802154_tx_error_t error)
{
    uint64_t args[1] = { (uint64_t)error };
    oer_host_event("transmit_failed", args, 1, frame, frame_bytes(frame), 0, 0);
}

void esp_ieee802154_transmit_sfd_done(uint8_t *frame)
{
    oer_host_event("transmit_sfd_done", 0, 0, frame, frame_bytes(frame), 0, 0);
}

void esp_ieee802154_cca_done(bool channel_free)
{
    uint64_t args[1] = { channel_free };
    oer_host_event("cca_done", args, 1, 0, 0, 0, 0);
}

void esp_ieee802154_energy_detect_done(int8_t power)
{
    uint64_t args[1] = { (uint64_t)(int64_t)power };
    oer_host_event("energy_detect_done", args, 1, 0, 0, 0, 0);
}

void esp_ieee802154_ed_failed(uint16_t error)
{
    uint64_t args[1] = { error };
    oer_host_event("ed_failed", args, 1, 0, 0, 0, 0);
}

void esp_ieee802154_receive_at_done(void) { oer_host_event("receive_at_done", 0, 0, 0, 0, 0, 0); }

/* The application generator. A secured ACK is armed as ESP-IDF's OpenThread
 * port arms it from its generator (esp_openthread_radio.c
 * `enh_ack_set_security_addr_and_key`): read the extended address, then
 * configure transmit security over the ACK. */
esp_err_t esp_ieee802154_enh_ack_generator(uint8_t *frame, esp_ieee802154_frame_info_t *frame_info,
                                           uint8_t *enhack_frame)
{
    uint8_t key[16];
    uint8_t address[8];
    int32_t generated;

    (void)frame_info;
    generated = oer_host_enh_ack(frame, frame_bytes(frame), enhack_frame, key);
    if (generated < 0) {
        return ESP_FAIL;
    }
    if (generated == 1) {
        esp_ieee802154_get_extended_address(address);
        esp_ieee802154_set_transmit_security(enhack_frame, key, address);
    }
    return ESP_OK;
}
