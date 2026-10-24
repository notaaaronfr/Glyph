/* ---- Glyph kernel platform: bare metal on QEMU's AArch64 `virt` machine ----
 * Included before the runtime when building with `--target kernel`. There is no
 * operating system underneath: this file boots the CPU and provides what the runtime
 * needs from one: memory allocation, text formatting, the serial port (for `print` and
 * `input`), the clock, and a way to stop the machine.
 *
 * The MMU stays off in this first version, so all memory is identity-mapped and
 * uncached; the code is built with -mstrict-align because unaligned accesses fault
 * without the MMU. */

#define TN_KERNEL 1

#include <stdarg.h>
#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

/* ---- boot ---- */

void tn_kernel_main(void);
void tn_kernel_trap(uint64_t kind, uint64_t esr, uint64_t elr, uint64_t far);

__asm__(
    ".section .text.boot, \"ax\"\n"
    ".global _start\n"
    "_start:\n"
    /* Only CPU 0 runs Glyph; any others wait forever. */
    "    mrs x0, mpidr_el1\n"
    "    and x0, x0, #0xff\n"
    "    cbnz x0, tn_park\n"
    "    ldr x0, =__stack_top\n"
    "    mov sp, x0\n"
    /* Allow floating-point / SIMD instructions (the C compiler may use them). */
    "    mov x0, #(3 << 20)\n"
    "    msr cpacr_el1, x0\n"
    "    isb\n"
    "    adr x0, tn_vectors\n"
    "    msr vbar_el1, x0\n"
    "    isb\n"
    /* Zero the .bss section. */
    "    ldr x0, =__bss_start\n"
    "    ldr x1, =__bss_end\n"
    "1:  cmp x0, x1\n"
    "    b.hs 2f\n"
    "    str xzr, [x0], #8\n"
    "    b 1b\n"
    "2:  bl tn_kernel_main\n"
    "tn_park:\n"
    "    wfe\n"
    "    b tn_park\n"
    /* Exception vectors: 16 entries of 128 bytes. Each passes its index to the trap
     * handler on a fresh stack. */
    ".balign 2048\n"
    "tn_vectors:\n"
    ".irp i, 0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15\n"
    "    .balign 128\n"
    "    mov x0, #\\i\n"
    "    b tn_trap_entry\n"
    ".endr\n"
    "tn_trap_entry:\n"
    "    mrs x1, esr_el1\n"
    "    mrs x2, elr_el1\n"
    "    mrs x3, far_el1\n"
    "    ldr x4, =__stack_top\n"
    "    mov sp, x4\n"
    "    bl tn_kernel_trap\n"
    "    b tn_park\n"
    ".text\n");

/* ---- the small part of the C library the runtime uses ---- */

void *memcpy(void *dst, const void *src, size_t n) {
    unsigned char *d = dst;
    const unsigned char *s = src;
    while (n--) *d++ = *s++;
    return dst;
}

void *memmove(void *dst, const void *src, size_t n) {
    unsigned char *d = dst;
    const unsigned char *s = src;
    if (d < s) {
        while (n--) *d++ = *s++;
    } else {
        while (n--) d[n] = s[n];
    }
    return dst;
}

void *memset(void *dst, int c, size_t n) {
    unsigned char *d = dst;
    while (n--) *d++ = (unsigned char)c;
    return dst;
}

int memcmp(const void *a, const void *b, size_t n) {
    const unsigned char *x = a, *y = b;
    for (size_t i = 0; i < n; i++)
        if (x[i] != y[i]) return x[i] < y[i] ? -1 : 1;
    return 0;
}

size_t strlen(const char *s) {
    size_t n = 0;
    while (s[n]) n++;
    return n;
}

char *strchr(const char *s, int c) {
    for (;; s++) {
        if (*s == (char)c) return (char *)s;
        if (!*s) return NULL;
    }
}

/* ---- memory allocator ----
 * Blocks carry a 16-byte header with their usable size. Freed blocks go on a free
 * list per size class (multiples of 16 bytes up to 1 KB) or on one list for larger
 * blocks; new memory comes from the heap region after the stack. Reference counting
 * frees most blocks soon after use, so they are reused quickly. */

