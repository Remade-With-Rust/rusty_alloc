/* Thread-churn hazard probe.
 *
 * Targets the failure mode a broken per-thread heap slot produces: 640 threads
 * created and joined in waves, each writing a thread-unique byte pattern across
 * every block it owns and verifying the pattern before freeing. If two threads
 * ever share a heap, or a recycled TCB hands a new thread a dead thread's heap,
 * the pattern check fails loudly instead of corrupting silently.
 *
 * Built and run by bench/churn.sh under LD_PRELOAD.
 */
#include <pthread.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#define BLOCKS 64
#define ROUNDS 200
#define WAVES 40
#define PER_WAVE 16

static void *worker(void *arg) {
    long id = (long)arg;
    void *keep[BLOCKS];
    for (int r = 0; r < ROUNDS; r++) {
        for (int i = 0; i < BLOCKS; i++) {
            size_t n = 16 + ((i * 37 + r) % 4000);
            keep[i] = malloc(n);
            if (!keep[i]) {
                fprintf(stderr, "OOM t=%ld\n", id);
                abort();
            }
            memset(keep[i], (int)(id & 0xff), n);
        }
        for (int i = 0; i < BLOCKS; i++) {
            size_t n = 16 + ((i * 37 + r) % 4000);
            unsigned char *p = keep[i];
            for (size_t k = 0; k < n; k++) {
                if (p[k] != (unsigned char)(id & 0xff)) {
                    fprintf(stderr, "CORRUPT t=%ld off=%zu\n", id, k);
                    abort();
                }
            }
            free(keep[i]);
        }
    }
    return NULL;
}

int main(void) {
    for (int wave = 0; wave < WAVES; wave++) {
        pthread_t t[PER_WAVE];
        for (long i = 0; i < PER_WAVE; i++) {
            long id = i + (long)wave * PER_WAVE + 1;
            if (pthread_create(&t[i], NULL, worker, (void *)id) != 0) {
                fprintf(stderr, "pthread_create failed\n");
                return 1;
            }
        }
        for (int i = 0; i < PER_WAVE; i++) {
            pthread_join(t[i], NULL);
        }
    }
    printf("churn ok: %d threads\n", WAVES * PER_WAVE);
    return 0;
}
