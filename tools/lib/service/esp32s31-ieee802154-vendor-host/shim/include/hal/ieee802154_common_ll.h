/* The real common LL with every `ieee802154_ll_*` accessor replaced by a
 * recording implementation. The generated rename table moves the vendor
 * inline bodies out of the way while their types, enums and constants stay
 * exactly as published; the generated recorder then declares the same
 * signatures under the original names. */
#ifndef OER_HOST_IEEE802154_COMMON_LL
#define OER_HOST_IEEE802154_COMMON_LL
#include "oer_ll_rename.h"
#include_next "hal/ieee802154_common_ll.h"
#include "oer_ll_restore.h"
#include "oer_ll_recorder.h"
#endif