extern char __heap_start[], __heap_end[];

typedef struct tn_block { size_t size; struct tn_block *next; } tn_block;
#define TN_SMALL_CLASSES 64
static tn_block *tn_free_small[TN_SMALL_CLASSES + 1];
static tn_block *tn_free_large;
static char *tn_heap_top;

void tn_kernel_out_of_memory(void);

void *malloc(size_t n) {
    size_t size = (n + 15) & ~(size_t)15;
    if (size == 0) size = 16;
    tn_block *b = NULL;
    if (size <= TN_SMALL_CLASSES * 16) {
        tn_block **list = &tn_free_small[size / 16];
        if (*list) {
            b = *list;
            *list = b->next;
        }
    } else {
        for (tn_block **p = &tn_free_large; *p; p = &(*p)->next) {
            if ((*p)->size >= size) {
                b = *p;
                *p = b->next;
                break;
            }
        }
    }
    if (!b) {
        if (!tn_heap_top) tn_heap_top = __heap_start;
        if ((size_t)(__heap_end - tn_heap_top) < size + sizeof(tn_block)) {
            tn_kernel_out_of_memory();
            return NULL;
        }
        b = (tn_block *)tn_heap_top;
        b->size = size;
        tn_heap_top += size + sizeof(tn_block);
    }
    return b + 1;
}

void free(void *p) {
    if (!p) return;
    tn_block *b = (tn_block *)p - 1;
    if (b->size <= TN_SMALL_CLASSES * 16) {
        b->next = tn_free_small[b->size / 16];
        tn_free_small[b->size / 16] = b;
    } else {
        b->next = tn_free_large;
        tn_free_large = b;
    }
}

void *realloc(void *p, size_t n) {
    if (!p) return malloc(n);
    tn_block *b = (tn_block *)p - 1;
    if (n <= b->size) return p;
    void *q = malloc(n);
    if (q) {
        memcpy(q, p, b->size);
        free(p);
    }
    return q;
}

/* ---- formatting: the subset of printf the runtime uses ----
 * %s %c %d %u %x %%, the `l` / `ll` length modifiers, `*` widths and `.*` precisions. */

typedef struct { char *buf; size_t size, len; } tn_fmtbuf;

static void tn_fmt_putc(tn_fmtbuf *f, char c) {
    if (f->len + 1 < f->size) f->buf[f->len] = c;
    f->len++;
}

static void tn_fmt_pad(tn_fmtbuf *f, int n) {
    while (n-- > 0) tn_fmt_putc(f, ' ');
}

int vsnprintf(char *buf, size_t size, const char *fmt, va_list ap) {
    tn_fmtbuf f = {buf, size, 0};
    for (; *fmt; fmt++) {
        if (*fmt != '%') {
            tn_fmt_putc(&f, *fmt);
            continue;
        }
        fmt++;
        int width = 0, prec = -1, longs = 0;
        if (*fmt == '*') {
            width = va_arg(ap, int);
            fmt++;
        } else {
            while (*fmt >= '0' && *fmt <= '9') width = width * 10 + (*fmt++ - '0');
        }
        if (*fmt == '.' && fmt[1] == '*') {
            prec = va_arg(ap, int);
            fmt += 2;
        }
        while (*fmt == 'l') {
            longs++;
            fmt++;
        }
        char tmp[24];
        int n = 0;
        switch (*fmt) {
        case 's': {
            const char *s = va_arg(ap, const char *);
            if (!s) s = "(null)";
            int len = (int)strlen(s);
            if (prec >= 0 && prec < len) len = prec;
            tn_fmt_pad(&f, width - len);
            for (int i = 0; i < len; i++) tn_fmt_putc(&f, s[i]);
            continue;
        }
        case 'c':
            tn_fmt_pad(&f, width - 1);
            tn_fmt_putc(&f, (char)va_arg(ap, int));
            continue;
        case 'd':
        case 'i': {
            long long v = longs ? va_arg(ap, long long) : va_arg(ap, int);
            unsigned long long u = v < 0 ? 0 - (unsigned long long)v : (unsigned long long)v;
            do tmp[n++] = (char)('0' + u % 10); while (u /= 10);
            if (v < 0) tmp[n++] = '-';
            break;
        }
        case 'u':
        case 'x': {
            unsigned long long u = longs ? va_arg(ap, unsigned long long) : va_arg(ap, unsigned);
            unsigned base = *fmt == 'x' ? 16 : 10;
            do tmp[n++] = "0123456789abcdef"[u % base]; while (u /= base);
            break;
        }
        case '%':
            tn_fmt_putc(&f, '%');
            continue;
        default:
            continue;
        }
        tn_fmt_pad(&f, width - n);
        while (n) tn_fmt_putc(&f, tmp[--n]);
    }
    if (size) buf[f.len < size ? f.len : size - 1] = 0;
    return (int)f.len;
}

