/* One host critical section stands for the driver spinlock. */
#pragma once
typedef struct { int owner; } portMUX_TYPE;
#define portMUX_INITIALIZER_UNLOCKED { 0 }
void oer_host_enter_critical(void);
void oer_host_exit_critical(void);
#define portENTER_CRITICAL(mux) oer_host_enter_critical()
#define portEXIT_CRITICAL(mux) oer_host_exit_critical()
#define portENTER_CRITICAL_ISR(mux) oer_host_enter_critical()
#define portEXIT_CRITICAL_ISR(mux) oer_host_exit_critical()
#define portENTER_CRITICAL_SAFE(mux) oer_host_enter_critical()
#define portEXIT_CRITICAL_SAFE(mux) oer_host_exit_critical()
