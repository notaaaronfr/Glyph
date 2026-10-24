/* ---- Glyph OS kernel: bare metal on QEMU's AArch64 `virt` machine ----
 * Compiled after kernel_api.h, as its own C file; each app is another C file (the
 * runtime plus the app's code) linked into the same kernel image. There is no
 * operating system underneath: this file boots the CPU and provides what the apps
 * need from one: memory, text formatting, the serial port (for `print` and `input`),
 * the clock, and running several apps at once.
 *
 * Memory protection: at boot the MMU is switched on with page tables that give every
 * region of memory exactly the access it needs: kernel code is read-only and
 * executable, constants are read-only, data / stacks / heaps are writable but never
 * executable, device registers are uncached, and everything else (including address 0
 * and a guard region below every stack) is unmapped, so a stray access traps instead
 * of corrupting memory. The code is built with -mstrict-align because it runs briefly
 * with the MMU off at boot, when unaligned accesses fault.
 *
 * Multitasking: each app is a task with its own stack and its own heap, so apps share
 * no memory. The timer interrupt switches tasks every tick (preemption), and tasks
 * waiting in `sleep` or `input` are parked until they can continue. An app that fails
 * stops alone; the machine powers off when every app has finished.
 *
 * Permissions: each app may use only the effects its `main` declares. The kernel
 * checks every service call (tn_need), on top of the compiler's own check. */

/* ---- apps, as listed by the compiler (generated, linked in) ---- */

/* Effects an app may use. The kernel grants each app exactly what its `main`
 * declares and checks every service call against that, so even an app whose
 * compiled code was changed can't do more than it declared. */
enum { TN_PERM_IO = 1, TN_PERM_TIME = 2, TN_PERM_RAND = 4 };

typedef struct {
    const char *name;
    void (*entry)(void);
    unsigned perms; /* TN_PERM_* */
} tn_app;
extern const tn_app tn_apps[];
extern const int tn_app_count;

void tn_kernel_boot(void);
void tn_kernel_irq(void);
void tn_kernel_trap(uint64_t kind, uint64_t esr, uint64_t elr, uint64_t far);

/* Saved registers of a task that isn't running: the callee-saved registers x19-x28,
 * the frame pointer x29, the return address x30, sp, and SIMD d8-d15. */
typedef struct { uint64_t r[21]; } tn_ctx;
void tn_switch(tn_ctx *from, tn_ctx *to);

/* ---- boot, exception vectors, context switch ---- */

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
     * handler on a fresh stack, except entry 5: an interrupt while running at EL1. */
    ".balign 2048\n"
    "tn_vectors:\n"
    ".irp i, 0, 1, 2, 3, 4\n"
    "    .balign 128\n"
    "    mov x0, #\\i\n"
    "    b tn_trap_entry\n"
    ".endr\n"
    "    .balign 128\n"
    "    b tn_irq_entry\n"
    ".irp i, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15\n"
    "    .balign 128\n"
    "    mov x0, #\\i\n"
    "    b tn_trap_entry\n"
    ".endr\n"
    /* Interrupts can arrive anywhere, so save every register the C handler may change
     * (the caller-saved general and SIMD registers), plus the interrupted code's
     * return address and state (ELR, SPSR): the handler may switch to another task,
     * and that task's own interrupt return must not use ours. */
    "tn_irq_entry:\n"
    "    sub sp, sp, #576\n"
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
    "    mrs x0, elr_el1\n"
    "    mrs x1, spsr_el1\n"
    "    str x0, [sp, #560]\n"
    "    str x1, [sp, #568]\n"
    "    bl tn_kernel_irq\n"
    "    ldr x0, [sp, #560]\n"
    "    ldr x1, [sp, #568]\n"
    "    msr elr_el1, x0\n"
    "    msr spsr_el1, x1\n"
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
    "    add sp, sp, #576\n"
    "    eret\n"
    "tn_trap_entry:\n"
    "    mrs x1, esr_el1\n"
    "    mrs x2, elr_el1\n"
    "    mrs x3, far_el1\n"
    "    ldr x4, =__stack_top\n"
    "    mov sp, x4\n"
    "    bl tn_kernel_trap\n"
    "    b tn_park\n"
    /* tn_switch(from, to): save the callee-saved registers and sp into *from, load
     * *to's, and return into the task they belong to. */
    ".text\n"
    ".global tn_switch\n"
    "tn_switch:\n"
    "    stp x19, x20, [x0, #0]\n"
    "    stp x21, x22, [x0, #16]\n"
    "    stp x23, x24, [x0, #32]\n"
    "    stp x25, x26, [x0, #48]\n"
    "    stp x27, x28, [x0, #64]\n"
    "    stp x29, x30, [x0, #80]\n"
    "    mov x9, sp\n"
    "    str x9, [x0, #96]\n"
    "    stp d8, d9, [x0, #104]\n"
    "    stp d10, d11, [x0, #120]\n"
    "    stp d12, d13, [x0, #136]\n"
    "    stp d14, d15, [x0, #152]\n"
    "    ldp x19, x20, [x1, #0]\n"
    "    ldp x21, x22, [x1, #16]\n"
    "    ldp x23, x24, [x1, #32]\n"
    "    ldp x25, x26, [x1, #48]\n"
    "    ldp x27, x28, [x1, #64]\n"
    "    ldp x29, x30, [x1, #80]\n"
    "    ldr x9, [x1, #96]\n"
    "    mov sp, x9\n"
    "    ldp d8, d9, [x1, #104]\n"
    "    ldp d10, d11, [x1, #120]\n"
    "    ldp d12, d13, [x1, #136]\n"
    "    ldp d14, d15, [x1, #152]\n"
    "    ret\n");

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

