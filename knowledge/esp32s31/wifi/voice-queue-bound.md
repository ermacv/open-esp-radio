# Station frames in the Voice queue: a bound of another driver

A fact of a Wi-Fi driver outside this repository, not a rule of this one.
Unverified: its source is a discussion without a measurement.

- The station's own frames (EAPOL, Null, Deauthentication) have no queue of
  their own; they go in the Voice access category beside data of user
  priority 6–7.
- **Bound:** once a held Voice queue is released, a station frame waits for
  at most **(software queue depth + 1)** data attempts. The depth is a
  build-time composition parameter per access category (the backend's
  software queue depth), 2 by default, so at most 3 attempts.
- The bound holds because the service keeps at most that many data attempts
  per access category at the port; further data waits, encoded, in the data
  machine. Station frames are submitted at once. Transmit tickets alone would
  not hold it. Beside an access point, the access point's data counts against
  the same limit.
- The per-access-category queue storage is derived from the ticket count, and
  the data admission bound (the depth) is separate; the "depth + 1" bound
  holds either way.
- Test shape: hold the Voice queue, fill it with priority 6–7 data to the
  limit, release it; EAPOL and Deauthentication go within the bound.

Source: driver design discussion, 2026-10-13 and 2026-10-16 (record outside
this repository).
