cmake_minimum_required(VERSION 3.22)

# The socket variant's configuration, unchanged: the variants differ only in
# how the load is consumed.
set(SDKCONFIG_DEFAULTS "${CMAKE_CURRENT_LIST_DIR}/../esp32s31-wifi-udp-rx/sdkconfig.defaults")
include($ENV{IDF_PATH}/tools/cmake/project.cmake)
idf_build_set_property(MINIMAL_BUILD ON)
# A fixed version keeps the repository commit out of the image, so its
# digest depends only on the sources; the board journal records the commit.
set(PROJECT_VER "1")
project(oer_wifi_udp_rx_fast_peer)
