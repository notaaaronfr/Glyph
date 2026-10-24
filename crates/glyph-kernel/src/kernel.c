/* ---- Glyph OS kernel: bare metal on QEMU's AArch64 `virt` machine ----
 * Compiled after abi.h, as its own C file; each app is another C file (the runtime
 * plus the app's code), and libc.c is shared by all of them, linked into one kernel
 * image. There is no operating system underneath: this file boots the CPU and
 * provides what the apps need from one: memory, the serial port (for `print` and
 * `input`), the clock, and running several apps at once.
 *
 * User mode: apps run at EL0, the CPU's unprivileged level, each with its own page
 * tables. An app can touch only its own code, data, stack and heap; the kernel, the
 * devices and the other apps are out of its reach, and it can't mask interrupts or
 * read the timer itself. It asks the kernel for everything else with system calls
 * (`svc`), and the kernel checks every pointer it is handed. A fault in an app stops
 * that app, not the machine.
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
 * checks every system call (tn_need), on top of the compiler's own check. */

/* ---- apps, as listed by the compiler (generated, linked in) ---- */

/* Effects an app may use. The kernel grants each app exactly what its `main`
 * declares and checks every service call against that, so even an app whose
 * compiled code was changed can't do more than it declared. */
enum { TN_PERM_IO = 1, TN_PERM_TIME = 2, TN_PERM_RAND = 4 };

typedef struct {
    const char *name;
    void (*entry)(void);
    unsigned perms; /* TN_PERM_* */
    /* The app's part of the image: code, then constants, then data, page-aligned. */
    char *start, *text_end, *ro_end, *end;
} tn_app;
extern const tn_app tn_apps[];
extern const int tn_app_count;

void tn_kernel_boot(void);
void tn_kernel_irq(void);
void tn_kernel_trap(uint64_t kind, uint64_t esr, uint64_t elr, uint64_t far);

/* What an app was doing when it entered the kernel: all its registers. The SIMD
 * registers q0-q31 follow these fields in the frame. */
