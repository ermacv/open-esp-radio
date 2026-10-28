# Bluetooth LE Link Layer

`oer-bluetooth-le-ll` owns allocation-free LE protocol state: connection and
channel progression, duplicate filtering, control, Data Length Update and
encryption procedures. AES-CCM transforms and zeroizing key material live here.
Air-format values and codecs are in `oer-bluetooth-le-wire`; DTM backend window
geometry is in `oer-bluetooth-le-port`.

HCI is the world boundary. Among conflicting connections the one closest to
its supervision timeout wins. An internal backend inconsistency is a radio
fault that poisons the port. Host backpressure withholds acknowledgement
instead of dropping the link.
