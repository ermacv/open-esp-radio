/* Record failed driver assertions instead of aborting the host process, so a
 * scenario can report the violation as part of its trace. */
#ifdef assert
#undef assert
#endif
void oer_host_assert_failed(const char *expression, const char *file, int line);
#define assert(e) ((e) ? (void)0 : oer_host_assert_failed(#e, __FILE__, __LINE__))
