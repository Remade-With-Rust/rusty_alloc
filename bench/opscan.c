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
    else {
        fprintf(stderr, "unknown op: %s\n", op);
        return 2;
    }
    return 0;
}
