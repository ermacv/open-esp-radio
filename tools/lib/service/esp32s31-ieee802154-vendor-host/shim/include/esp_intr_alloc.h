/* Interrupt allocation: the stand captures the handler to deliver scenario
 * interrupts. */
#pragma once
#include "esp_err.h"
typedef void (*intr_handler_t)(void *arg);
typedef struct oer_host_intr_handle *intr_handle_t;
esp_err_t esp_intr_alloc(int source, int flags, intr_handler_t handler, void *arg,
                         intr_handle_t *ret_handle);
esp_err_t esp_intr_free(intr_handle_t handle);
