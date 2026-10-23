/* ---- Glyph native runtime (embedded into every compiled program) ----
 * Expects TN_FILE_NAME and TN_SOURCE to be defined first, for error messages.
 * Memory is never freed in v0.1: programs are short-lived. */

#include <errno.h>
#include <stdarg.h>
#include <stdbool.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/time.h>
#include <time.h>

#define TN_MAX_DEPTH 10000

typedef uint8_t tn_unit;
typedef struct { int32_t line, col; } tn_loc;
#define TN_LOC(l, c) ((tn_loc){(l), (c)})

/* Strings are immutable UTF-8 with a cached character count; data is NUL-terminated. */
typedef struct { int64_t len; int64_t nchars; char data[]; } tn_str;

/* Every Glyph value fits in 8 bytes: Int, Bool, Unit, or a pointer to a Str / List. */
typedef union { int64_t i; bool b; void *p; } tn_val;
#define TN_I(x) ((tn_val){.i = (x)})
#define TN_B(x) ((tn_val){.b = (x)})
#define TN_P(x) ((tn_val){.p = (void *)(x)})

/* Lists have value semantics. Several lists may share one buffer: each sees only its
 * first `len` items, and items are never overwritten, so the list that ends at the
 * buffer's tip can be extended in place. That makes `xs = push(xs, v)` O(1). */
typedef struct { int64_t cap, used; tn_val items[]; } tn_buf;
typedef struct { tn_buf *buf; int64_t len; } tn_list;

/* Records are immutable arrays of fields in declaration order: `p.x = v` copies. */
typedef struct { int64_t n; tn_val f[]; } tn_rec;

/* Enum values: which variant (`tag`, its index in the declaration) and its fields. */
typedef struct { int64_t tag; tn_val f[]; } tn_enum;

struct tn_type;
typedef struct {
    const char *name;
    int64_t nfields;
    const char *const *fnames;
    const struct tn_type *const *ftypes;
} tn_variant;

enum { TN_INT, TN_STR, TN_BOOL, TN_UNIT, TN_LIST, TN_RECORD, TN_ENUM };
typedef struct tn_type {
    int kind;
    const struct tn_type *elem;               /* lists */
    const char *name;                         /* records and enums */
    int64_t nfields;                          /* records: fields; enums: variants */
    const char *const *fnames;
    const struct tn_type *const *ftypes;
    const tn_variant *variants;               /* enums */
} tn_type;
static const tn_type TY_i = {TN_INT, 0}, TY_s = {TN_STR, 0}, TY_b = {TN_BOOL, 0}, TY_u = {TN_UNIT, 0};

static int64_t tn_depth = 0;
static uint64_t tn_rng = 1;

static void *tn_alloc(size_t n) {
    void *p = malloc(n ? n : 1);
    if (!p) {
        fputs("out of memory\n", stderr);
        exit(1);
    }
    return p;
}

static const char *tn_fmt(const char *fmt, ...) {
    va_list ap;
    va_start(ap, fmt);
    int n = vsnprintf(NULL, 0, fmt, ap);
    va_end(ap);
    char *s = tn_alloc(n + 1);
    va_start(ap, fmt);
    vsnprintf(s, n + 1, fmt, ap);
    va_end(ap);
    return s;
}

/* Same format as the interpreter: location, message, source line, caret, hint. */
__attribute__((noreturn)) static void tn_fail(const char *kind, tn_loc at, const char *msg, const char *hint) {
    fflush(stdout);
    fprintf(stderr, "%s:%d:%d: %s error: %s\n", TN_FILE_NAME, at.line, at.col, kind, msg);
    const char *p = TN_SOURCE;
    int line = 1;
    while (*p && line < at.line) {
        if (*p == '\n') line++;
        p++;
    }
    if (line == at.line && *p) {
        const char *end = strchr(p, '\n');
        int len = end ? (int)(end - p) : (int)strlen(p);
        if (len > 0 && p[len - 1] == '\r') len--;
        char num[32];
        int w = snprintf(num, sizeof num, "%d", at.line);
        fprintf(stderr, "%s | %.*s\n", num, len, p);
        fprintf(stderr, "%*s | %*s^\n", w, "", at.col - 1, "");
    }
    if (hint) fprintf(stderr, "  hint: %s\n", hint);
    exit(1);
}