int snprintf(char *buf, size_t size, const char *fmt, ...) {
    va_list ap;
    va_start(ap, fmt);
    int n = vsnprintf(buf, size, fmt, ap);
    va_end(ap);
    return n;
}

/* ---- devices: the PL011 serial port and the CPU's generic timer ---- */

#define TN_UART ((volatile uint32_t *)0x09000000)
#define TN_UART_FR 6           /* flag register, in 32-bit words */
#define TN_UART_TXFF (1 << 5)  /* transmit FIFO full */
#define TN_UART_RXFE (1 << 4)  /* receive FIFO empty */

static void tn_uart_putc(char c) {
    while (TN_UART[TN_UART_FR] & TN_UART_TXFF) {
    }
    TN_UART[0] = (uint32_t)(unsigned char)c;
}

static int tn_uart_getc(void) {
    while (TN_UART[TN_UART_FR] & TN_UART_RXFE) {
    }
    return (int)(TN_UART[0] & 0xff);
}

/* Standard output and errors both go to the serial port. */
static void tn_out(const char *s, size_t n) {
    for (size_t i = 0; i < n; i++) {
        if (s[i] == '\n') tn_uart_putc('\r');
        tn_uart_putc(s[i]);
    }
}
#define tn_err tn_out

static void tn_flush(void) {}

static const char *tn_getenv(const char *name) {
    (void)name;
    return NULL;
}

static uint64_t tn_ticks(void) {
    uint64_t t;
    __asm__ volatile("isb; mrs %0, cntpct_el0" : "=r"(t));
    return t;
}

static uint64_t tn_tick_freq(void) {
    uint64_t f;
    __asm__ volatile("mrs %0, cntfrq_el0" : "=r"(f));
    return f;
}

/* Stops the machine. Under QEMU (with semihosting enabled) the exit code becomes
 * QEMU's own exit code, so tests can check it. */
__attribute__((noreturn)) static void tn_exit(int code) {
    uint64_t block[2] = {0x20026 /* ADP_Stopped_ApplicationExit */, (uint64_t)code};
    register uint64_t x0 __asm__("x0") = 0x18; /* SYS_EXIT */
    register uint64_t x1 __asm__("x1") = (uint64_t)block;
    __asm__ volatile("hlt #0xf000" : : "r"(x0), "r"(x1) : "memory");
    for (;;) __asm__ volatile("wfe");
}

void tn_kernel_out_of_memory(void) {
    tn_out("kernel panic: out of memory\n", 28);
    tn_exit(71);
}

void tn_kernel_trap(uint64_t kind, uint64_t esr, uint64_t elr, uint64_t far) {
    char msg[160];
    int n = snprintf(msg, sizeof msg,
                     "kernel panic: CPU exception %llu (ESR %llx) at %llx, address %llx\n",
                     (unsigned long long)kind, (unsigned long long)esr, (unsigned long long)elr,
                     (unsigned long long)far);
    tn_out(msg, (size_t)n);
    tn_exit(70);
}

/* ---- end of kernel platform ---- */

