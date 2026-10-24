/* ---- Glyph kernel API: what an app sees of the kernel ----
 * Every app is compiled as its own C file: the ABI, this header, then the runtime,
 * then the app's code. Apps run in user mode, in their own address space: they can
 * touch only their own code, data, stack and heap, and reach everything else through
 * the system calls below. The kernel checks each call against the effects the app's
 * `main` declares (the comments say which), and every pointer it is given. */

#define TN_KERNEL 1

static inline uint64_t tn_svc(uint64_t n, uint64_t a, uint64_t b, uint64_t *second) {
    register uint64_t x0 __asm__("x0") = a;
    register uint64_t x1 __asm__("x1") = b;
    register uint64_t x8 __asm__("x8") = n;
    __asm__ volatile("svc #0" : "+r"(x0), "+r"(x1) : "r"(x8) : "memory");
    if (second) *second = x1;
    return x0;
}

/* Serial port. Each call's output comes out whole, even if apps switch meanwhile. */
static void tn_out(const char *s, size_t n) { tn_svc(TN_SYS_OUT, (uint64_t)s, n, NULL); } /* io */
/* Error reports: always allowed. */
static void tn_err(const char *s, size_t n) { tn_svc(TN_SYS_ERR, (uint64_t)s, n, NULL); }
static void tn_flush(void) {}
/* Waits (without using the CPU) for a typed character. */
static int tn_uart_getc(void) { return (int)tn_svc(TN_SYS_GETC, 0, 0, NULL); } /* io */

static const char *tn_getenv(const char *name) {
    (void)name;
    return NULL;
}

/* The CPU's generic timer. */
static uint64_t tn_ticks(void) { return tn_svc(TN_SYS_TICKS, 0, 0, NULL); }         /* time */
static uint64_t tn_tick_freq(void) { return tn_svc(TN_SYS_TICK_FREQ, 0, 0, NULL); } /* time */
/* Puts the app to sleep; other apps run meanwhile. */
static void tn_sleep_ms(int64_t ms) { tn_svc(TN_SYS_SLEEP, (uint64_t)ms, 0, NULL); } /* time */

/* A seed for the app's random numbers. */
static uint64_t tn_random_seed(void) { return tn_svc(TN_SYS_RANDOM_SEED, 0, 0, NULL); } /* rand */

/* Ends the app (not the machine) with an exit code. */
__attribute__((noreturn)) static void tn_exit(int code) {
    tn_svc(TN_SYS_EXIT, (uint64_t)code, 0, NULL);
    __builtin_unreachable();
}

/* ---- memory allocation, from the app's own heap ----
 * Blocks carry a 16-byte header with their usable size. Freed blocks go on a free
 * list per size class (multiples of 16 bytes up to 1 KB) or on one list for larger
 * blocks; new memory comes from the heap region the kernel gave the app. Reference
 * counting frees most blocks soon after use, so they are reused quickly. */

typedef struct tn_block { size_t size; struct tn_block *next; } tn_block;
#define TN_SMALL_CLASSES 64

static struct {
    tn_block *small[TN_SMALL_CLASSES + 1];
    tn_block *large;
    char *top, *end;
} tn_heap;

#define malloc tn_malloc
#define free tn_free
#define realloc tn_realloc

static void *tn_malloc(size_t n) {
    if (!tn_heap.top) {
        uint64_t end;
        tn_heap.top = (char *)tn_svc(TN_SYS_HEAP, 0, 0, &end);
        tn_heap.end = (char *)end;
    }
    size_t size = (n + 15) & ~(size_t)15;
    if (size == 0) size = 16;
    tn_block *b = NULL;
    if (size <= TN_SMALL_CLASSES * 16) {
        tn_block **list = &tn_heap.small[size / 16];
        if (*list) {
            b = *list;
            *list = b->next;
        }
    } else {
        for (tn_block **p = &tn_heap.large; *p; p = &(*p)->next) {
            if ((*p)->size >= size) {
                b = *p;
                *p = b->next;
                break;
            }
        }
    }
    if (!b) {
        if ((size_t)(tn_heap.end - tn_heap.top) < size + sizeof(tn_block)) {
            static const char msg[] = "out of memory (each app has its own heap)\n";
            tn_err(msg, sizeof msg - 1);
            tn_exit(71);
        }
        b = (tn_block *)tn_heap.top;
        b->size = size;
        tn_heap.top += size + sizeof(tn_block);
    }
    return b + 1;
}

static void tn_free(void *p) {
    if (!p) return;
    tn_block *b = (tn_block *)p - 1;
    if (b->size <= TN_SMALL_CLASSES * 16) {
        b->next = tn_heap.small[b->size / 16];
        tn_heap.small[b->size / 16] = b;
    } else {
        b->next = tn_heap.large;
        tn_heap.large = b;
    }
}

static void *tn_realloc(void *p, size_t n) {
    if (!p) return tn_malloc(n);
    tn_block *b = (tn_block *)p - 1;
    if (n <= b->size) return p;
    void *q = tn_malloc(n);
    memcpy(q, p, b->size);
    tn_free(p);
    return q;
}

#ifdef TN_USERTEST
/* Deliberate attacks from user mode, to test that the kernel and the MMU stop them
 * (GLYPH_KERNEL_CFLAGS=-DTN_USERTEST=n). Only the first app attacks. */
extern char __text_start[];
static void tn_usertest(int which) {
    if (TN_APP_INDEX != 0) return;
    uint64_t heap_end;
    switch (which) {
    case 1: *(volatile uint32_t *)0x09000000 = 'X'; break;                 /* the serial port */
    case 2: (void)*(volatile char *)__text_start; break;                   /* kernel memory */
    case 3: { uint64_t t; __asm__ volatile("mrs %0, cntpct_el0" : "=r"(t)); break; } /* the clock */
    case 4: __asm__ volatile("msr daifset, #2"); break;                    /* mask interrupts */
    case 5: tn_svc(TN_SYS_OUT, (uint64_t)__text_start, 16, NULL); break;   /* kernel prints for us */
    case 6:                                                                /* the next app's heap */
        tn_svc(TN_SYS_HEAP, 0, 0, &heap_end);
        (void)*(volatile char *)heap_end;
        break;
    case 7: *(volatile uint32_t *)(void *)&tn_usertest = 0; break;         /* our own code */
    }
}
#endif

/* ---- end of kernel API ---- */