typedef struct { uint64_t x[31], sp, elr, spsr, fpsr, fpcr; } tn_frame;
#define TN_FRAME_SIZE 800
void tn_user_exception(uint64_t kind, tn_frame *f);
void tn_enter_user(void (*entry)(void), uint64_t sp);

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
    /* Exception vectors: 16 entries of 128 bytes. Entries 0-7 are taken in the kernel:
     * entry 5 is an interrupt, the rest are kernel faults, passed by index to the
     * trap handler on a fresh stack. Entries 8-15 come from an app in user mode: they
     * save the app's registers on its kernel stack and pass the index along. */
    ".balign 2048\n"
    "tn_vectors:\n"
    ".irp i, 0, 1, 2, 3, 4\n"
    "    .balign 128\n"
    "    mov x0, #\\i\n"
    "    b tn_trap_entry\n"
    ".endr\n"
    "    .balign 128\n"
    "    b tn_irq_entry\n"
    ".irp i, 6, 7\n"
    "    .balign 128\n"
    "    mov x0, #\\i\n"
    "    b tn_trap_entry\n"
    ".endr\n"
    ".irp i, 8, 9, 10, 11, 12, 13, 14, 15\n"
    "    .balign 128\n"
    "    sub sp, sp, #800\n"
    "    stp x0, x1, [sp, #0]\n"
    "    mov x0, #\\i\n"
    "    b tn_user_entry\n"
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
    /* From user mode: save everything the app had (the kernel may switch to another
     * app before returning to this one), then call tn_user_exception(kind, frame).
     * Frame layout (tn_frame): x0-x30, sp_el0, elr, spsr, fpsr, fpcr, then q0-q31. */
    "tn_user_entry:\n"
    "    stp x2, x3, [sp, #16]\n"
    "    stp x4, x5, [sp, #32]\n"
    "    stp x6, x7, [sp, #48]\n"
    "    stp x8, x9, [sp, #64]\n"
    "    stp x10, x11, [sp, #80]\n"
    "    stp x12, x13, [sp, #96]\n"
    "    stp x14, x15, [sp, #112]\n"
    "    stp x16, x17, [sp, #128]\n"
    "    stp x18, x19, [sp, #144]\n"
    "    stp x20, x21, [sp, #160]\n"
    "    stp x22, x23, [sp, #176]\n"
    "    stp x24, x25, [sp, #192]\n"
    "    stp x26, x27, [sp, #208]\n"
    "    stp x28, x29, [sp, #224]\n"
    "    str x30, [sp, #240]\n"
    "    mrs x21, sp_el0\n"
    "    mrs x22, elr_el1\n"
    "    mrs x23, spsr_el1\n"
    "    mrs x24, fpsr\n"
    "    mrs x25, fpcr\n"
    "    stp x21, x22, [sp, #248]\n"
    "    str x23, [sp, #264]\n"
    "    stp x24, x25, [sp, #272]\n"
    "    stp q0, q1, [sp, #288]\n"
    "    stp q2, q3, [sp, #320]\n"
    "    stp q4, q5, [sp, #352]\n"
    "    stp q6, q7, [sp, #384]\n"
    "    stp q8, q9, [sp, #416]\n"
    "    stp q10, q11, [sp, #448]\n"
    "    stp q12, q13, [sp, #480]\n"
    "    stp q14, q15, [sp, #512]\n"
    "    stp q16, q17, [sp, #544]\n"
    "    stp q18, q19, [sp, #576]\n"
    "    stp q20, q21, [sp, #608]\n"
    "    stp q22, q23, [sp, #640]\n"
    "    stp q24, q25, [sp, #672]\n"
    "    stp q26, q27, [sp, #704]\n"
    "    stp q28, q29, [sp, #736]\n"
    "    stp q30, q31, [sp, #768]\n"
    "    mov x1, sp\n"
    "    bl tn_user_exception\n"
    /* Back to the app, with interrupts masked until eret restores its state. */
    "tn_user_return:\n"
    "    msr daifset, #2\n"
    "    ldp x21, x22, [sp, #248]\n"
    "    ldr x23, [sp, #264]\n"
    "    ldp x24, x25, [sp, #272]\n"
    "    msr sp_el0, x21\n"
    "    msr elr_el1, x22\n"
    "    msr spsr_el1, x23\n"
    "    msr fpsr, x24\n"
    "    msr fpcr, x25\n"
    "    ldp q0, q1, [sp, #288]\n"
    "    ldp q2, q3, [sp, #320]\n"
    "    ldp q4, q5, [sp, #352]\n"
    "    ldp q6, q7, [sp, #384]\n"
    "    ldp q8, q9, [sp, #416]\n"
    "    ldp q10, q11, [sp, #448]\n"
    "    ldp q12, q13, [sp, #480]\n"
    "    ldp q14, q15, [sp, #512]\n"
    "    ldp q16, q17, [sp, #544]\n"
    "    ldp q18, q19, [sp, #576]\n"
    "    ldp q20, q21, [sp, #608]\n"
    "    ldp q22, q23, [sp, #640]\n"
    "    ldp q24, q25, [sp, #672]\n"
    "    ldp q26, q27, [sp, #704]\n"
    "    ldp q28, q29, [sp, #736]\n"
    "    ldp q30, q31, [sp, #768]\n"
    "    ldp x2, x3, [sp, #16]\n"
    "    ldp x4, x5, [sp, #32]\n"
    "    ldp x6, x7, [sp, #48]\n"
    "    ldp x8, x9, [sp, #64]\n"
    "    ldp x10, x11, [sp, #80]\n"
    "    ldp x12, x13, [sp, #96]\n"
    "    ldp x14, x15, [sp, #112]\n"
    "    ldp x16, x17, [sp, #128]\n"
    "    ldp x18, x19, [sp, #144]\n"
    "    ldp x20, x21, [sp, #160]\n"
    "    ldp x22, x23, [sp, #176]\n"
    "    ldp x24, x25, [sp, #192]\n"
    "    ldp x26, x27, [sp, #208]\n"
    "    ldp x28, x29, [sp, #224]\n"
    "    ldr x30, [sp, #240]\n"
    "    ldp x0, x1, [sp, #0]\n"
    "    add sp, sp, #800\n"
    "    eret\n"
    /* tn_enter_user(entry, sp): start an app in user mode, at `entry` with stack
     * `sp`, interrupts on and every other register zero (so nothing of the kernel's
     * leaks into it). */
    ".global tn_enter_user\n"
    "tn_enter_user:\n"
    "    sub sp, sp, #800\n"
    "    mov x2, sp\n"
    "    mov x3, #100\n"
    "1:  str xzr, [x2], #8\n"
    "    subs x3, x3, #1\n"
    "    b.ne 1b\n"
    "    str x1, [sp, #248]\n"
    "    str x0, [sp, #256]\n"
    "    b tn_user_return\n"
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
 * After the kernel image come 8 stack slots of 12 MB (slot 0: boot; slots 1-7: apps).
 * Each is a 2 MB unmapped guard, an 8 MB stack the app uses in user mode, and a 2 MB
 * stack the kernel uses while it works for the app. Then come the heaps, one region
 * of whole 2 MB blocks per app, up to the end of the 256 MB of RAM. */

