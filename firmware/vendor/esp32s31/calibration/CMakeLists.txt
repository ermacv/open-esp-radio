# Vendor calibration firmware for the hardware calibration cross-check.
#
# Built by `cargo hil firmware build vendor-calibration` against the pinned ESP-IDF
# tree whose PHY, coexistence and Wi-Fi library submodules are replaced by
# the pinned archives of verification/esp32s31/artifacts.toml.
cmake_minimum_required(VERSION 3.22)
include($ENV{IDF_PATH}/tools/cmake/project.cmake)
idf_build_set_property(MINIMAL_BUILD ON)
# A fixed version keeps the repository commit out of the image, so its
# digest depends only on the sources; the board journal records the commit.
set(PROJECT_VER "1")
project(oer_vendor_calibration)
