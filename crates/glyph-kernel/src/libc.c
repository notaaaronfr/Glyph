/* ---- the small part of the C library the kernel and the runtime use ----
 * Stateless, so one copy serves the kernel and every app: its pages are read-only
 * and executable in user mode and in the kernel. */

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

/* ---- end of the shared C library ---- */
