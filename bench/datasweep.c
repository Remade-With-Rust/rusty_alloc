/* datasweep — does this allocator hand back CORRECT memory for every shape of
 * request, not just fast memory for the common one?
 *
 * The instruction-count harnesses in this directory answer "how much work".
 * They cannot see a block that overlaps another, a `calloc` that returns a
 * recycled block still holding old bytes, a `realloc` that loses the tail, or
 * an alignment that is honoured for 64 but not for 4096. Those are the
 * failures that matter most and the ones a benchmark is blind to.
 *
 * Every block written here carries a pattern derived from its own identity, so
 * a byte that comes back wrong names the allocation that should have owned it.
 * The program is deterministic (its own PRNG, no rand(), no clock), returns
 * non-zero on the first inconsistency, and is allocator-agnostic: run it under
 * LD_PRELOAD against ra, mimalloc, jemalloc and glibc and all four must pass.
 * A difference between arms is the interesting signal.
 *
 *   cc -O2 -pthread -o datasweep datasweep.c
 *   LD_PRELOAD=... ./datasweep [scale]
 */
#define _GNU_SOURCE
#include <errno.h>
#include <malloc.h>
#include <pthread.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

static long failures = 0;
static long checks = 0;
static long bytes_written = 0;

static void fail(const char *phase, const char *what, size_t a, size_t b) {
    if (failures < 20) {
        fprintf(stderr, "FAIL [%s] %s (%zu vs %zu)\n", phase, what, a, b);
    }
    failures++;
}

/* --- deterministic PRNG: no rand(), no clock, identical in every arm ------ */
static uint64_t rng_state = 0x9E3779B97F4A7C15ull;
static inline uint64_t xrand(void) {
    rng_state = rng_state * 6364136223846793005ull + 1442695040888963407ull;
    return rng_state >> 33;
}

/* --- identity-bearing fill ----------------------------------------------- */
static inline unsigned char pat(uint64_t id, size_t i) {
    return (unsigned char)((id * 31u) ^ (i * 17u) ^ 0xA5u);
}

static void fill(void *p, size_t n, uint64_t id) {
    unsigned char *b = (unsigned char *)p;
    for (size_t i = 0; i < n; i++) b[i] = pat(id, i);
    bytes_written += (long)n;
}

static int verify(const char *phase, const void *p, size_t n, uint64_t id) {
    const unsigned char *b = (const unsigned char *)p;
    checks++;
    for (size_t i = 0; i < n; i++) {
        if (b[i] != pat(id, i)) {
            fail(phase, "content mismatch", (size_t)b[i], (size_t)pat(id, i));
            return 0;
        }
    }
    return 1;
}

static int check_zero(const char *phase, const void *p, size_t n) {
    const unsigned char *b = (const unsigned char *)p;
    checks++;
    for (size_t i = 0; i < n; i++) {
        if (b[i] != 0) { fail(phase, "calloc byte not zero", i, (size_t)b[i]); return 0; }
    }
    return 1;
}

/* Every allocation must satisfy the platform's fundamental alignment. */
static void check_basic_align(const char *phase, void *p, size_t sz) {
    checks++;
    size_t need = (sz >= 16) ? 16 : 8;
    if (((uintptr_t)p % need) != 0) fail(phase, "under-aligned", (uintptr_t)p % need, need);
}

/* ---- A. every small size, exhaustively ---------------------------------- */
static void phase_every_size(void) {
    for (size_t sz = 1; sz <= 4096; sz++) {
        void *p = malloc(sz);
        if (!p) { fail("A/every-size", "malloc returned null", sz, 0); return; }
        check_basic_align("A/every-size", p, sz);
        size_t us = malloc_usable_size(p);
        if (us < sz) fail("A/every-size", "usable_size < requested", us, sz);
        /* Writing the FULL usable extent must be safe — if it is not, the
         * usable_size contract is lying and the next block will see it. */
        fill(p, us, sz);
        if (!verify("A/every-size", p, us, sz)) return;
        free(p);
    }
}

