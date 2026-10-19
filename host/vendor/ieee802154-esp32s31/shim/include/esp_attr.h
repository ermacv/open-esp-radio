/* Placement attributes have no host meaning; inlining is preserved. */
#pragma once
#define IRAM_ATTR
#define DRAM_ATTR
#define RTC_DATA_ATTR
#define NOINLINE_ATTR __attribute__((noinline))
#define FORCE_INLINE_ATTR static inline __attribute__((always_inline))
#define TCM_IRAM_ATTR
#ifndef likely
#define likely(x) __builtin_expect(!!(x), 1)
#endif
#ifndef unlikely
#define unlikely(x) __builtin_expect(!!(x), 0)
#endif
