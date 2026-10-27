/* Run against the actual shim/src/host.c with an isolated recorder. */
#include <stdint.h>
#include <string.h>
#include "esp_intr_alloc.h"

static unsigned calls;
static int invalid_record;

uint64_t oer_host_record(const char *name, const uint64_t *arguments,
                         uint32_t count, uint32_t pointers)
{
    const char *expected_name = calls == 0 ? "esp_intr_disable" : "esp_intr_enable";
    uint64_t expected_handle = calls == 0 ? 37 : 83;
    if (calls >= 2 || strcmp(name, expected_name) || count != 1 ||
        !arguments || arguments[0] != expected_handle || pointers != 0) {
        invalid_record = 1;
    }
    ++calls;
    return calls == 1 ? 258 : (uint64_t)(int64_t)-513;
}

int main(void)
{
    esp_err_t disabled = esp_intr_disable((intr_handle_t)(uintptr_t)37);
    esp_err_t enabled = esp_intr_enable((intr_handle_t)(uintptr_t)83);
    return disabled != 258 || enabled != -513 || calls != 2 || invalid_record;
}
