/* Per-thread retention probe: does peak RSS scale with THREAD COUNT?
 *
 * FFAI's scaling.rs found the mechanism: a per-thread heap retains freed pages,
 * and with 4 workers + 24 candle threads that retention is multiplied 28 ways
 * (they measured 174 MiB steady against 26.3 MiB live). A single-threaded
 * benchmark cannot show this at all, which is why bench/rss.sh saw rusty_alloc
 * at or below mimalloc while FFAI saw +17.9%.
 *
 * This holds N LONG-LIVED concurrent threads, each cycling a working set, then
 * reports peak RSS for the process. Sweep N to see retention scale.
 *
 * usage: rss-threads <threads> <rounds>
 */
#include <pthread.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

static int g_rounds;
/* Max allocation size. Tensor work lives in the hundreds-of-KB..MB range,
 * which takes a DIFFERENT path (spans/huge, arena-backed and aggressively
 * cached) than the binned path a 32 KB ceiling exercises. */
static size_t g_maxsize = 32768;
/* 0 = plain malloc; otherwise posix_memalign to this boundary. */
static size_t g_align = 0;

/* Each thread: allocate a batch spanning small..large, touch it, free it all,
 * repeat. Freed pages become per-thread retention if the allocator holds them. */
static void *worker(void *arg) {
    (void)arg;
    enum { BATCH = 256 };
    void *v[BATCH];
    for (int r = 0; r < g_rounds; r++) {
        for (int i = 0; i < BATCH; i++) {
            size_t n = 64 + ((size_t)(i * 131) % g_maxsize);
            if (g_align > 0) {
                /* What Rust's GlobalAlloc does: every allocation carries an
                 * alignment from its Layout. SIMD tensor buffers are 32/64-byte
                 * aligned, so a global-allocator workload NEVER takes the plain
                 * malloc path that this probe used before. */
                if (posix_memalign(&v[i], g_align, n) != 0) v[i] = NULL;
            } else {
                v[i] = malloc(n);
            }
            if (!v[i]) { fprintf(stderr, "OOM\n"); abort(); }
            memset(v[i], 0x5A, n);      /* touch: make it resident */
        }
        for (int i = 0; i < BATCH; i++) free(v[i]);
    }
    return NULL;
}

int main(int argc, char **argv) {
    int nthreads = (argc > 1) ? atoi(argv[1]) : 8;
    g_rounds = (argc > 2) ? atoi(argv[2]) : 40;
    if (argc > 3) g_maxsize = (size_t)atol(argv[3]);
    if (argc > 4) g_align = (size_t)atol(argv[4]);
    if (nthreads < 1) nthreads = 1;
    if (g_maxsize < 64) g_maxsize = 64;

    pthread_t *t = calloc((size_t)nthreads, sizeof *t);
    if (!t) return 1;
    /* Long-lived and CONCURRENT: all threads coexist, so each keeps its own
     * heap alive for the whole run. Threads that come and go would instead
     * exercise abandonment, which is a different mechanism. */
    for (int i = 0; i < nthreads; i++) pthread_create(&t[i], NULL, worker, NULL);
    for (int i = 0; i < nthreads; i++) pthread_join(t[i], NULL);
    free(t);
    printf("%d threads ok\n", nthreads);
    return 0;
}