#define TN_MAX_APPS 7
#define TN_SLOT 0xc00000ULL
#define TN_GUARD 0x200000ULL
#define TN_USER_STACK 0x800000ULL
extern char __stacks_start[], __heap_start[], __heap_end[];

static uint64_t tn_slot(int i) { return (uint64_t)__stacks_start + (uint64_t)i * TN_SLOT; }

/* ---- tasks ---- */

enum { TN_READY, TN_SLEEPING, TN_WAITING_INPUT, TN_DONE };

typedef struct {
    const tn_app *app;
    int state;
    int exit_code;
    uint64_t wake_at;   /* TN_SLEEPING: counter value to wake at */
    uint64_t cpu_ticks; /* timer ticks it was running for */
    tn_ctx ctx;
    uint64_t heap, heap_end; /* its heap region */
} tn_task;

static tn_task tn_tasks[TN_MAX_APPS];
static int tn_ntasks;
static tn_task *tn_cur; /* NULL while booting */
static tn_ctx tn_boot_ctx;

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

static void tn_use_tables(tn_task *t);

static void tn_run(tn_task *next) {
    tn_task *prev = tn_cur;
    tn_cur = next;
    if (prev != next) {
        tn_use_tables(next);
        tn_switch(prev ? &prev->ctx : &tn_boot_ctx, &next->ctx);
    }
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

/* Where a new task starts (tn_switch "returns" here on its fresh kernel stack). */
static void tn_task_start(void) {
    int i = (int)(tn_cur - tn_tasks);
    tn_enter_user(tn_cur->app->entry, tn_slot(i + 1) + TN_GUARD + TN_USER_STACK);
}

/* ---- system calls: what the kernel offers apps ----
 * Every call that has an effect first checks that the app was granted it, and every
 * pointer an app passes must point into its own memory. An app that asks for more
 * is stopped, and the other apps keep running. */

#define TN_EXIT_DENIED 77

__attribute__((noreturn)) static void tn_exit(int code);

static void tn_need(unsigned perm, const char *effect, const char *what) {
    if (tn_cur->app->perms & perm) return;
    char allowed[32];
    tn_perm_names(tn_cur->app->perms, allowed, sizeof allowed);
    tn_outf("[Glyph OS: denied: app %s tried to %s, which needs `%s`, but it is only allowed %s]\n",
            tn_cur->app->name, what, effect, allowed);
    tn_exit(TN_EXIT_DENIED);
}

static bool tn_within(uint64_t p, uint64_t n, uint64_t lo, uint64_t hi) {
    return p >= lo && p <= hi && n <= hi - p;
}

/* Whether [p, p + n) lies in memory the current app owns. */
static bool tn_owns(uint64_t p, uint64_t n) {
    const tn_app *a = tn_cur->app;
    uint64_t stack = tn_slot((int)(tn_cur - tn_tasks) + 1) + TN_GUARD;
    return tn_within(p, n, (uint64_t)a->start, (uint64_t)a->end) ||
           tn_within(p, n, stack, stack + TN_USER_STACK) ||
           tn_within(p, n, tn_cur->heap, tn_cur->heap_end);
}

static void tn_need_own(uint64_t p, uint64_t n) {
    if (tn_owns(p, n)) return;
    tn_outf("[Glyph OS: denied: app %s asked the kernel to read memory that isn't its own (address %llx)]\n",
            tn_cur->app->name, (unsigned long long)p);
    tn_exit(TN_EXIT_DENIED);
}

static void tn_sleep_ms(int64_t ms) {
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
        tn_cur->state = TN_WAITING_INPUT;
        tn_block_current();
        tn_irqs_on();
    }
}

