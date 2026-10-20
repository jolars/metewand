#define R_NO_REMAP
#include <R.h>
#include <Rinternals.h>
#include <R_ext/Connections.h>

/* R's connection header uses C keywords that are reserved in C++. */
size_t mw_connection_read(SEXP source, unsigned char* byte) {
    return R_ReadConnection(R_GetConnection(source), byte, 1);
}
