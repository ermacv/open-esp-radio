/* Modem clock ownership calls, recorded by the host stand. */
#pragma once
#include "soc/periph_defs.h"
void modem_clock_module_enable(shared_periph_module_t module);
void modem_clock_module_disable(shared_periph_module_t module);
void modem_clock_module_mac_reset(shared_periph_module_t module);
