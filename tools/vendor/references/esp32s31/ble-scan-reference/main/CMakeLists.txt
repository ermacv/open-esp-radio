idf_component_register(
    SRCS "reference.c"
    PRIV_REQUIRES bt nvs_flash esp_driver_usb_serial_jtag vfs
)
# The scanner's item-end callback frees the completed item through this
# memory-manager call from another object, so a linker wrap observes every
# completed scheduler item before the Controller recycles it.
target_link_libraries(${COMPONENT_LIB} INTERFACE "-Wl,--wrap=r_sym_memMgmt_232y4Maqyh8DfDo3KxfV")