__attribute__((noreturn)) static void tn_fail_depth(tn_loc at, const char *fn) {
    tn_fail("runtime", at, tn_fmt("recursion deeper than %d calls in `%s`", TN_MAX_DEPTH, fn),
            "check that the recursion has a base case that is always reached");
}

/* ---- checked integer arithmetic ---- */

__attribute__((noreturn)) static void tn_overflow(tn_loc at, const char *text) {
    tn_fail("runtime", at, tn_fmt("integer overflow in `%s`", text), NULL);
}

static inline int64_t tn_add(int64_t a, int64_t b, tn_loc at, const char *t) {
    int64_t r;
    if (__builtin_add_overflow(a, b, &r)) tn_overflow(at, t);
    return r;
}
static inline int64_t tn_sub(int64_t a, int64_t b, tn_loc at, const char *t) {
    int64_t r;
    if (__builtin_sub_overflow(a, b, &r)) tn_overflow(at, t);
    return r;
}
static inline int64_t tn_mul(int64_t a, int64_t b, tn_loc at, const char *t) {
    int64_t r;
    if (__builtin_mul_overflow(a, b, &r)) tn_overflow(at, t);
    return r;
}
static inline void tn_check_div(int64_t a, int64_t b, tn_loc at, const char *t) {
    if (b == 0)
        tn_fail("runtime", at, tn_fmt("division by zero in `%s`", t),
                "guard it with `requires` on the divisor, e.g. `requires b != 0`");
    if (a == INT64_MIN && b == -1) tn_overflow(at, t);
}
static inline int64_t tn_div(int64_t a, int64_t b, tn_loc at, const char *t) {
    tn_check_div(a, b, at, t);
    return a / b;
}
static inline int64_t tn_mod(int64_t a, int64_t b, tn_loc at, const char *t) {
    tn_check_div(a, b, at, t);
    return a % b;
}
static inline int64_t tn_neg(int64_t a, tn_loc at, const char *t) {
    if (a == INT64_MIN) tn_overflow(at, t);
    return -a;
}
static inline int64_t tn_abs(int64_t a, tn_loc at) {
    if (a == INT64_MIN) tn_overflow(at, "abs");
    return a < 0 ? -a : a;
}

/* ---- strings ---- */

static int64_t tn_utf8_count(const char *s, int64_t n) {
    int64_t c = 0;
    for (int64_t i = 0; i < n; i++)
        if (((unsigned char)s[i] & 0xC0) != 0x80) c++;
    return c;
}

static int64_t tn_utf8_width(unsigned char c) { return c < 0x80 ? 1 : c >= 0xF0 ? 4 : c >= 0xE0 ? 3 : 2; }

static tn_str *tn_str_raw(int64_t n) {
    tn_str *s = tn_alloc(sizeof(tn_str) + n + 1);
    s->len = n;
    s->data[n] = 0;
    return s;
}

static tn_str *tn_str_from(const char *p, int64_t n) {
    tn_str *s = tn_str_raw(n);
    memcpy(s->data, p, n);
    s->nchars = tn_utf8_count(p, n);
    return s;
}

static tn_str *tn_str_cat(tn_str *a, tn_str *b) {
    tn_str *s = tn_str_raw(a->len + b->len);
    memcpy(s->data, a->data, a->len);
    memcpy(s->data + a->len, b->data, b->len);
    s->nchars = a->nchars + b->nchars;
    return s;
}

static inline bool tn_str_eq(tn_str *a, tn_str *b) {
    return a->len == b->len && memcmp(a->data, b->data, a->len) == 0;
}

static int tn_str_cmp(tn_str *a, tn_str *b) {
    int64_t n = a->len < b->len ? a->len : b->len;
    int c = memcmp(a->data, b->data, n);
    if (c) return c;
    return (a->len > b->len) - (a->len < b->len);
}

