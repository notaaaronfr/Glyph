/* ---- Glyph OS ABI: shared by the kernel, the shared C library and every app ----
 * Apps run in user mode (EL0) and reach the kernel only through system calls:
 * `svc #0` with the call number in x8, arguments in x0-x2, and results in x0 (and
 * x1 for TN_SYS_HEAP). */

#include <stdarg.h>
#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

enum {
    TN_SYS_EXIT,        /* (code): ends the app */
    TN_SYS_OUT,         /* (ptr, len): print; needs io */
    TN_SYS_ERR,         /* (ptr, len): report an error; always allowed */
    TN_SYS_GETC,        /* (): wait for a typed character; needs io */
    TN_SYS_TICKS,       /* (): the clock's counter; needs time */
    TN_SYS_TICK_FREQ,   /* (): counter ticks per second; needs time */
    TN_SYS_SLEEP,       /* (ms): sleep; needs time */
    TN_SYS_RANDOM_SEED, /* (): a seed for random numbers; needs rand */
    TN_SYS_HEAP,        /* (): the app's heap, as start (x0) and end (x1) */
};

/* The shared C library (libc.c): stateless, so the kernel and every app use one copy. */
void *memcpy(void *dst, const void *src, size_t n);
void *memmove(void *dst, const void *src, size_t n);
void *memset(void *dst, int c, size_t n);
int memcmp(const void *a, const void *b, size_t n);
size_t strlen(const char *s);
char *strchr(const char *s, int c);
int vsnprintf(char *buf, size_t size, const char *fmt, va_list ap);
int snprintf(char *buf, size_t size, const char *fmt, ...);

/* ---- end of ABI ---- */