/* ---- B. class boundaries and the sizes around them ---------------------- */
static void phase_boundaries(void) {
    static const size_t base[] = {
        0, 1, 7, 8, 9, 15, 16, 17, 31, 32, 33, 63, 64, 65, 127, 128, 129,
        255, 256, 257, 511, 512, 513, 1023, 1024, 1025,          /* small max */
        2047, 2048, 2049, 4095, 4096, 4097, 8191, 8192, 8193,
        16383, 16384, 16385, 32768, 65535, 65536, 65537,         /* slice */
        131072, 262144, 524287, 524288, 524289,                  /* medium/large */
        1u << 20, (1u << 20) + 1, 4u << 20,                      /* huge */
    };
    const size_t n = sizeof base / sizeof base[0];
    void **keep = (void **)calloc(n, sizeof(void *));
    if (!keep) { fail("B/boundaries", "calloc of keeper array failed", n, 0); return; }

    /* Hold them ALL live at once: a boundary bug that only shows when two
     * neighbouring classes are simultaneously in play cannot be seen by an
     * alloc/free-immediately loop. */
    for (size_t i = 0; i < n; i++) {
        size_t sz = base[i];
        void *p = malloc(sz);
        if (!p && sz != 0) { fail("B/boundaries", "malloc returned null", sz, 0); continue; }
        if (!p) continue;                       /* malloc(0) may legally be NULL */
        check_basic_align("B/boundaries", p, sz);
        if (malloc_usable_size(p) < sz) fail("B/boundaries", "usable_size < requested", 0, sz);
        fill(p, sz, 0x1000 + i);
        keep[i] = p;
    }
    for (size_t i = 0; i < n; i++) {
        if (keep[i]) { verify("B/boundaries", keep[i], base[i], 0x1000 + i); free(keep[i]); }
    }
    free(keep);
}

/* ---- C. calloc really zeroes, INCLUDING recycled memory ----------------- */
static void phase_calloc_zero(void) {
    /* Fresh pages are zero because the OS gave them so; the interesting case
     * is a block that has already held data. Dirty it, free it, then demand a
     * zeroed block of the same class back. This is what a `free_is_zero`
     * optimisation gets wrong when it gets anything wrong. */
    static const size_t sizes[] = {8, 16, 24, 64, 100, 512, 1000, 1024, 4096, 16384, 100000};
    for (size_t s = 0; s < sizeof sizes / sizeof sizes[0]; s++) {
        size_t sz = sizes[s];
        for (int round = 0; round < 8; round++) {
            void *d = malloc(sz);
            if (!d) { fail("C/calloc-zero", "malloc returned null", sz, 0); return; }
            memset(d, 0xDD, sz);                 /* poison, then recycle */
            free(d);
            void *z = calloc(1, sz);
            if (!z) { fail("C/calloc-zero", "calloc returned null", sz, 0); return; }
            if (!check_zero("C/calloc-zero", z, sz)) { free(z); return; }
            free(z);
        }
    }
    /* calloc's multiply must be checked, not wrapped. `volatile` so the
     * compiler cannot fold the product and warn about the very case this is
     * here to exercise. */
    volatile size_t huge_n = (size_t)-1 / 2, huge_s = 4;
    void *ov = calloc(huge_n, huge_s);
    checks++;
    if (ov) { fail("C/calloc-zero", "calloc overflow not refused", (size_t)huge_n, (size_t)huge_s); free(ov); }
}

/* ---- D. alignment matrix ------------------------------------------------ */
static void phase_alignment(void) {
    for (size_t align = 8; align <= 65536; align <<= 1) {
        for (size_t sz = 1; sz <= 8192; sz = sz * 3 + 1) {
            void *p = NULL;
            int rc = posix_memalign(&p, align, sz);
            if (rc != 0 || !p) { fail("D/align", "posix_memalign failed", align, sz); continue; }
            checks++;
            if (((uintptr_t)p % align) != 0)
                fail("D/align", "alignment not honoured", (uintptr_t)p % align, align);
            if (malloc_usable_size(p) < sz) fail("D/align", "usable_size < requested", 0, sz);
            fill(p, sz, align * 7 + sz);
            if (!verify("D/align", p, sz, align * 7 + sz)) { free(p); return; }
            free(p);
        }
    }
    /* An aligned block must survive being freed through plain free(), and a
     * whole cohort held live at once must not overlap. */
    enum { N = 512 };
    void *v[N];
    size_t vs[N];
    for (int i = 0; i < N; i++) {
        size_t align = (size_t)1 << (3 + (i % 10));      /* 8 .. 4096 */
        vs[i] = 1 + (size_t)(xrand() % 3000);
        if (posix_memalign(&v[i], align, vs[i]) != 0) { v[i] = NULL; continue; }
        checks++;
        if (((uintptr_t)v[i] % align) != 0) fail("D/align", "cohort misaligned", align, 0);
        fill(v[i], vs[i], 0x20000 + (uint64_t)i);
    }
    /* Verify only AFTER the whole cohort is live: an aligned carve that
     * overlapped a neighbour would still read back correctly if each block
     * were checked before the next was handed out. */
    for (int i = 0; i < N; i++) {
        if (v[i]) verify("D/align", v[i], vs[i], 0x20000 + (uint64_t)i);
    }
    for (int i = 0; i < N; i++) free(v[i]);
}

