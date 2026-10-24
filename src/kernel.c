/* ---- Glyph kernel platform: bare metal on QEMU's AArch64 `virt` machine ----
 * Included before the runtime when building with `--target kernel`. There is no
 * operating system underneath: this file boots the CPU and provides what the runtime
 * needs from one: memory allocation, text formatting, the serial port (for `print` and
 * `input`), the clock, and a way to stop the machine.
 *
 * At boot the MMU is switched on with page tables that give every region of memory
 * exactly the access it needs: kernel code is read-only and executable, constants are
 * read-only, data / stack / heap are writable but never executable, device registers
 * are uncached, and everything else (including address 0 and a guard region below the
 * stack) is unmapped, so a stray access traps instead of corrupting memory. The code
 * is built with -mstrict-align because it runs briefly with the MMU off at boot, when
 * unaligned accesses fault. */

#define TN_KERNEL 1

#include <stdarg.h>
#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

/* ---- boot ---- */

void tn_kernel_main(void);
void tn_kernel_boot(void);
void tn_kernel_irq(void);
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
    "2:  bl tn_kernel_boot\n"
    "tn_park:\n"
    "    wfe\n"
    "    b tn_park\n"
    /* Exception vectors: 16 entries of 128 bytes. Each passes its index to the trap
     * handler on a fresh stack. */
    ".balign 2048\n"
    "tn_vectors:\n"
    ".irp i, 0, 1, 2, 3, 4\n"
    "    .balign 128\n"
    "    mov x0, #\\i\n"
    "    b tn_trap_entry\n"
    ".endr\n"
    /* Entry 5: an interrupt while the kernel runs (current EL, SP_EL1). */
    "    .balign 128\n"
    "    b tn_irq_entry\n"
    ".irp i, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15\n"
    "    .balign 128\n"
    "    mov x0, #\\i\n"
    "    b tn_trap_entry\n"
    ".endr\n"
    /* Interrupts can arrive anywhere, so save every register the C handler may
     * change (the caller-saved general and SIMD registers), then return with eret. */
    "tn_irq_entry:\n"
    "    sub sp, sp, #560\n"
    "    stp x0, x1, [sp, #0]\n"
    "    stp x2, x3, [sp, #16]\n"
    "    stp x4, x5, [sp, #32]\n"
    "    stp x6, x7, [sp, #48]\n"
    "    stp x8, x9, [sp, #64]\n"
    "    stp x10, x11, [sp, #80]\n"
    "    stp x12, x13, [sp, #96]\n"
    "    stp x14, x15, [sp, #112]\n"
    "    stp x16, x17, [sp, #128]\n"
    "    stp x18, x29, [sp, #144]\n"
    "    str x30, [sp, #160]\n"
    "    stp q0, q1, [sp, #176]\n"
    "    stp q2, q3, [sp, #208]\n"
    "    stp q4, q5, [sp, #240]\n"
    "    stp q6, q7, [sp, #272]\n"
    "    stp q16, q17, [sp, #304]\n"
    "    stp q18, q19, [sp, #336]\n"
    "    stp q20, q21, [sp, #368]\n"
    "    stp q22, q23, [sp, #400]\n"
    "    stp q24, q25, [sp, #432]\n"
    "    stp q26, q27, [sp, #464]\n"
    "    stp q28, q29, [sp, #496]\n"
    "    stp q30, q31, [sp, #528]\n"
    "    bl tn_kernel_irq\n"
    "    ldp q30, q31, [sp, #528]\n"
    "    ldp q28, q29, [sp, #496]\n"
    "    ldp q26, q27, [sp, #464]\n"
    "    ldp q24, q25, [sp, #432]\n"
    "    ldp q22, q23, [sp, #400]\n"
    "    ldp q20, q21, [sp, #368]\n"
    "    ldp q18, q19, [sp, #336]\n"
    "    ldp q16, q17, [sp, #304]\n"
    "    ldp q6, q7, [sp, #272]\n"
    "    ldp q4, q5, [sp, #240]\n"
    "    ldp q2, q3, [sp, #208]\n"
    "    ldp q0, q1, [sp, #176]\n"
    "    ldr x30, [sp, #160]\n"
    "    ldp x18, x29, [sp, #144]\n"
    "    ldp x16, x17, [sp, #128]\n"
    "    ldp x14, x15, [sp, #112]\n"
    "    ldp x12, x13, [sp, #96]\n"
    "    ldp x10, x11, [sp, #80]\n"
    "    ldp x8, x9, [sp, #64]\n"
    "    ldp x6, x7, [sp, #48]\n"
    "    ldp x4, x5, [sp, #32]\n"
    "    ldp x2, x3, [sp, #16]\n"
    "    ldp x0, x1, [sp, #0]\n"
    "    add sp, sp, #560\n"
    "    eret\n"
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