static inline int64_t tn_str_len(tn_str *s) { return s->nchars; }

static int64_t tn_find(tn_str *s, tn_str *sub, int64_t from) {
    for (int64_t i = from; i + sub->len <= s->len; i++)
        if (memcmp(s->data + i, sub->data, sub->len) == 0) return i;
    return -1;
}

static bool tn_str_contains(tn_str *s, tn_str *sub) { return tn_find(s, sub, 0) >= 0; }

static tn_str *tn_str_at(tn_str *s, int64_t i, tn_loc at) {
    if (i < 0 || i >= s->nchars)
        tn_fail("runtime", at, tn_fmt("index %lld out of bounds (length %lld)", (long long)i, (long long)s->nchars), NULL);
    if (s->nchars == s->len) return tn_str_from(s->data + i, 1);
    int64_t off = 0;
    for (int64_t k = 0; k < i; k++) off += tn_utf8_width(s->data[off]);
    return tn_str_from(s->data + off, tn_utf8_width(s->data[off]));
}

/* ---- lists ---- */

static tn_list *tn_list_new(int64_t cap) {
    if (cap < 4) cap = 4;
    tn_buf *b = tn_alloc(sizeof(tn_buf) + sizeof(tn_val) * cap);
    b->cap = cap;
    b->used = 0;
    tn_list *l = tn_alloc(sizeof(tn_list));
    l->buf = b;
    l->len = 0;
    return l;
}

static tn_list *tn_list_from(int64_t n, const tn_val *items) {
    tn_list *l = tn_list_new(n);
    memcpy(l->buf->items, items, n * sizeof(tn_val));
    l->buf->used = l->len = n;
    return l;
}

static tn_list *tn_list_push(tn_list *l, tn_val v) {
    tn_buf *b = l->buf;
    tn_list *r = tn_alloc(sizeof(tn_list));
    r->len = l->len + 1;
    if (l->len == b->used && b->used < b->cap) {
        b->items[b->used++] = v;
        r->buf = b;
        return r;
    }
    int64_t cap = l->len * 2 + 4;
    tn_buf *nb = tn_alloc(sizeof(tn_buf) + sizeof(tn_val) * cap);
    nb->cap = cap;
    memcpy(nb->items, b->items, l->len * sizeof(tn_val));
    nb->items[l->len] = v;
    nb->used = r->len;
    r->buf = nb;
    return r;
}

static tn_list *tn_list_cat(tn_list *a, tn_list *b) {
    tn_list *l = tn_list_new(a->len + b->len);
    memcpy(l->buf->items, a->buf->items, a->len * sizeof(tn_val));
    memcpy(l->buf->items + a->len, b->buf->items, b->len * sizeof(tn_val));
    l->buf->used = l->len = a->len + b->len;
    return l;
}

static inline tn_val tn_list_item(tn_list *l, int64_t i) { return l->buf->items[i]; }

static inline tn_val tn_list_get(tn_list *l, int64_t i, tn_loc at) {
    if (i < 0 || i >= l->len)
        tn_fail("runtime", at, tn_fmt("index %lld out of bounds (length %lld)", (long long)i, (long long)l->len), NULL);
    return l->buf->items[i];
}

static tn_list *tn_range(int64_t a, int64_t b) {
    int64_t n = b > a ? b - a : 0;
    tn_list *l = tn_list_new(n);
    for (int64_t i = 0; i < n; i++) l->buf->items[i] = TN_I(a + i);
    l->buf->used = l->len = n;
    return l;
}

static tn_list *tn_str_chars(tn_str *s) {
    tn_list *l = tn_list_new(s->nchars);
    for (int64_t off = 0; off < s->len;) {
        int64_t w = tn_utf8_width(s->data[off]);
        l->buf->items[l->len++] = TN_P(tn_str_from(s->data + off, w));
        off += w;
    }
    l->buf->used = l->len;
    return l;
}