/* ---- E. realloc preserves content across every direction ---------------- */
static void phase_realloc(void) {
    static const size_t steps[] = {8, 64, 1000, 1024, 5000, 100, 32, 65536, 300, 200000, 16, 0};
    const size_t n = sizeof steps / sizeof steps[0];

    void *p = malloc(steps[0]);
    if (!p) { fail("E/realloc", "malloc returned null", steps[0], 0); return; }
    size_t cur = steps[0];
    fill(p, cur, 0xE0);

    for (size_t i = 1; i < n; i++) {
        size_t next = steps[i];
        void *q = realloc(p, next);
        if (next == 0) { /* realloc(p,0): implementation-defined; either is fine */
            if (q) free(q);
            p = NULL;
            break;
        }
        if (!q) { fail("E/realloc", "realloc returned null", next, 0); free(p); return; }
        size_t keep = cur < next ? cur : next;
        /* The preserved prefix must still read as the ORIGINAL pattern. */
        checks++;
        for (size_t k = 0; k < keep; k++) {
            if (((unsigned char *)q)[k] != pat(0xE0, k)) {
                fail("E/realloc", "prefix not preserved across realloc", k, next);
                free(q);
                return;
            }
        }
        /* Re-establish the pattern over the WHOLE new extent, so the next
         * step's prefix check is against a known state either way. */
        fill(q, next, 0xE0);
        p = q;
        cur = next;
    }
    free(p);

    /* realloc(NULL, n) == malloc(n) */
    void *r = realloc(NULL, 1234);
    checks++;
    if (!r) fail("E/realloc", "realloc(NULL,n) returned null", 1234, 0);
    else { fill(r, 1234, 0xE1); verify("E/realloc", r, 1234, 0xE1); free(r); }
}

/* ---- F. churn against a live set, with integrity held throughout -------- */
static void phase_churn(long ops) {
    enum { LIVE = 8192 };
    void **slot = (void **)calloc(LIVE, sizeof(void *));
    size_t *len = (size_t *)calloc(LIVE, sizeof(size_t));
    uint64_t *id = (uint64_t *)calloc(LIVE, sizeof(uint64_t));
    if (!slot || !len || !id) { fail("F/churn", "bookkeeping alloc failed", 0, 0); return; }

    for (long k = 0; k < ops; k++) {
        int i = (int)(xrand() % LIVE);
        if (slot[i]) {
            if (!verify("F/churn", slot[i], len[i], id[i])) return;
            free(slot[i]);
            slot[i] = NULL;
        }
        /* A spread that crosses every routing decision the allocator makes:
         * fast-path small, medium, large and the occasional huge. */
        uint64_t r = xrand() % 1000;
        size_t sz = (r < 800) ? 1 + (size_t)(xrand() % 1024)
                  : (r < 970) ? 1025 + (size_t)(xrand() % 64000)
                  : (r < 998) ? 65537 + (size_t)(xrand() % 400000)
                              : 600000 + (size_t)(xrand() % 200000);
        void *p = malloc(sz);
        if (!p) { fail("F/churn", "malloc returned null", sz, 0); return; }
        check_basic_align("F/churn", p, sz);
        slot[i] = p; len[i] = sz; id[i] = (uint64_t)k + 1;
        fill(p, sz, id[i]);
    }
    for (int i = 0; i < LIVE; i++) {
        if (slot[i]) { verify("F/churn", slot[i], len[i], id[i]); free(slot[i]); }
    }
    free(slot); free(len); free(id);
}

/* ---- G. cross-thread: allocate here, free there, and the reverse -------- */
typedef struct { void **v; size_t *len; uint64_t *id; long n; int produce; } work_t;