static void tn_syscall(tn_frame *f) {
    uint64_t a = f->x[0], b = f->x[1];
    switch (f->x[8]) {
    case TN_SYS_EXIT:
        tn_exit((int)a);
        break;
    case TN_SYS_OUT:
        tn_need(TN_PERM_IO, "io", "print");
        tn_need_own(a, b);
        tn_write((const char *)a, b);
        break;
    case TN_SYS_ERR: /* error reports (failed contracts and the like) are always allowed */
        tn_need_own(a, b);
        tn_write((const char *)a, b);
        break;
    case TN_SYS_GETC:
        tn_need(TN_PERM_IO, "io", "read input");
        f->x[0] = (uint64_t)tn_uart_getc();
        break;
    case TN_SYS_TICKS:
        tn_need(TN_PERM_TIME, "time", "read the clock");
        f->x[0] = tn_counter();
        break;
    case TN_SYS_TICK_FREQ:
        tn_need(TN_PERM_TIME, "time", "read the clock");
        f->x[0] = tn_counter_freq();
        break;
    case TN_SYS_SLEEP:
        tn_need(TN_PERM_TIME, "time", "sleep");
        tn_sleep_ms((int64_t)a);
        break;
    case TN_SYS_RANDOM_SEED:
        tn_need(TN_PERM_RAND, "rand", "get random numbers");
        f->x[0] = tn_counter() ^ 0x9e3779b97f4a7c15ULL;
        break;
    case TN_SYS_HEAP:
        f->x[0] = tn_cur->heap;
        f->x[1] = tn_cur->heap_end;
        break;
    default:
        tn_outf("[Glyph OS: denied: app %s made an unknown system call (%llu)]\n", tn_cur->app->name,
                (unsigned long long)f->x[8]);
        tn_exit(TN_EXIT_DENIED);
    }
}

