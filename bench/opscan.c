/* opscan — per-operation allocator scan driver.
 *
 * One C binary, run under LD_PRELOAD against each allocator, so every arm
 * executes byte-identical caller code and the only variable is the allocator.
 *
 * Each op runs a fixed, dependency-preserving loop. The harness runs every op
 * at N and 2N iterations and reports (Ir(2N) - Ir(N)) / N, which cancels
 * process startup, dynamic-linker work, allocator init and first-touch warmup
 * exactly — no null arm to subtract by hand, no fixed-cost assumptions.
 *
 * usage: opscan <op> <iters>
 */
#include <malloc.h>
#include <pthread.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

/* Volatile sink: stops the compiler from proving the allocation is dead and
 * deleting the very thing we are measuring. */
static void *volatile sink;

#define BATCH 64

static void op_pair(size_t n, long iters) {
    for (long i = 0; i < iters; i++) {
        void *p = malloc(n);
        sink = p;
        free(p);
    }
}

/* NOTE: the writes MUST go through a volatile pointer and be read back.
 * Written as plain stores into a block that is freed without being read, GCC
 * deletes them as dead - and this op then measured byte-for-byte identical to
 * `small` on all three arms, which is how the bug was caught. */
static void op_pair_touch(size_t n, long iters) {
    unsigned long acc = 0;
    for (long i = 0; i < iters; i++) {
        volatile char *p = (volatile char *)malloc(n);
        sink = (void *)p;
        p[0] = (char)i;      /* first byte */
        p[n - 1] = (char)i;  /* last byte  */
        acc += (unsigned long)p[0] + (unsigned long)p[n - 1];
        free((void *)p);
    }
    sink = (void *)acc;
}

static void op_calloc(size_t n, long iters) {
    for (long i = 0; i < iters; i++) {
        void *p = calloc(1, n);
        sink = p;
        free(p);
    }
}

/* Batch alloc then batch free. LIFO is the friendly order (free list stays
 * hot); FIFO is the adversarial one and is where free-list layout shows up. */
static void op_batch(size_t n, long iters, int lifo) {
    void *v[BATCH];
    long rounds = iters / BATCH;
    for (long i = 0; i < rounds; i++) {
        for (int j = 0; j < BATCH; j++) {
            v[j] = malloc(n);
            sink = v[j];
        }
        if (lifo) {
            for (int j = BATCH - 1; j >= 0; j--) free(v[j]);
        } else {
            for (int j = 0; j < BATCH; j++) free(v[j]);
        }
    }
}

static void op_realloc_grow(long iters) {
    for (long i = 0; i < iters; i++) {
        void *p = malloc(64);
        p = realloc(p, 128);
        p = realloc(p, 512);
        sink = p;
        free(p);
    }
}

static void op_aligned(size_t align, size_t n, long iters) {
    for (long i = 0; i < iters; i++) {
        void *p = NULL;
        if (posix_memalign(&p, align, n) != 0) abort();
        sink = p;
        free(p);
    }
}

static void op_usable(size_t n, long iters) {
    void *p = malloc(n);
    size_t acc = 0;
    for (long i = 0; i < iters; i++) acc += malloc_usable_size(p);
    sink = (void *)acc;
    free(p);
}

/* Mixed sizes across bins with a live working set — closest thing here to a
 * real program's shape. */
static void op_mixed(long iters) {
    void *v[BATCH];
    memset(v, 0, sizeof v);
    for (long i = 0; i < iters; i++) {
        int j = (int)(i % BATCH);
        if (v[j]) free(v[j]);
        size_t n = 16 + (size_t)((i * 37) % 3000);
        v[j] = malloc(n);
        sink = v[j];
    }
    for (int j = 0; j < BATCH; j++) {
        if (v[j]) free(v[j]);
    }
}

