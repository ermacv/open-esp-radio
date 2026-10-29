# Vendor link environment of the ESP32-S31 vendor scenarios.
#
# Built by `cargo fw idf build phy-tracking-reference` against the
# pinned ESP-IDF tree whose PHY, coexistence and Wi-Fi library submodules are
# replaced by the pinned archives of chips/esp32s31/vendor/artifacts.toml.
cmake_minimum_required(VERSION 3.22)
include($ENV{IDF_PATH}/tools/cmake/project.cmake)
idf_build_set_property(MINIMAL_BUILD ON)
# A fixed version keeps the repository commit out of the image, so its
# digest depends only on the sources.
set(PROJECT_VER "1")
project(phy_tracking_reference)