/* Ends the current app. Before any app runs (the boot self-tests), stops the machine. */
__attribute__((noreturn)) static void tn_exit(int code) {
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

/* ---- CPU exceptions ----
 * In an app: explain what went wrong and stop that app; the others keep running.
 * In the kernel: explain what went wrong, then stop the machine. */

static void tn_user_fault(uint64_t esr, uint64_t elr, uint64_t far) {
    uint64_t ec = esr >> 26;
    bool data = ec == 0x24, code = ec == 0x20;
    bool write = data && (esr & (1 << 6));
    uint64_t guard = tn_slot((int)(tn_cur - tn_tasks) + 1);
    const char *what;
    if (!data && !code) {
        what = ec == 0x00 || ec == 0x18 ? "used an instruction only the kernel may use" : "CPU exception";
        tn_outf("[Glyph OS: app %s crashed: %s, at instruction %llx (ESR %llx)]\n", tn_cur->app->name, what,
                (unsigned long long)elr, (unsigned long long)esr);
        tn_exit(70);
    }
    if (far >= guard && far < guard + TN_GUARD)
        what = "stack overflow";
    else if (far < 0x10000)
        what = "null pointer access";
    else if (!tn_owns(far, 1))
        what = "touched memory that isn't its own (the kernel's, a device's or another app's)";
    else if (write)
        what = "write to read-only memory";
    else
        what = "tried to run memory that isn't code";
    tn_outf("[Glyph OS: app %s crashed: %s, at address %llx, instruction %llx (ESR %llx)]\n",
            tn_cur->app->name, what, (unsigned long long)far, (unsigned long long)elr,
            (unsigned long long)esr);
    tn_exit(70);
}

void tn_kernel_irq(void);

/* Every exception from user mode: kind is the vector entry (8: synchronous, such as
 * a system call or a fault; 9: an interrupt). */
void tn_user_exception(uint64_t kind, tn_frame *f) {
    uint64_t esr, far;
    __asm__ volatile("mrs %0, esr_el1\nmrs %1, far_el1" : "=r"(esr), "=r"(far));
    if (kind == 9)
        tn_kernel_irq();
    else if (kind == 8 && esr >> 26 == 0x15)
        tn_syscall(f);
    else
        tn_user_fault(esr, f->elr, far);
}

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
 *   kernel image             4 KB pages: kernel code RX, constants R, data RW; the
 *                            shared C library; then each app's code, constants, data
 *   stack slots              per slot: 2 MB unmapped guard, 8 MB app stack, 2 MB
 *                            kernel stack
 *   heaps                    RW, never executable, up to the end of RAM
 * The kernel's tables give user mode nothing but the shared C library. Each app's
 * tables are the same, except that its own code, data, stack and heap are open to
 * user mode too; the kernel switches tables when it switches apps. */

#define TN_PAGE 0x1000ULL
#define TN_BLOCK_2M 0x200000ULL
#define TN_RAM 0x40000000ULL
#define TN_L3_TABLES 8 /* page-granular mapping for the first 16 MB of RAM */

#define TN_D_BLOCK 1ULL        /* level 1/2: maps a whole block */
#define TN_D_TABLE 3ULL        /* level 1/2: points to the next table */
#define TN_D_PAGE 3ULL         /* level 3: maps one page */
#define TN_D_AF (1ULL << 10)   /* accessed flag (otherwise the first access faults) */
#define TN_D_INNER (3ULL << 8) /* inner shareable */
#define TN_D_USER (1ULL << 6)  /* accessible in user mode too */
#define TN_D_RO (2ULL << 6)    /* read-only */
#define TN_D_PXN (1ULL << 53)  /* never executable by the kernel */
#define TN_D_UXN (1ULL << 54)  /* never executable by user code */
#define TN_D_XN (TN_D_PXN | TN_D_UXN)
#define TN_D_NORMAL ((1ULL << 2) | TN_D_INNER | TN_D_AF) /* MAIR attribute 1 */
#define TN_D_DEVICE ((0ULL << 2) | TN_D_AF | TN_D_XN)   /* attribute 0 */

typedef struct {
    uint64_t l1[512], l2_ram[512], l3[TN_L3_TABLES][512];
} tn_tables;

static uint64_t tn_l2_low[512] __attribute__((aligned(4096))); /* devices: the same for all */
static tn_tables tn_kernel_tables __attribute__((aligned(4096)));
static tn_tables tn_app_tables[TN_MAX_APPS] __attribute__((aligned(4096)));
static bool tn_mmu_enabled;

extern char __text_start[], __text_end[], __rodata_end[], __image_end[];
extern char __shared_start[], __shared_text_end[], __shared_end[];

/* How page `a` of the image is mapped in the tables of app `owner` (-1: the kernel's). */
static uint64_t tn_image_page(uint64_t a, int owner) {
    if (a < (uint64_t)__text_end) return TN_D_NORMAL | TN_D_RO | TN_D_UXN;
    if (a < (uint64_t)__rodata_end) return TN_D_NORMAL | TN_D_RO | TN_D_XN;
    if (a < (uint64_t)__shared_start) return TN_D_NORMAL | TN_D_XN;
    if (a < (uint64_t)__shared_text_end) return TN_D_NORMAL | TN_D_RO | TN_D_USER;
    if (a < (uint64_t)__shared_end) return TN_D_NORMAL | TN_D_RO | TN_D_USER | TN_D_XN;
    for (int i = 0; i < tn_app_count && i < TN_MAX_APPS; i++) {
        const tn_app *app = &tn_apps[i];
        if (a < (uint64_t)app->start || a >= (uint64_t)app->end) continue;
        uint64_t user = i == owner ? TN_D_USER : 0;
        if (a < (uint64_t)app->text_end) return TN_D_NORMAL | TN_D_RO | user | TN_D_PXN | (user ? 0 : TN_D_UXN);
        if (a < (uint64_t)app->ro_end) return TN_D_NORMAL | TN_D_RO | user | TN_D_XN;
        return TN_D_NORMAL | user | TN_D_XN;
    }
    return TN_D_NORMAL | TN_D_XN;
}

static void tn_build_tables(tn_tables *t, int owner) {
    uint64_t text = (uint64_t)__text_start, image_end = (uint64_t)__image_end;
    uint64_t tables = (image_end - TN_RAM) / TN_BLOCK_2M;
    t->l1[0] = (uint64_t)tn_l2_low | TN_D_TABLE;
    for (uint64_t a = text; a < image_end; a += TN_PAGE) {
        uint64_t i = (a - TN_RAM) / TN_PAGE;
        t->l3[i / 512][i % 512] = a | tn_image_page(a, owner) | TN_D_PAGE;
    }
    for (uint64_t i = 0; i < tables; i++) t->l2_ram[i] = (uint64_t)t->l3[i] | TN_D_TABLE;
    uint64_t data = TN_D_NORMAL | TN_D_XN | TN_D_BLOCK;
    for (int s = 0; s < 8; s++) {
        uint64_t slot = tn_slot(s), user = s == owner + 1 ? TN_D_USER : 0;
        for (uint64_t a = slot + TN_GUARD; a < slot + TN_SLOT; a += TN_BLOCK_2M)
            t->l2_ram[(a - TN_RAM) / TN_BLOCK_2M] = a | data | (a < slot + TN_GUARD + TN_USER_STACK ? user : 0);
    }
    for (uint64_t a = (uint64_t)__heap_start; a < (uint64_t)__heap_end; a += TN_BLOCK_2M) {
        bool own = owner >= 0 && a >= tn_tasks[owner].heap && a < tn_tasks[owner].heap_end;
        t->l2_ram[(a - TN_RAM) / TN_BLOCK_2M] = a | data | (own ? TN_D_USER : 0);
    }
    t->l1[1] = (uint64_t)t->l2_ram | TN_D_TABLE;
}

static void tn_set_ttbr(tn_tables *t) {
    __asm__ volatile("dsb ishst\n" /* table writes are visible to the table walker */
                     "msr ttbr0_el1, %0\n"
                     "isb\n"
                     "tlbi vmalle1\n"
                     "dsb ish\n"
                     "isb\n"
                     :
                     : "r"((uint64_t)t->l1)
                     : "memory");
}

static void tn_use_tables(tn_task *t) {
    if (tn_mmu_enabled) tn_set_ttbr(&tn_app_tables[t - tn_tasks]);
}

static void tn_mmu_on(void) {
    if (((uint64_t)__image_end - TN_RAM) / TN_BLOCK_2M > TN_L3_TABLES) {
        tn_write("kernel panic: the kernel image is too large to map\n", 51);
        tn_halt(72);
    }
    for (uint64_t a = 0x08000000; a < 0x10000000; a += TN_BLOCK_2M)
        tn_l2_low[a / TN_BLOCK_2M] = a | TN_D_DEVICE | TN_D_BLOCK;
    tn_build_tables(&tn_kernel_tables, -1);

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
                     "dsb ish\n"
                     "isb\n"
                     :
                     : "r"(mair), "r"(tcr)
                     : "memory");
    tn_set_ttbr(&tn_kernel_tables);
    uint64_t sctlr;
    __asm__ volatile("mrs %0, sctlr_el1" : "=r"(sctlr));
    sctlr |= (1 << 0)    /* MMU on */
             | (1 << 2)  /* data cache on */
             | (1 << 12) /* instruction cache on */
             | (1 << 19); /* writable memory is never executable */
    sctlr &= ~(uint64_t)((1 << 1)    /* no alignment checking */
                         | (1 << 9)  /* user mode can't mask interrupts */
                         | (1 << 26)); /* nor maintain caches */
    __asm__ volatile("msr sctlr_el1, %0\nisb\nic iallu\ndsb ish\nisb" : : "r"(sctlr) : "memory");
    tn_mmu_enabled = true;
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
    /* User mode may not read the timer or the counter itself. */
    __asm__ volatile("msr cntkctl_el1, xzr\nisb");
    tn_interrupts_on();
#ifdef TN_SELFTEST
    tn_selftest(TN_SELFTEST);
#endif
    tn_irqs_off();
    tn_ntasks = tn_app_count < TN_MAX_APPS ? tn_app_count : TN_MAX_APPS;
    uint64_t heap = (uint64_t)__heap_start;
    uint64_t share = (((uint64_t)__heap_end - heap) / (uint64_t)tn_ntasks) & ~(TN_BLOCK_2M - 1);
    for (int i = 0; i < tn_ntasks; i++) {
        tn_task *t = &tn_tasks[i];
        t->app = &tn_apps[i];
        t->state = TN_READY;
        t->heap = heap + (uint64_t)i * share;
        t->heap_end = t->heap + share;
        /* Slot i + 1: the kernel stack is at the top of the slot. */
        t->ctx.r[11] = (uint64_t)&tn_task_start; /* x30: where tn_switch returns to */
        t->ctx.r[12] = tn_slot(i + 1) + TN_SLOT; /* sp */
    }
    for (int i = 0; i < tn_ntasks; i++) tn_build_tables(&tn_app_tables[i], i);
    tn_run(&tn_tasks[0]);
}

/* ---- end of kernel ---- */
