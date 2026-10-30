/* Per-thread floor probe: anonymous memory per LIGHT thread.
 *
 * rusty_WRT's shape (docs/plans/shared-light-heap.md): T threads each keep 3
 * live blocks in each of 15 size classes from 16 B to 4 KiB, then wait at a
 * barrier while the main thread reads Anonymous from /proc/self/smaps_rollup.
 * Sweep T and read the slope: KiB per thread. Thread stacks are the same in
 * every arm, so the arms differ only by the allocator.
 *
 * usage: light-threads <threads>      (prints "<threads> <anon_kib>")
 */
#include <pthread.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

static const size_t SIZES[15] = {16, 32, 48, 64, 96, 128, 192, 256, 384, 512, 768, 1024, 1536, 2048, 4096};
static pthread_barrier_t ready, done;

static void *worker(void *arg) {
    (void)arg;
    void *v[45];
    for (int c = 0; c < 15; c++)
        for (int k = 0; k < 3; k++) {
            v[c * 3 + k] = malloc(SIZES[c]);
            if (!v[c * 3 + k]) abort();
            memset(v[c * 3 + k], 0x5A, SIZES[c]);
        }
    pthread_barrier_wait(&ready); /* main reads smaps now */
    pthread_barrier_wait(&done);
    for (int i = 0; i < 45; i++) free(v[i]);
    return NULL;
}

static long anon_kib(void) {
    FILE *f = fopen("/proc/self/smaps_rollup", "r");
    char line[256];
    long kib = -1;
    if (!f) return -1;
    while (fgets(line, sizeof line, f))
        if (sscanf(line, "Anonymous: %ld kB", &kib) == 1) break;
    fclose(f);
    return kib;
}

int main(int argc, char **argv) {
    int n = argc > 1 ? atoi(argv[1]) : 4;
    pthread_t t[256];
    if (n < 0 || n > 256) return 2;
    pthread_barrier_init(&ready, NULL, (unsigned)n + 1);
    pthread_barrier_init(&done, NULL, (unsigned)n + 1);
    for (int i = 0; i < n; i++) pthread_create(&t[i], NULL, worker, NULL);
    pthread_barrier_wait(&ready);
    printf("%d %ld\n", n, anon_kib());
    pthread_barrier_wait(&done);
    for (int i = 0; i < n; i++) pthread_join(t[i], NULL);
    return 0;
}