static tn_list *tn_split(tn_str *s, tn_str *sep, tn_loc at) {
    if (sep->len == 0) tn_fail("runtime", at, "invalid arguments to `split`: the separator must not be empty", NULL);
    tn_list *l = tn_list_new(4);
    int64_t start = 0, pos;
    while ((pos = tn_find(s, sep, start)) >= 0) {
        l = tn_list_push(l, TN_P(tn_str_from(s->data + start, pos - start)));
        start = pos + sep->len;
    }
    return tn_list_push(l, TN_P(tn_str_from(s->data + start, s->len - start)));
}

/* ---- records ---- */

static tn_rec *tn_rec_new(int64_t n, const tn_val *fields) {
    tn_rec *r = tn_alloc(sizeof(tn_rec) + sizeof(tn_val) * n);
    r->n = n;
    if (n) memcpy(r->f, fields, sizeof(tn_val) * n);
    return r;
}

static tn_rec *tn_rec_set(tn_rec *r, int64_t i, tn_val v) {
    tn_rec *c = tn_rec_new(r->n, r->f);
    c->f[i] = v;
    return c;
}

static tn_enum *tn_enum_new(int64_t tag, int64_t n, const tn_val *fields) {
    tn_enum *e = tn_alloc(sizeof(tn_enum) + sizeof(tn_val) * n);
    e->tag = tag;
    if (n) memcpy(e->f, fields, sizeof(tn_val) * n);
    return e;
}

/* ---- generic operations, driven by type descriptors ---- */

typedef struct { char *p; int64_t len, cap; } tn_sb;

static void tn_sb_put(tn_sb *b, const char *s, int64_t n) {
    if (b->len + n + 1 > b->cap) {
        b->cap = (b->len + n + 1) * 2;
        b->p = realloc(b->p, b->cap);
        if (!b->p) tn_alloc((size_t)-1);
    }
    memcpy(b->p + b->len, s, n);
    b->len += n;
}

static void tn_show(tn_sb *b, tn_val v, const tn_type *t, bool quote);

/* " { a: 1, b: \"x\" }" */
static void tn_show_fields(tn_sb *b, const tn_val *f, int64_t n, const char *const *names,
                           const struct tn_type *const *types) {
    tn_sb_put(b, " { ", 3);
    for (int64_t i = 0; i < n; i++) {
        if (i) tn_sb_put(b, ", ", 2);
        tn_sb_put(b, names[i], strlen(names[i]));
        tn_sb_put(b, ": ", 2);
        tn_show(b, f[i], types[i], true);
    }
    tn_sb_put(b, " }", 2);
}

static void tn_show(tn_sb *b, tn_val v, const tn_type *t, bool quote) {
    switch (t->kind) {
    case TN_INT: {
        char tmp[32];
        tn_sb_put(b, tmp, snprintf(tmp, sizeof tmp, "%lld", (long long)v.i));
        break;
    }
    case TN_BOOL:
        if (v.b) tn_sb_put(b, "true", 4);
        else tn_sb_put(b, "false", 5);
        break;
    case TN_UNIT:
        tn_sb_put(b, "()", 2);
        break;
    case TN_STR: {
        tn_str *s = v.p;
        if (!quote) {
            tn_sb_put(b, s->data, s->len);
            break;
        }
        tn_sb_put(b, "\"", 1);
        for (int64_t i = 0; i < s->len; i++) {
            unsigned char c = s->data[i];
            char tmp[16];
            switch (c) {
            case '"': tn_sb_put(b, "\\\"", 2); break;
            case '\\': tn_sb_put(b, "\\\\", 2); break;
            case '\n': tn_sb_put(b, "\\n", 2); break;
            case '\t': tn_sb_put(b, "\\t", 2); break;
            case '\r': tn_sb_put(b, "\\r", 2); break;
            case 0: tn_sb_put(b, "\\0", 2); break;
            default:
                if (c < 0x20 || c == 0x7f) tn_sb_put(b, tmp, snprintf(tmp, sizeof tmp, "\\u{%x}", c));
                else tn_sb_put(b, (const char *)&c, 1);
            }
        }
        tn_sb_put(b, "\"", 1);
        break;
    }
    case TN_LIST: {
        tn_list *l = v.p;
        tn_sb_put(b, "[", 1);
        for (int64_t i = 0; i < l->len; i++) {
            if (i) tn_sb_put(b, ", ", 2);
            tn_show(b, l->buf->items[i], t->elem, true);
        }
        tn_sb_put(b, "]", 1);
        break;
    }
    case TN_RECORD: {
        tn_rec *r = v.p;
        tn_sb_put(b, t->name, strlen(t->name));
        if (t->nfields == 0) {
            tn_sb_put(b, " {}", 3);
            break;
        }
        tn_show_fields(b, r->f, t->nfields, t->fnames, t->ftypes);
        break;
    }
    case TN_ENUM: {
        tn_enum *e = v.p;
        const tn_variant *var = &t->variants[e->tag];
        tn_sb_put(b, var->name, strlen(var->name));
        if (var->nfields) tn_show_fields(b, e->f, var->nfields, var->fnames, var->ftypes);
        break;
    }
    }
}