static void *worker(void *arg) {
    work_t *w = (work_t *)arg;
    if (w->produce) {
        for (long i = 0; i < w->n; i++) {
            size_t sz = 1 + (size_t)((uint64_t)i * 2654435761u % 8192u);
            void *p = malloc(sz);
            if (!p) { fail("G/xthread", "malloc returned null", sz, 0); return NULL; }
            w->v[i] = p; w->len[i] = sz; w->id[i] = 0x30000 + (uint64_t)i;
            fill(p, sz, w->id[i]);
        }
    } else {
        for (long i = 0; i < w->n; i++) {
            if (!w->v[i]) continue;
            verify("G/xthread", w->v[i], w->len[i], w->id[i]);
            free(w->v[i]);                       /* freed by a NON-owning thread */
            w->v[i] = NULL;
        }
    }
    return NULL;
}

static void phase_xthread(long per_thread, int nthreads) {
    for (int t = 0; t < nthreads; t++) {
        work_t w;
        w.n = per_thread;
        w.v = (void **)calloc((size_t)per_thread, sizeof(void *));
        w.len = (size_t *)calloc((size_t)per_thread, sizeof(size_t));
        w.id = (uint64_t *)calloc((size_t)per_thread, sizeof(uint64_t));
        if (!w.v || !w.len || !w.id) { fail("G/xthread", "bookkeeping alloc failed", 0, 0); return; }

        /* produced on a spawned thread, freed on main */
        pthread_t th;
        w.produce = 1;
        pthread_create(&th, NULL, worker, &w);
        pthread_join(th, NULL);
        for (long i = 0; i < per_thread; i++) {
            if (!w.v[i]) continue;
            verify("G/xthread", w.v[i], w.len[i], w.id[i]);
            free(w.v[i]);
        }

        /* produced on main, freed on a spawned thread */
        for (long i = 0; i < per_thread; i++) {
            size_t sz = 1 + (size_t)((uint64_t)i * 40503u % 6000u);
            w.v[i] = malloc(sz);
            if (!w.v[i]) { fail("G/xthread", "malloc returned null", sz, 0); return; }
            w.len[i] = sz; w.id[i] = 0x40000 + (uint64_t)i;
            fill(w.v[i], sz, w.id[i]);
        }
        w.produce = 0;
        pthread_create(&th, NULL, worker, &w);
        pthread_join(th, NULL);

        free(w.v); free(w.len); free(w.id);
    }
}

/* ---- H. no two live blocks overlap -------------------------------------- */
typedef struct { uintptr_t a; size_t n; } ext_t;

static int cmp_ext(const void *x, const void *y) {
    uintptr_t a = ((const ext_t *)x)->a, b = ((const ext_t *)y)->a;
    return (a > b) - (a < b);
}

static void phase_no_overlap(void) {
    enum { N = 20000 };
    ext_t *e = (ext_t *)malloc(N * sizeof(ext_t));
    void **p = (void **)malloc(N * sizeof(void *));
    if (!e || !p) { fail("H/overlap", "bookkeeping alloc failed", 0, 0); free(e); free(p); return; }

    for (int i = 0; i < N; i++) {
        size_t s = 1 + (size_t)(xrand() % 2000);
        p[i] = malloc(s);
        if (!p[i]) { fail("H/overlap", "malloc returned null", s, 0); return; }
        /* Record the USABLE extent, not the requested one: two blocks whose
         * usable ranges overlap is a bug even when their requested ranges do
         * not, because the usable extent is what callers are told they own. */
        e[i].a = (uintptr_t)p[i];
        e[i].n = malloc_usable_size(p[i]);
        memset(p[i], (int)(i & 0xFF), e[i].n);
    }
    qsort(e, N, sizeof(ext_t), cmp_ext);
    for (int i = 1; i < N; i++) {
        checks++;
        if (e[i - 1].a + e[i - 1].n > e[i].a) {
            fail("H/overlap", "live blocks overlap", (size_t)(e[i - 1].a + e[i - 1].n),
                 (size_t)e[i].a);
            break;
        }
    }
    for (int i = 0; i < N; i++) free(p[i]);
    free(e); free(p);
}

int main(int argc, char **argv) {
    long scale = (argc > 1) ? atol(argv[1]) : 1;
    if (scale < 1) scale = 1;

    phase_every_size();
    phase_boundaries();
    phase_calloc_zero();
    phase_alignment();
    phase_realloc();
    phase_churn(120000 * scale);
    phase_xthread(4000 * scale, 4);
    phase_no_overlap();

    printf("datasweep: %ld checks, %ld MiB written, %ld failures\n",
           checks, bytes_written / (1024 * 1024), failures);
    if (failures) { printf("DATASWEEP FAILED\n"); return 1; }
    printf("DATASWEEP PASSED\n");
    return 0;
}
