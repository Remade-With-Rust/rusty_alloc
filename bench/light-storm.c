/* Shared-phase contention probe: the WORST case for the shared light heap.
 *
 * Repeated storms of T threads started together, each making 256
 * allocations of 64..512 bytes (about 64 KiB, the whole shared phase, so
 * every allocation is a locked one), freeing them and exiting. Under the
 * default build every thread instead builds its own heap. Time it.
 *
 * usage: light-storm <threads> <storms>
 */
#include <pthread.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

static pthread_barrier_t go;

static void *worker(void *arg) {
    void *v[256];
    size_t salt = (size_t)arg;
    pthread_barrier_wait(&go);
    for (int i = 0; i < 256; i++) {
        size_t n = 64 + ((size_t)i * 37 + salt * 11) % 449;
        v[i] = malloc(n);
        if (!v[i]) abort();
        memset(v[i], 1, n);
    }
    for (int i = 0; i < 256; i++) free(v[i]);
    return NULL;
}

int main(int argc, char **argv) {
    int n = argc > 1 ? atoi(argv[1]) : 32, storms = argc > 2 ? atoi(argv[2]) : 100;
    pthread_t t[256];
    if (n < 1 || n > 256) return 2;
    for (int s = 0; s < storms; s++) {
        pthread_barrier_init(&go, NULL, (unsigned)n);
        for (int i = 0; i < n; i++) pthread_create(&t[i], NULL, worker, (void *)(size_t)i);
        for (int i = 0; i < n; i++) pthread_join(t[i], NULL);
        pthread_barrier_destroy(&go);
    }
    return 0;
}