static tn_str *tn_to_str(tn_val v, const tn_type *t, bool quote) {
    if (t->kind == TN_STR && !quote) return v.p;
    tn_sb b = {0};
    tn_show(&b, v, t, quote);
    tn_str *s = tn_str_from(b.p ? b.p : "", b.len);
    free(b.p);
    return s;
}

static bool tn_eq(tn_val a, tn_val b, const tn_type *t) {
    switch (t->kind) {
    case TN_INT: return a.i == b.i;
    case TN_BOOL: return a.b == b.b;
    case TN_UNIT: return true;
    case TN_STR: return tn_str_eq(a.p, b.p);
    case TN_RECORD: {
        tn_rec *x = a.p, *y = b.p;
        for (int64_t i = 0; i < t->nfields; i++)
            if (!tn_eq(x->f[i], y->f[i], t->ftypes[i])) return false;
        return true;
    }
    case TN_ENUM: {
        tn_enum *x = a.p, *y = b.p;
        if (x->tag != y->tag) return false;
        const tn_variant *var = &t->variants[x->tag];
        for (int64_t i = 0; i < var->nfields; i++)
            if (!tn_eq(x->f[i], y->f[i], var->ftypes[i])) return false;
        return true;
    }
    default: {
        tn_list *x = a.p, *y = b.p;
        if (x->len != y->len) return false;
        for (int64_t i = 0; i < x->len; i++)
            if (!tn_eq(x->buf->items[i], y->buf->items[i], t->elem)) return false;
        return true;
    }
    }
}

static bool tn_list_contains(tn_list *l, tn_val v, const tn_type *elem) {
    for (int64_t i = 0; i < l->len; i++)
        if (tn_eq(l->buf->items[i], v, elem)) return true;
    return false;
}

static tn_str *tn_join(tn_list *l, const tn_type *elem, tn_str *sep) {
    tn_sb b = {0};
    for (int64_t i = 0; i < l->len; i++) {
        if (i) tn_sb_put(&b, sep->data, sep->len);
        tn_show(&b, l->buf->items[i], elem, false);
    }
    tn_str *s = tn_str_from(b.p ? b.p : "", b.len);
    free(b.p);
    return s;
}

/* "a = 1, b = 2", for contract failure hints. */
static const char *tn_inputs(int n, const char **names, tn_str **vals) {
    tn_sb b = {0};
    for (int i = 0; i < n; i++) {
        if (i) tn_sb_put(&b, ", ", 2);
        tn_sb_put(&b, names[i], strlen(names[i]));
        tn_sb_put(&b, " = ", 3);
        tn_sb_put(&b, vals[i]->data, vals[i]->len);
    }
    tn_sb_put(&b, "", 1);
    return b.p;
}