/* ---- formatting: the subset of printf the kernel and runtime use ----
 * %s %c %d %u %x %%, the `-` flag, `l` / `ll`, widths (digits or `*`) and `.*`. */

typedef struct { char *buf; size_t size, len; } tn_fmtbuf;

static void tn_fmt_putc(tn_fmtbuf *f, char c) {
    if (f->len + 1 < f->size) f->buf[f->len] = c;
    f->len++;
}

static void tn_fmt_pad(tn_fmtbuf *f, int n) {
    while (n-- > 0) tn_fmt_putc(f, ' ');
}

/* Writes s[0..len), padded with spaces to `width` on the left, or on the right. */
static void tn_fmt_field(tn_fmtbuf *f, const char *s, int len, int width, bool left) {
    if (!left) tn_fmt_pad(f, width - len);
    for (int i = 0; i < len; i++) tn_fmt_putc(f, s[i]);
    if (left) tn_fmt_pad(f, width - len);
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
        bool left = false;
        if (*fmt == '-') {
            left = true;
            fmt++;
        }
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
            tn_fmt_field(&f, s, len, width, left);
            continue;
        }
        case 'c': {
            char c = (char)va_arg(ap, int);
            tn_fmt_field(&f, &c, 1, width, left);
            continue;
        }
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
        char digits[24];
        for (int i = 0; i < n; i++) digits[i] = tmp[n - 1 - i];
        tn_fmt_field(&f, digits, n, width, left);
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

/* ---- interrupt masking ---- */

static inline void tn_irqs_off(void) { __asm__ volatile("msr daifset, #2" ::: "memory"); }
static inline void tn_irqs_on(void) { __asm__ volatile("msr daifclr, #2" ::: "memory"); }

static inline uint64_t tn_irq_save(void) {
    uint64_t daif;
    __asm__ volatile("mrs %0, daif" : "=r"(daif));
    tn_irqs_off();
    return daif;
}

static inline void tn_irq_restore(uint64_t daif) {
    __asm__ volatile("msr daif, %0" : : "r"(daif) : "memory");
}

/* ---- memory layout ----
 * After the kernel image come 8 stack slots of 10 MB, each a 2 MB unmapped guard
 * followed by an 8 MB stack (slot 0: boot and the kernel; slots 1-7: apps), then
 * the heaps, one region per app, up to the end of the 256 MB of RAM. */

#define TN_MAX_APPS 7
#define TN_SLOT 0xa00000ULL
#define TN_GUARD 0x200000ULL
extern char __stacks_start[], __heap_start[], __heap_end[];

/* ---- tasks ---- */

typedef struct tn_block { size_t size; struct tn_block *next; } tn_block;
#define TN_SMALL_CLASSES 64