/* A LIVE SET, which is the regime every other op here is missing.
 *
 * `op_mixed` cycles 64 blocks; `op_pair` cycles one. Both keep a single page
 * per size class permanently warm, so the allocator's generic path is barely
 * entered and its page queues never grow. Real programs — and `alloc-test`,
 * the only mimalloc-bench benchmark with this shape — hold hundreds of
 * thousands of live objects across many size classes and free them in random
 * order, which is what makes pages fill, park, and un-park.
 *
 * This op reproduces that: LIVE slots held live at all times, each step
 * freeing a randomly chosen one and replacing it with a fresh random size.
 * Sizes follow alloc-test's own distribution (a power-of-two class chosen by
 * trailing-zero count, so small classes dominate) so the bin spread matches.
 *
 * Deterministic: a fixed-seed LCG, no clock, no address dependence. The
 * two-point estimator cancels the setup because the fill loop is identical at
 * N and 2N.
 */
#define LIVE_SLOTS (1u << 16)

static uint64_t lv_rng = 0x9E3779B97F4A7C15ull;

static uint64_t lv_next(void) {
    lv_rng = lv_rng * 6364136223846793005ull + 1442695040888963407ull;
    return lv_rng;
}

/* alloc-test's `calcSizeWithStatsAdjustment` for maxSizeExp = 10 (max 1 KiB). */
static size_t lv_size(uint64_t r) {
    uint32_t base = (uint32_t)(r & ((1u << 7) - 1)) + 1;
    int idx = __builtin_ctz(base) + 2;
    size_t mask = ((size_t)1 << idx) - 1;
    return ((size_t)(r >> 20) & mask) + 1 + ((size_t)1 << idx);
}

static void op_liveset(long iters) {
    static void *slots[LIVE_SLOTS];
    lv_rng = 0x9E3779B97F4A7C15ull;
    for (unsigned i = 0; i < LIVE_SLOTS; i++) {
        slots[i] = malloc(lv_size(lv_next()));
        sink = slots[i];
    }
    for (long i = 0; i < iters; i++) {
        uint64_t r = lv_next();
        unsigned idx = (unsigned)((r >> 33) & (LIVE_SLOTS - 1));
        free(slots[idx]);
        slots[idx] = malloc(lv_size(r));
        sink = slots[idx];
    }
    for (unsigned i = 0; i < LIVE_SLOTS; i++) free(slots[i]);
}

/* sh8bench's shape, at opscan speed.
 *
 * `liveset` replaces a RANDOM slot; sh8bench does not. It allocates in bursts
 * of one size class at a time, walking a histogram dominated by 8/16/48-byte
 * blocks with a long tail out to 168 KiB, into a large buffer — then frees the
 * bottom of that buffer in allocation order and the top in REVERSE. Order is
 * the point: FIFO and LIFO release put a page's free list in different states,
 * and the size tail crosses every page class the allocator has (small, medium,
 * large span, huge).
 *
 * Single-threaded, so it does NOT model sh8bench's foreign frees; the profile
 * says whether that matters.
 */
#define SH_SLOTS (1u << 15)

static const struct {
    size_t size;
    unsigned count;
} sh_hist[] = {{8, 1000},   {16, 5000},  {48, 1000},   {72, 100},
               {148, 100},  {200, 100},  {520, 10},    {1056, 5},
               {4096, 3},   {9162, 1},   {34562, 1},   {168524, 1}};

#define SH_CLASSES (sizeof sh_hist / sizeof sh_hist[0])

/* Walk the histogram the way sh8bench does: `count` blocks of one class, then
 * on to the next class, wrapping. */
static size_t sh_next(unsigned *cls, unsigned *left) {
    while (*left == 0) {
        *cls = (*cls + 1) % SH_CLASSES;
        *left = sh_hist[*cls].count;
    }
    (*left)--;
    return sh_hist[*cls].size;
}

