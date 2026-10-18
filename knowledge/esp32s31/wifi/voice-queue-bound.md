# Station frames in the Voice queue: the predecessor's bound

A fact of the predecessor driver, not a rule of this one: W3 decides the
station-frame queueing anew.

- The station's own frames (EAPOL, Null, Deauthentication) had no queue of
  their own; they went in the Voice access category beside data of user
  priority 6–7.
- **Bound:** once a held Voice queue was released, a station frame waited for
  at most **(software queue depth + 1)** data attempts. The depth was a
  build-time composition parameter per access category (the backend's
  software queue depth), initially 2, so at most 3 attempts.
- The bound held because the service kept at most that many data attempts per
  access category at the port; further data waited, encoded, in the data
  machine. Station frames were submitted at once. Transmit tickets alone would
  not have held it. Beside an access point, the access point's data counted
  against the same limit.
- Later the per-access-category queue storage was derived from the ticket
  count, and the data admission bound (the depth, initially 2) stayed separate;
  the "depth + 1" bound was unchanged.
- Test shape: hold the Voice queue, fill it with priority 6–7 data to the
  limit, release it; EAPOL and Deauthentication go within the bound.

Source: predecessor issue discussion, 2026-10-13 and 2026-10-16.
