/* The real SoC header, with direct register access recorded by the stand.
 * Only `esp_ieee802154_util.c` (the ETM channel helpers) uses these macros;
 * every MAC register access goes through the recorded LL instead. */
#include_next "soc/soc.h"

#ifndef OER_HOST_SOC_REGISTERS
#define OER_HOST_SOC_REGISTERS
#include <stdint.h>
uint32_t oer_host_register_read(uint32_t address);
void oer_host_register_write(uint32_t address, uint32_t value);
#undef REG_READ
#undef REG_WRITE
#define REG_READ(address) oer_host_register_read((uint32_t)(address))
#define REG_WRITE(address, value) oer_host_register_write((uint32_t)(address), (uint32_t)(value))
#endif
