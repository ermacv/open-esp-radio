# ESP32-S31 access-point transactions

This crate composes portable AP policy with S31 key slots and frame publication.
It is independent of the executor and network stack.

| Module | Responsibility |
| --- | --- |
| `engine` | A bounded protocol epoch, peers, security and power-save decisions |
| `hardware` | Borrowed key, receive-policy and TSF authority required by that epoch |
| `transaction` | One finite protocol/TX transaction, retaining its owner until completion |
| `tx`, `ampdu`, `rx` | S31 frame preparation, aggregate admission and receive processing |
| `security` | Key installation and pairwise receive ownership |
| `profile` | Implemented local capability claims, which the portable beacon (`oer-ieee80211-ap::beacon`) and association responses advertise |

The transaction owner composes these parts; the hardware contract does not
acquire another radio. The runtime supplies timers, DMA/IRQ progress and
network handoffs. Publication is not successful transmission: resources are
released only through the completion or terminal recovery path.

A-MPDU retry decisions use MAC radio samples from the publication's generation.
Before servicing an aggregate's ordinary BlockAckReq, the runtime obtains a fresh
sample and asks the aggregate owner to validate its generation. It passes the same
stamp to the answer processing. Missing or stale timing leaves the completion,
ordinary descriptor, retained frames and exchange accounting untouched.

Probe discovery accepts broadcast or AP-addressed requests with a wildcard or
matching SSID. The response uses the current beacon advertisement, excludes TIM,
and shares the beacon clock and management sequence space. It is published through
the management TX owner and retained until completion. Discovery does not allocate
a peer, alter its security state, start WPA2, or count as an association response.
Malformed requests and requests for another AP or SSID are ignored.