static void op_shbench(long iters) {
    static void *slots[SH_SLOTS];
    unsigned cls = 0, left = sh_hist[0].count;
    unsigned wr = 0;
    long done = 0;
    /* sh8bench's cycle: fill the buffer, then release the bottom fifth in
     * ALLOCATION order and the top fifth in REVERSE, then refill those. Whole
     * pages empty together, which is what drives span carve/retire churn —
     * the thing a one-slot-at-a-time replacement never produces. */
    while (done < iters) {
        for (; wr < SH_SLOTS && done < iters; wr++, done++) {
            slots[wr] = malloc(sh_next(&cls, &left));
            sink = slots[wr];
            char *c = slots[wr];
            if (c) { c[0] = 0; }
        }
        if (wr < SH_SLOTS) break;
        unsigned fifth = SH_SLOTS / 5;
        for (unsigned k = 0; k < fifth; k++) {          /* bottom, in order */
            free(slots[k]);
            slots[k] = NULL;
        }
        for (unsigned k = 0; k < fifth; k++) {          /* top, in reverse */
            free(slots[SH_SLOTS - 1 - k]);
            slots[SH_SLOTS - 1 - k] = NULL;
        }
        for (unsigned k = 0; k < fifth && done < iters; k++, done++) {
            slots[k] = malloc(sh_next(&cls, &left));
            sink = slots[k];
        }
        for (unsigned k = 0; k < fifth && done < iters; k++, done++) {
            slots[SH_SLOTS - 1 - k] = malloc(sh_next(&cls, &left));
            sink = slots[SH_SLOTS - 1 - k];
        }
    }
    for (unsigned k = 0; k < SH_SLOTS; k++) {
        if (slots[k]) free(slots[k]);
        slots[k] = NULL;
    }
}

/* CROSS-THREAD frees, without the interleaving.
 *
 * `xmalloc-test`'s shape is "one thread allocates, another frees" — the
 * `remote_free` path and the delayed list. It is also a TIME-BOUNDED loop, so
 * the work it does depends on how fast the machine is at that moment: three
 * runs of one binary measured 109.1M / 109.2M / 124.9M allocator instructions,
 * a 13.6% spread. No primitive can be adjudicated on it.
 *
 * This does the same ALLOCATOR work deterministically. The main thread
 * allocates a batch; a spawned thread frees the whole batch and is joined
 * before the next one starts. Every free is remote — the freeing thread is not
 * the owner — so it routes exactly as xmalloc-test's does, and because the two
 * threads never run at the same time there is no interleaving to vary the
 * count. Thread creation is amortised over XT_BATCH frees.
 */
#define XT_BATCH 4096

static void *xt_blocks[XT_BATCH];
static long xt_n;

static void *xt_free_fn(void *arg) {
    (void)arg;
    for (long i = 0; i < xt_n; i++) free(xt_blocks[i]);
    return NULL;
}

static void op_xthread(long iters) {
    long done = 0;
    while (done < iters) {
        long n = iters - done;
        if (n > XT_BATCH) n = XT_BATCH;
        for (long i = 0; i < n; i++) {
            xt_blocks[i] = malloc(64);
            sink = xt_blocks[i];
        }
        xt_n = n;
        pthread_t t;
        if (pthread_create(&t, NULL, xt_free_fn, NULL) != 0) abort();
        pthread_join(t, NULL);
        done += n;
    }
}

int main(int argc, char **argv) {
    if (argc < 3) {
        fprintf(stderr, "usage: opscan <op> <iters>\n");
        return 2;
    }
    const char *op = argv[1];
    long n = atol(argv[2]);

    if (!strcmp(op, "small"))            op_pair(16, n);
    else if (!strcmp(op, "small_touch")) op_pair_touch(16, n);
    else if (!strcmp(op, "med"))         op_pair(256, n);
    else if (!strcmp(op, "big"))         op_pair(4096, n);
    else if (!strcmp(op, "large"))       op_pair(65536, n);
    else if (!strcmp(op, "huge"))        op_pair(2 * 1024 * 1024, n);
    else if (!strcmp(op, "calloc"))      op_calloc(256, n);
    else if (!strcmp(op, "batch_lifo"))  op_batch(64, n, 1);
    else if (!strcmp(op, "batch_fifo"))  op_batch(64, n, 0);
    else if (!strcmp(op, "realloc"))     op_realloc_grow(n);
    else if (!strcmp(op, "aligned"))     op_aligned(64, 256, n);
    else if (!strcmp(op, "usable"))      op_usable(256, n);
    else if (!strcmp(op, "mixed"))       op_mixed(n);
    else if (!strcmp(op, "liveset"))     op_liveset(n);
    else if (!strcmp(op, "shbench"))     op_shbench(n);
    else if (!strcmp(op, "xthread"))     op_xthread(n);
    else {
        fprintf(stderr, "unknown op: %s\n", op);
        return 2;
    }
    return 0;
}
