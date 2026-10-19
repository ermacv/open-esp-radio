/* Logging has no behavioural effect in the compared driver paths. */
#pragma once
#define ESP_LOGE(tag, format, ...) do { } while (0)
#define ESP_LOGW(tag, format, ...) do { } while (0)
#define ESP_LOGI(tag, format, ...) do { } while (0)
#define ESP_LOGD(tag, format, ...) do { } while (0)
#define ESP_LOGV(tag, format, ...) do { } while (0)
#define ESP_EARLY_LOGE(tag, format, ...) do { } while (0)
#define ESP_EARLY_LOGW(tag, format, ...) do { } while (0)