/* Typed characters, filled by the serial port's interrupt. */
static volatile char tn_rx[256];
static volatile uint32_t tn_rx_head, tn_rx_tail;

/* Moves characters waiting in the serial port into tn_rx. */
static void tn_uart_drain(void) {
    while (!(TN_UART[TN_UART_FR] & TN_UART_RXFE)) {
        char c = (char)(TN_UART[0] & 0xff);
        if (tn_rx_head - tn_rx_tail < sizeof tn_rx) {
            tn_rx[tn_rx_head % sizeof tn_rx] = c;
            tn_rx_head++;
        }
    }
}

static inline void tn_irqs_off(void) { __asm__ volatile("msr daifset, #2" ::: "memory"); }
static inline void tn_irqs_on(void) { __asm__ volatile("msr daifclr, #2" ::: "memory"); }

/* Waits for the next typed character, sleeping (wfi) until an interrupt arrives.
 * Interrupts are masked while checking, so one can't slip in between the check and
 * the wfi; a pending interrupt still wakes wfi, and is taken once unmasked. */
static int tn_uart_getc(void) {
    for (;;) {
        tn_irqs_off();
        tn_uart_drain(); /* also catches a character whose interrupt was missed */
        if (tn_rx_head != tn_rx_tail) {
            char c = tn_rx[tn_rx_tail % sizeof tn_rx];
            tn_rx_tail++;
            tn_irqs_on();
            return (unsigned char)c;
        }
        __asm__ volatile("wfi");
        tn_irqs_on();
    }
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

/* Sleeps until `ms` milliseconds have passed. The CPU idles in wfi and wakes on each
 * timer tick (every 10 ms), so the wake-up can be up to one tick late. */
static void tn_sleep_ms(int64_t ms) {
    uint64_t end = tn_ticks() + (uint64_t)ms * (tn_tick_freq() / 1000);
    while (tn_ticks() < end) __asm__ volatile("wfi");
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

extern char __guard_start[], __stack_bottom[];

/* CPU exceptions: explain what went wrong, then stop the machine. */
void tn_kernel_trap(uint64_t kind, uint64_t esr, uint64_t elr, uint64_t far) {
    uint64_t ec = esr >> 26, fsc = esr & 0x3f;
    bool data = ec == 0x24 || ec == 0x25, code = ec == 0x20 || ec == 0x21;
    bool write = data && (esr & (1 << 6));
    bool permission = (fsc & 0x3c) == 0x0c;
    const char *what;
    if ((data || code) && far >= (uint64_t)__guard_start && far < (uint64_t)__stack_bottom)
        what = "stack overflow";
    else if ((data || code) && far < 0x10000)
        what = "null pointer access";
    else if (data && permission && write)
        what = "write to read-only memory";
    else if (code && permission)
        what = "tried to run memory that isn't code";
    else if (data || code)
        what = "access to unmapped memory";
    else
        what = "CPU exception";
    char msg[200];
    int n = snprintf(msg, sizeof msg,
                     "kernel panic: %s (address %llx, at instruction %llx; exception %llu, ESR %llx)\n",
                     what, (unsigned long long)far, (unsigned long long)elr,
                     (unsigned long long)kind, (unsigned long long)esr);
    tn_out(msg, (size_t)n);
    tn_exit(70);
}

/* ---- memory protection: page tables and the MMU ----
 * Identity mapping (virtual address = physical address) with a 4 KB granule and 39-bit
 * addresses, so translation starts at level 1 (1 GB per entry):
 *   0x00000000 - 0x07ffffff  unmapped: null pointers fault
 *   0x08000000 - 0x0fffffff  devices (interrupt controller, serial port, ...), uncached
 *   0x40000000 - kernel      unmapped
 *   kernel image             4 KB pages: code RX, constants R, data RW
 *   guard (2 MB)             unmapped: stack overflow faults
 *   stack, heap              RW, never executable, up to the end of RAM */

#define TN_PAGE 0x1000ULL
#define TN_BLOCK_2M 0x200000ULL
#define TN_RAM 0x40000000ULL
#define TN_L3_TABLES 8 /* page-granular mapping for the first 16 MB of RAM */

#define TN_D_BLOCK 1ULL        /* level 1/2: maps a whole block */
#define TN_D_TABLE 3ULL        /* level 1/2: points to the next table */
#define TN_D_PAGE 3ULL         /* level 3: maps one page */
#define TN_D_AF (1ULL << 10)   /* accessed flag (otherwise the first access faults) */
#define TN_D_INNER (3ULL << 8) /* inner shareable */
#define TN_D_RO (2ULL << 6)    /* read-only */
#define TN_D_PXN (1ULL << 53)  /* never executable by the kernel */
#define TN_D_UXN (1ULL << 54)  /* never executable by user code */
#define TN_D_NORMAL ((1ULL << 2) | TN_D_INNER | TN_D_AF) /* MAIR attribute 1 */
#define TN_D_DEVICE ((0ULL << 2) | TN_D_AF | TN_D_PXN | TN_D_UXN) /* attribute 0 */

static uint64_t tn_l1[512] __attribute__((aligned(4096)));
static uint64_t tn_l2_low[512] __attribute__((aligned(4096)));
static uint64_t tn_l2_ram[512] __attribute__((aligned(4096)));
static uint64_t tn_l3[TN_L3_TABLES][512] __attribute__((aligned(4096)));

extern char __text_start[], __text_end[], __rodata_end[], __image_end[];
static void tn_mmu_on(void) {
    for (uint64_t a = 0x08000000; a < 0x10000000; a += TN_BLOCK_2M)
        tn_l2_low[a / TN_BLOCK_2M] = a | TN_D_DEVICE | TN_D_BLOCK;
    tn_l1[0] = (uint64_t)tn_l2_low | TN_D_TABLE;

    uint64_t text = (uint64_t)__text_start, text_end = (uint64_t)__text_end;
    uint64_t ro_end = (uint64_t)__rodata_end, image_end = (uint64_t)__image_end;
    uint64_t tables = (image_end - TN_RAM) / TN_BLOCK_2M;
    if (tables > TN_L3_TABLES) {
        tn_out("kernel panic: the kernel image is too large to map\n", 51);
        tn_exit(72);
    }
    for (uint64_t a = text; a < image_end; a += TN_PAGE) {
        uint64_t i = (a - TN_RAM) / TN_PAGE, attr;
        if (a < text_end) attr = TN_D_NORMAL | TN_D_RO | TN_D_UXN;
        else if (a < ro_end) attr = TN_D_NORMAL | TN_D_RO | TN_D_PXN | TN_D_UXN;
        else attr = TN_D_NORMAL | TN_D_PXN | TN_D_UXN;
        tn_l3[i / 512][i % 512] = a | attr | TN_D_PAGE;
    }
    for (uint64_t t = 0; t < tables; t++) tn_l2_ram[t] = (uint64_t)tn_l3[t] | TN_D_TABLE;
    for (uint64_t a = (uint64_t)__stack_bottom; a < (uint64_t)__heap_end; a += TN_BLOCK_2M)
        tn_l2_ram[(a - TN_RAM) / TN_BLOCK_2M] = a | TN_D_NORMAL | TN_D_PXN | TN_D_UXN | TN_D_BLOCK;
    tn_l1[1] = (uint64_t)tn_l2_ram | TN_D_TABLE;

    uint64_t mmfr0;
    __asm__ volatile("mrs %0, id_aa64mmfr0_el1" : "=r"(mmfr0));
    uint64_t mair = 0x00 | (0xffULL << 8); /* 0: device nGnRnE, 1: normal write-back */
    uint64_t tcr = 25                      /* T0SZ: 39-bit addresses */
                   | (1ULL << 8)           /* inner write-back cacheable walks */
                   | (1ULL << 10)          /* outer write-back cacheable walks */
                   | (3ULL << 12)          /* inner shareable */
                   | (1ULL << 23)          /* no upper-half (TTBR1) translations */
                   | ((mmfr0 & 0xf) << 32); /* physical address size the CPU supports */
    __asm__ volatile("msr mair_el1, %0\n"
                     "msr tcr_el1, %1\n"
                     "msr ttbr0_el1, %2\n"
                     "dsb ish\n"
                     "isb\n"
                     "tlbi vmalle1\n"
                     "dsb ish\n"
                     "isb\n"
                     :
                     : "r"(mair), "r"(tcr), "r"((uint64_t)tn_l1)
                     : "memory");
    uint64_t sctlr;
    __asm__ volatile("mrs %0, sctlr_el1" : "=r"(sctlr));
    sctlr |= (1 << 0)    /* MMU on */
             | (1 << 2)  /* data cache on */
             | (1 << 12) /* instruction cache on */
             | (1 << 19); /* writable memory is never executable */
    sctlr &= ~(uint64_t)(1 << 1); /* no alignment checking */
    __asm__ volatile("msr sctlr_el1, %0\nisb\nic iallu\ndsb ish\nisb" : : "r"(sctlr) : "memory");
}

#ifdef TN_SELFTEST
/* Deliberate faults, to test the protection (GLYPH_KERNEL_CFLAGS=-DTN_SELFTEST=n). */
static int tn_selftest_deep(int n) {
    volatile char buf[1024];
    buf[0] = (char)n;
    return tn_selftest_deep(n + 1) + buf[0];
}

static void tn_selftest(int which) {
    static uint32_t data_code[4];
    switch (which) {
    case 1: *(volatile uint32_t *)(void *)&tn_mmu_on = 0; break;     /* write to code */
    case 2: (void)*(volatile uint64_t *)(uintptr_t)8; break;         /* null pointer */
    case 3: tn_selftest_deep(0); break;                              /* stack overflow */
    case 4: ((void (*)(void))(void *)data_code)(); break;            /* run data */
    }
}
#endif

/* ---- interrupts: the GICv2 interrupt controller, the timer, the serial port ---- */

#define TN_GICD ((volatile uint32_t *)0x08000000)   /* distributor */
#define TN_GICD8 ((volatile uint8_t *)0x08000000)
#define TN_GICC ((volatile uint32_t *)0x08010000)   /* CPU interface */
#define TN_IRQ_TIMER 27                             /* EL1 virtual timer (a PPI) */
#define TN_IRQ_UART 33                              /* PL011 serial port (SPI 1) */
#define TN_HZ 100                                   /* timer ticks per second */

static volatile uint64_t tn_irq_ticks; /* timer interrupts handled */

static void tn_timer_arm(void) {
    uint64_t interval = tn_tick_freq() / TN_HZ;
    __asm__ volatile("msr cntv_tval_el0, %0\nmsr cntv_ctl_el0, %1\nisb" : : "r"(interval), "r"(1ULL));
}

void tn_kernel_irq(void) {
    uint32_t iar = TN_GICC[0x00c / 4]; /* acknowledge */
    uint32_t id = iar & 0x3ff;
    if (id == TN_IRQ_TIMER) {
        tn_irq_ticks++;
        tn_timer_arm();
    } else if (id == TN_IRQ_UART) {
        /* Reading a character clears its interrupt. (Clearing it by hand afterwards
         * would race with a character arriving just then, losing its interrupt.) */
        tn_uart_drain();
    }
    if (id < 1020) TN_GICC[0x010 / 4] = iar; /* end of interrupt */
}

static void tn_interrupts_on(void) {
    TN_GICD[0x000 / 4] = 1;                                  /* distributor on */
    TN_GICD[0x100 / 4] = 1u << TN_IRQ_TIMER;                 /* enable the timer PPI */
    TN_GICD[0x104 / 4] = 1u << (TN_IRQ_UART - 32);           /* enable the UART SPI */
    TN_GICD8[0x400 + TN_IRQ_TIMER] = 0x80;                   /* priorities */
    TN_GICD8[0x400 + TN_IRQ_UART] = 0x80;
    TN_GICD8[0x800 + TN_IRQ_UART] = 0x01;                    /* deliver to CPU 0 */
    TN_GICC[0x004 / 4] = 0xff;                               /* accept all priorities */
    TN_GICC[0x000 / 4] = 1;                                  /* CPU interface on */
    TN_UART[0x038 / 4] = (1 << 4) | (1 << 6);                /* UART: interrupt on receive */
    tn_timer_arm();
    tn_irqs_on();
}

void tn_kernel_boot(void) {
#ifndef TN_NO_MMU /* GLYPH_KERNEL_CFLAGS=-DTN_NO_MMU: run without protection or caches */
    tn_mmu_on();
#endif
    tn_interrupts_on();
#ifdef TN_SELFTEST
    tn_selftest(TN_SELFTEST);
#endif
    tn_kernel_main();
}

/* ---- end of kernel platform ---- */

