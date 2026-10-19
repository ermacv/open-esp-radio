/* Host form of the ESP-IDF check macros; logging is dropped, control flow is
 * identical. */
#pragma once
#include "esp_err.h"

#define ESP_RETURN_ON_FALSE(a, err_code, log_tag, format, ...) \
    do { if (!(a)) { return (err_code); } } while (0)
#define ESP_RETURN_VOID_ON_FALSE(a, log_tag, format, ...) \
    do { if (!(a)) { return; } } while (0)
#define ESP_RETURN_ON_ERROR(x, log_tag, format, ...) \
    do { esp_err_t err_rc_ = (x); if (err_rc_ != ESP_OK) { return err_rc_; } } while (0)
#define ESP_GOTO_ON_FALSE(a, err_code, goto_tag, log_tag, format, ...) \
    do { if (!(a)) { ret = (err_code); goto goto_tag; } } while (0)
#define ESP_GOTO_ON_ERROR(x, goto_tag, log_tag, format, ...) \
    do { esp_err_t err_rc_ = (x); if (err_rc_ != ESP_OK) { ret = err_rc_; goto goto_tag; } } while (0)
#define ESP_RETURN_ON_FALSE_ISR(a, err_code, log_tag, format, ...) \
    do { if (!(a)) { return (err_code); } } while (0)
