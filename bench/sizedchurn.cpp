// Deterministic stand-in for larson-sized.
//
// larson-sized is wall-clock bounded: its workers loop until a timer fires and
// RESPAWN when their block quota runs out, so the amount of work it does is an
// output of how fast the machine (and the allocator) happens to be. Under
// callgrind the two arms therefore complete different numbers of operations
// and no aggregate comparison means anything.
//
// This reproduces the same allocator work with a FIXED iteration count: N live
// blocks, each step picking a random victim, releasing it through C++ SIZED
// delete (which is what "larson-sized" exercises and what an ordinary
// benchmark does not), and replacing it with a fresh random size drawn from
// larson's own 8..1000 range. One byte is written to each new block, as larson
// does, so the page is really touched.
//
// Iteration count is argv[1] so the harness can use the repo's two-point
// estimator, Ir/op = (Ir(2n) - Ir(n)) / n, which cancels startup and the
// initial fill exactly.
#include <cstddef>
#include <cstdlib>

static unsigned long long rng_state = 4141ULL;

static inline unsigned long long xrand() {
    rng_state = rng_state * 6364136223846793005ULL + 1442695040888963407ULL;
    return rng_state >> 33;
}

int main(int argc, char** argv) {
    const int N = 5000;                                     // larson chperthread
    const long iters = (argc > 1) ? atol(argv[1]) : 500000; // two-point knob
    const size_t lo = 8, hi = 1000;                         // larson min/max

    char** a = new char*[N];
    size_t* sz = new size_t[N];

    for (int i = 0; i < N; i++) {
        size_t b = lo + (size_t)(xrand() % (hi - lo));
        a[i] = new char[b];
        sz[i] = b;
    }

    for (long k = 0; k < iters; k++) {
        int v = (int)(xrand() % (unsigned long long)N);
        operator delete[](a[v], sz[v]);
        size_t b = lo + (size_t)(xrand() % (hi - lo));
        a[v] = new char[b];
        sz[v] = b;
        volatile char* c = a[v];
        *c = 'a';
    }

    for (int i = 0; i < N; i++) {
        operator delete[](a[i], sz[i]);
    }
    delete[] a;
    delete[] sz;
    return 0;
}