static int64_t tn_parse_int(tn_str *s, tn_loc at) {
    const char *p = s->data, *end = s->data + s->len;
    while (p < end && (*p == ' ' || (*p >= '\t' && *p <= '\r'))) p++;
    while (end > p && (end[-1] == ' ' || (end[-1] >= '\t' && end[-1] <= '\r'))) end--;
    bool neg = p < end && *p == '-';
    if (p < end && (*p == '-' || *p == '+')) p++;
    bool ok = p < end;
    uint64_t v = 0, limit = neg ? (uint64_t)INT64_MAX + 1 : (uint64_t)INT64_MAX;
    for (; ok && p < end; p++) {
        if (*p < '0' || *p > '9' || v > (limit - (uint64_t)(*p - '0')) / 10) ok = false;
        else v = v * 10 + (uint64_t)(*p - '0');
    }
    if (!ok) tn_fail("runtime", at, tn_fmt("cannot convert %s to Int", tn_to_str(TN_P(s), &TY_s, true)->data), NULL);
    return neg ? (int64_t)(0 - v) : (int64_t)v;
}

/* ---- effects ---- */

static tn_unit tn_print(int n, tn_str **parts) {
    for (int i = 0; i < n; i++) {
        if (i) fputc(' ', stdout);
        fwrite(parts[i]->data, 1, parts[i]->len, stdout);
    }
    fputc('\n', stdout);
    return 0;
}

static tn_str *tn_input(tn_str *prompt, tn_loc at) {
    (void)at;
    if (prompt) fwrite(prompt->data, 1, prompt->len, stdout);
    fflush(stdout);
    tn_sb b = {0};
    int c;
    while ((c = fgetc(stdin)) != EOF && c != '\n') {
        char ch = (char)c;
        tn_sb_put(&b, &ch, 1);
    }
    while (b.len > 0 && b.p[b.len - 1] == '\r') b.len--;
    tn_str *s = tn_str_from(b.p ? b.p : "", b.len);
    free(b.p);
    return s;
}

static tn_str *tn_read_file(tn_str *path, tn_loc at) {
    FILE *f = fopen(path->data, "rb");
    if (!f)
        tn_fail("runtime", at, tn_fmt("cannot read %s: %s (os error %d)", tn_to_str(TN_P(path), &TY_s, true)->data,
                                      strerror(errno), errno), NULL);
    tn_sb b = {0};
    char tmp[65536];
    size_t n;
    while ((n = fread(tmp, 1, sizeof tmp, f)) > 0) tn_sb_put(&b, tmp, n);
    fclose(f);
    tn_str *s = tn_str_from(b.p ? b.p : "", b.len);
    free(b.p);
    return s;
}

static tn_unit tn_write_file(tn_str *path, tn_str *content, tn_loc at) {
    FILE *f = fopen(path->data, "wb");
    if (!f || fwrite(content->data, 1, content->len, f) != (size_t)content->len)
        tn_fail("runtime", at, tn_fmt("cannot write %s: %s (os error %d)", tn_to_str(TN_P(path), &TY_s, true)->data,
                                      strerror(errno), errno), NULL);
    fclose(f);
    return 0;
}

static int64_t tn_now(void) {
    struct timeval tv;
    gettimeofday(&tv, NULL);
    return (int64_t)tv.tv_sec * 1000 + tv.tv_usec / 1000;
}

static int64_t tn_random(int64_t n, tn_loc at) {
    if (n <= 0) tn_fail("runtime", at, "invalid arguments to `random`: the limit must be positive", NULL);
    tn_rng ^= tn_rng << 13;
    tn_rng ^= tn_rng >> 7;
    tn_rng ^= tn_rng << 17;
    return (int64_t)(tn_rng % (uint64_t)n);
}

static void tn_init(void) {
    static char buf[1 << 16];
    setvbuf(stdout, buf, _IOFBF, sizeof buf);
    struct timespec ts;
    clock_gettime(CLOCK_REALTIME, &ts);
    tn_rng = ((uint64_t)ts.tv_sec * 1000000000u + (uint64_t)ts.tv_nsec) | 1;
}

/* ---- end of runtime ---- */
