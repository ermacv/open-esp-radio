idf_component_register(SRCS "main.c" PRIV_REQUIRES esp_phy esp_hw_support esp_driver_usb_serial_jtag esp_wifi esp_event)

# Vendor data objects the firmware reports after calibration. Each size is
# the object's symbol size in the pinned archive that defines it, so the
# report covers exactly the object the relations name.
set(reported_objects "phy_param:libphy.a")

idf_component_get_property(phy_dir esp_phy COMPONENT_DIR)
set(definitions "")
set(entries "")
foreach(object ${reported_objects})
    string(REPLACE ":" ";" parts "${object}")
    list(GET parts 0 symbol)
    list(GET parts 1 archive)
    set(path "${phy_dir}/lib/${IDF_TARGET}/${archive}")
    execute_process(
        COMMAND ${CMAKE_NM} --defined-only --print-size "${path}"
        OUTPUT_VARIABLE listing
        COMMAND_ERROR_IS_FATAL ANY)
    string(REGEX MATCH "[0-9a-f]+ ([0-9a-f]+) [DdBb] ${symbol}\n" match "${listing}")
    if(NOT match)
        message(FATAL_ERROR "${archive} defines no sized data object ${symbol}")
    endif()
    math(EXPR size "0x${CMAKE_MATCH_1}" OUTPUT_FORMAT DECIMAL)
    string(APPEND definitions "extern uint8_t ${symbol}[];\n")
    string(APPEND entries "    {\"${symbol}\", ${symbol}, ${size}},\n")
endforeach()
configure_file(reported.h.in "${CMAKE_CURRENT_BINARY_DIR}/reported.h" @ONLY)
target_include_directories(${COMPONENT_LIB} PRIVATE "${CMAKE_CURRENT_BINARY_DIR}")