/* A task's private heap. Blocks carry a 16-byte header with their usable size. Freed
 * blocks go on a free list per size class (multiples of 16 bytes up to 1 KB) or on
 * one list for larger blocks; new memory comes from the task's region. Reference
 * counting frees most blocks soon after use, so they are reused quickly. */
typedef struct {
    tn_block *small[TN_SMALL_CLASSES + 1];
    tn_block *large;
    char *top, *end;
} tn_heap;

enum { TN_READY, TN_SLEEPING, TN_WAITING_INPUT, TN_DONE };

typedef struct {
    const tn_app *app;
    int state;
    int exit_code;
    uint64_t wake_at;   /* TN_SLEEPING: counter value to wake at */
    uint64_t cpu_ticks; /* timer ticks it was running for */
    tn_ctx ctx;
    tn_heap heap;
} tn_task;

static tn_task tn_tasks[TN_MAX_APPS];
static int tn_ntasks;
static tn_task *tn_cur; /* NULL while booting */
static tn_ctx tn_boot_ctx;

/* ---- memory allocation (from the running task's heap) ---- */

static void tn_out_of_memory(void);

void *malloc(size_t n) {
    tn_heap *h = &tn_cur->heap;
    size_t size = (n + 15) & ~(size_t)15;
    if (size == 0) size = 16;
    tn_block *b = NULL;
    if (size <= TN_SMALL_CLASSES * 16) {
        tn_block **list = &h->small[size / 16];
        if (*list) {
            b = *list;
            *list = b->next;
        }
    } else {
        for (tn_block **p = &h->large; *p; p = &(*p)->next) {
            if ((*p)->size >= size) {
                b = *p;
                *p = b->next;
                break;
            }
        }
    }
    if (!b) {
        if ((size_t)(h->end - h->top) < size + sizeof(tn_block)) {
            tn_out_of_memory();
            return NULL;
        }
        b = (tn_block *)h->top;
        b->size = size;
        h->top += size + sizeof(tn_block);
    }
    return b + 1;
}

