# Examples

Applications on the application entry `oer`: each names the capabilities it
uses as features of `oer` and no chip. The image pipeline selects the
board's chip, builds the image for it and links it with the chip's staged
boot (ROM → ESP-IDF bootloader → flash bootstrap → stage two in PSRAM on the
ESP32-S31).

No example is built today. `wifi-station` stands on the entry's Wi-Fi
station, which is outside the build with the old Wi-Fi path; its sources
stay ([repository layout](../docs/repository-layout.md)).

| Example | What it does |
| --- | --- |
| [`wifi-station`](wifi-station/README.md) (not built) | Joins its build profile's network, takes an address by DHCP and exchanges a line over TCP with the gateway |
