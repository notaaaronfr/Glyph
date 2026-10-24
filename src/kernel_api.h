/* ---- Glyph kernel API: what the kernel (kernel.c) provides to each app ----
 * Every app is compiled as its own C file: this header, then the runtime, then the
 * app's code. The runtime's state is `static`, so apps share nothing but the kernel. */

#define TN_KERNEL 1

#include <stdarg.h>
#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

void *memcpy(void *dst, const void *src, size_t n);
void *memmove(void *dst, const void *src, size_t n);
void *memset(void *dst, int c, size_t n);
int memcmp(const void *a, const void *b, size_t n);
size_t strlen(const char *s);
char *strchr(const char *s, int c);

/* Each app has its own heap. */
void *malloc(size_t n);
void free(void *p);
void *realloc(void *p, size_t n);

int vsnprintf(char *buf, size_t size, const char *fmt, va_list ap);
int snprintf(char *buf, size_t size, const char *fmt, ...);

/* Serial port. Each call's output comes out whole, even if apps switch meanwhile. */
void tn_out(const char *s, size_t n);
#define tn_err tn_out
void tn_flush(void);
/* Waits (without using the CPU) for a typed character. */
int tn_uart_getc(void);

const char *tn_getenv(const char *name);

/* The CPU's generic timer. */
uint64_t tn_ticks(void);
uint64_t tn_tick_freq(void);
/* Puts the app to sleep; other apps run meanwhile. */
void tn_sleep_ms(int64_t ms);

/* Ends the app (not the machine) with an exit code. */
__attribute__((noreturn)) void tn_exit(int code);

/* ---- end of kernel API ---- */