void free(void *p) {
    if (!p) return;
    tn_heap *h = &tn_cur->heap;
    tn_block *b = (tn_block *)p - 1;
    if (b->size <= TN_SMALL_CLASSES * 16) {
        b->next = h->small[b->size / 16];
        h->small[b->size / 16] = b;
    } else {
        b->next = h->large;
        h->large = b;
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

/* Writes to the serial port. Interrupts are masked for the whole call, so another
 * app's output can't land in the middle of it. */
static void tn_write(const char *s, size_t n) {
    uint64_t daif = tn_irq_save();
    for (size_t i = 0; i < n; i++) {
        if (s[i] == '\n') tn_uart_putc('\r');
        tn_uart_putc(s[i]);
    }
    tn_irq_restore(daif);
}

static uint64_t tn_counter(void) {
    uint64_t t;
    __asm__ volatile("isb; mrs %0, cntpct_el0" : "=r"(t));
    return t;
}

static uint64_t tn_counter_freq(void) {
    uint64_t f;
    __asm__ volatile("mrs %0, cntfrq_el0" : "=r"(f));
    return f;
}

/* Stops the machine. Under QEMU (with semihosting enabled) the exit code becomes
 * QEMU's own exit code, so tests can check it. */
__attribute__((noreturn)) static void tn_halt(int code) {
    uint64_t block[2] = {0x20026 /* ADP_Stopped_ApplicationExit */, (uint64_t)code};
    register uint64_t x0 __asm__("x0") = 0x18; /* SYS_EXIT */
    register uint64_t x1 __asm__("x1") = (uint64_t)block;
    __asm__ volatile("hlt #0xf000" : : "r"(x0), "r"(x1) : "memory");
    for (;;) __asm__ volatile("wfe");
}

static void tn_outf(const char *fmt, ...) {
    char buf[200];
    va_list ap;
    va_start(ap, fmt);
    int n = vsnprintf(buf, sizeof buf, fmt, ap);
    va_end(ap);
    tn_write(buf, (size_t)(n < (int)sizeof buf ? n : (int)sizeof buf - 1));
}

/* "io, time", or "nothing". */
static void tn_perm_names(unsigned perms, char *buf, size_t size) {
    static const char *names[] = {"io", "time", "rand"}; /* in TN_PERM_* bit order */
    size_t n = 0;
    for (int i = 0; i < 3; i++)
        if (perms & (1u << i)) n += (size_t)snprintf(buf + n, size - n, "%s%s", n ? ", " : "", names[i]);
    if (!n) snprintf(buf, size, "nothing");
}

/* ---- the scheduler ----
 * Round robin over ready tasks. It runs with interrupts masked: from the timer
 * interrupt (preemption), or when a task blocks or exits. */

#define TN_HZ 100 /* timer ticks per second */

/* Makes sleeping and waiting tasks ready when their time comes or input arrives. */
static void tn_wake(void) {
    uint64_t now = tn_counter();
    bool input = tn_rx_head != tn_rx_tail;
    for (int i = 0; i < tn_ntasks; i++) {
        tn_task *t = &tn_tasks[i];
        if ((t->state == TN_SLEEPING && now >= t->wake_at) || (t->state == TN_WAITING_INPUT && input))
            t->state = TN_READY;
    }
}

/* The next ready task after the current one, or NULL. */
static tn_task *tn_pick(void) {
    int start = tn_cur ? (int)(tn_cur - tn_tasks) : tn_ntasks - 1;
    for (int k = 1; k <= tn_ntasks; k++) {
        tn_task *t = &tn_tasks[(start + k) % tn_ntasks];
        if (t->state == TN_READY) return t;
    }
    return NULL;
}

static void tn_run(tn_task *next) {
    tn_task *prev = tn_cur;
    tn_cur = next;
    if (prev != next) tn_switch(prev ? &prev->ctx : &tn_boot_ctx, &next->ctx);
}

__attribute__((noreturn)) static void tn_power_off(void) {
    int code = 0;
    for (int i = 0; i < tn_ntasks; i++)
        if (tn_tasks[i].exit_code != 0) {
            code = tn_tasks[i].exit_code;
            break;
        }
    if (tn_ntasks > 1) {
        tn_outf("Glyph OS: all %d apps finished\n", tn_ntasks);
        for (int i = 0; i < tn_ntasks; i++) {
            tn_task *t = &tn_tasks[i];
            char perms[32];
            tn_perm_names(t->app->perms, perms, sizeof perms);
            tn_outf("  %-12s exit %d   cpu %llu ms   allowed %s\n", t->app->name, t->exit_code,
                    (unsigned long long)(t->cpu_ticks * 1000 / TN_HZ), perms);
        }
    }
    tn_halt(code);
}

/* The current task can't continue (it sleeps, waits or has finished): run another,
 * or idle until an interrupt makes one ready. Returns once the current task is
 * ready again. */
static void tn_block_current(void) {
    for (;;) {
        tn_wake();
        tn_task *next = tn_pick();
        if (next) {
            tn_run(next);
            return;
        }
        bool all_done = true;
        for (int i = 0; i < tn_ntasks; i++) all_done &= tn_tasks[i].state == TN_DONE;
        if (all_done) tn_power_off();
        __asm__ volatile("wfi");
        tn_irqs_on(); /* take the interrupt that woke us */
        tn_irqs_off();
    }
}

/* From the timer interrupt: give the CPU to the next ready task. */
static void tn_preempt(void) {
    if (!tn_cur || tn_cur->state != TN_READY) return; /* idling: tn_block_current decides */
    tn_wake();
    tn_task *next = tn_pick();
    if (next) tn_run(next);
}

/* Where a new task starts (tn_switch "returns" here on its fresh stack). */
static void tn_task_start(void) {
    tn_irqs_on();
    tn_cur->app->entry();
    tn_exit(0);
}

/* ---- what the kernel API offers apps ----
 * Every service that has an effect first checks that the app was granted it. An app
 * that asks for more than it declared is stopped, and the other apps keep running. */

#define TN_EXIT_DENIED 77

static void tn_need(unsigned perm, const char *effect, const char *what) {
    if (!tn_cur || (tn_cur->app->perms & perm)) return;
    char allowed[32];
    tn_perm_names(tn_cur->app->perms, allowed, sizeof allowed);
    tn_outf("[Glyph OS: denied: app %s tried to %s, which needs `%s`, but it is only allowed %s]\n",
            tn_cur->app->name, what, effect, allowed);
    tn_exit(TN_EXIT_DENIED);
}

void tn_out(const char *s, size_t n) {
    tn_need(TN_PERM_IO, "io", "print");
    tn_write(s, n);
}

/* Error reports (failed contracts and the like) are always allowed. */
void tn_err(const char *s, size_t n) { tn_write(s, n); }

void tn_flush(void) {}

const char *tn_getenv(const char *name) {
    (void)name;
    return NULL;
}

uint64_t tn_ticks(void) {
    tn_need(TN_PERM_TIME, "time", "read the clock");
    return tn_counter();
}

uint64_t tn_tick_freq(void) {
    tn_need(TN_PERM_TIME, "time", "read the clock");
    return tn_counter_freq();
}

uint64_t tn_random_seed(void) {
    tn_need(TN_PERM_RAND, "rand", "get random numbers");
    return tn_counter() ^ 0x9e3779b97f4a7c15ULL;
}

void tn_sleep_ms(int64_t ms) {
    tn_need(TN_PERM_TIME, "time", "sleep");
    uint64_t end = tn_counter() + (uint64_t)ms * (tn_counter_freq() / 1000);
    tn_irqs_off();
    if (tn_counter() < end) {
        tn_cur->wake_at = end;
        tn_cur->state = TN_SLEEPING;
        tn_block_current();
    }
    tn_irqs_on();
}

/* Waits for the next typed character. Interrupts are masked while checking, so a
 * character can't arrive unnoticed between the check and parking the task. */
int tn_uart_getc(void) {
    tn_need(TN_PERM_IO, "io", "read input");
    for (;;) {
        tn_irqs_off();
        tn_uart_drain(); /* also catches a character whose interrupt was missed */
        if (tn_rx_head != tn_rx_tail) {
            char c = tn_rx[tn_rx_tail % sizeof tn_rx];
            tn_rx_tail++;
            tn_irqs_on();
            return (unsigned char)c;
        }
        tn_cur->state = TN_WAITING_INPUT;
        tn_block_current();
        tn_irqs_on();
    }
}

/* Ends the current app. Before any app runs (the boot self-tests), stops the machine. */
void tn_exit(int code) {
    tn_irqs_off();
    if (!tn_cur) tn_halt(code);
    tn_cur->exit_code = code;
    tn_cur->state = TN_DONE;
    if (tn_ntasks > 1 && code != 0)
        tn_outf("[Glyph OS: app %s stopped with exit code %d; the other apps keep running]\n",
                tn_cur->app->name, code);
    tn_block_current();
    for (;;) {
    }
}

static void tn_out_of_memory(void) {
    tn_outf("%s: out of memory (each app has its own heap)\n", tn_cur->app->name);
    tn_exit(71);
}

/* ---- CPU exceptions: explain what went wrong, then stop the machine ---- */

void tn_kernel_trap(uint64_t kind, uint64_t esr, uint64_t elr, uint64_t far) {
    uint64_t ec = esr >> 26, fsc = esr & 0x3f;
    bool data = ec == 0x24 || ec == 0x25, code = ec == 0x20 || ec == 0x21;
    bool write = data && (esr & (1 << 6));
    bool permission = (fsc & 0x3c) == 0x0c;
    uint64_t stacks = (uint64_t)__stacks_start;
    bool in_guard = far >= stacks && far < stacks + 8 * TN_SLOT && (far - stacks) % TN_SLOT < TN_GUARD;
    const char *what;
    if ((data || code) && in_guard)
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
    tn_outf("kernel panic: %s (address %llx, at instruction %llx; exception %llu, ESR %llx)\n",
            what, (unsigned long long)far, (unsigned long long)elr, (unsigned long long)kind,
            (unsigned long long)esr);
    tn_halt(70);
}

/* ---- memory protection: page tables and the MMU ----
 * Identity mapping (virtual address = physical address) with a 4 KB granule and 39-bit
 * addresses, so translation starts at level 1 (1 GB per entry):
 *   0x00000000 - 0x07ffffff  unmapped: null pointers fault
 *   0x08000000 - 0x0fffffff  devices (interrupt controller, serial port, ...), uncached
 *   0x40000000 - kernel      unmapped
 *   kernel image             4 KB pages: code RX, constants R, data RW
 *   stack slots              per slot: 2 MB unmapped guard, then an 8 MB RW stack
 *   heaps                    RW, never executable, up to the end of RAM */

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
#define TN_D_DATA (TN_D_NORMAL | TN_D_PXN | TN_D_UXN | TN_D_BLOCK)

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
        tn_write("kernel panic: the kernel image is too large to map\n", 51);
        tn_halt(72);
    }
    for (uint64_t a = text; a < image_end; a += TN_PAGE) {
        uint64_t i = (a - TN_RAM) / TN_PAGE, attr;
        if (a < text_end) attr = TN_D_NORMAL | TN_D_RO | TN_D_UXN;
        else if (a < ro_end) attr = TN_D_NORMAL | TN_D_RO | TN_D_PXN | TN_D_UXN;
        else attr = TN_D_NORMAL | TN_D_PXN | TN_D_UXN;
        tn_l3[i / 512][i % 512] = a | attr | TN_D_PAGE;
    }
    for (uint64_t t = 0; t < tables; t++) tn_l2_ram[t] = (uint64_t)tn_l3[t] | TN_D_TABLE;
    uint64_t stacks = (uint64_t)__stacks_start;
    for (uint64_t s = 0; s < 8; s++)
        for (uint64_t a = stacks + s * TN_SLOT + TN_GUARD; a < stacks + (s + 1) * TN_SLOT; a += TN_BLOCK_2M)
            tn_l2_ram[(a - TN_RAM) / TN_BLOCK_2M] = a | TN_D_DATA;
    for (uint64_t a = (uint64_t)__heap_start; a < (uint64_t)__heap_end; a += TN_BLOCK_2M)
        tn_l2_ram[(a - TN_RAM) / TN_BLOCK_2M] = a | TN_D_DATA;
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

static void tn_timer_arm(void) {
    uint64_t interval = tn_counter_freq() / TN_HZ;
    __asm__ volatile("msr cntv_tval_el0, %0\nmsr cntv_ctl_el0, %1\nisb" : : "r"(interval), "r"(1ULL));
}

void tn_kernel_irq(void) {
    uint32_t iar = TN_GICC[0x00c / 4]; /* acknowledge */
    uint32_t id = iar & 0x3ff;
    if (id == TN_IRQ_TIMER) {
        if (tn_cur && tn_cur->state == TN_READY) tn_cur->cpu_ticks++;
        tn_timer_arm();
    } else if (id == TN_IRQ_UART) {
        /* Reading a character clears its interrupt. (Clearing it by hand afterwards
         * would race with a character arriving just then, losing its interrupt.) */
        tn_uart_drain();
    }
    if (id < 1020) TN_GICC[0x010 / 4] = iar; /* end of interrupt */
    if (id == TN_IRQ_TIMER || id == TN_IRQ_UART) tn_preempt();
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

/* ---- boot: protection, interrupts, then start the apps ---- */

void tn_kernel_boot(void) {
#ifndef TN_NO_MMU /* GLYPH_KERNEL_CFLAGS=-DTN_NO_MMU: run without protection or caches */
    tn_mmu_on();
#endif
    tn_interrupts_on();
#ifdef TN_SELFTEST
    tn_selftest(TN_SELFTEST);
#endif
    tn_irqs_off();
    tn_ntasks = tn_app_count < TN_MAX_APPS ? tn_app_count : TN_MAX_APPS;
    uint64_t heap = (uint64_t)__heap_start;
    uint64_t share = (((uint64_t)__heap_end - heap) / (uint64_t)tn_ntasks) & ~(uint64_t)15;
    for (int i = 0; i < tn_ntasks; i++) {
        tn_task *t = &tn_tasks[i];
        t->app = &tn_apps[i];
        t->state = TN_READY;
        t->heap.top = (char *)(heap + (uint64_t)i * share);
        t->heap.end = t->heap.top + share;
        /* Slot i + 1: the stack grows down from the top of the slot. */
        t->ctx.r[11] = (uint64_t)&tn_task_start; /* x30: where tn_switch returns to */
        t->ctx.r[12] = (uint64_t)__stacks_start + (uint64_t)(i + 2) * TN_SLOT; /* sp */
    }
    tn_run(&tn_tasks[0]);
}

/* ---- end of kernel ---- */
