// Deterministic C++17 over-aligned new/delete churn: the `align_val_t`
// operators, which a type declared `alignas(64)` (a cache-line-padded
// counter, a SIMD block) reaches on every `new`. Nothing else in the
// repository exercises them. Same two-point shape as sizedchurn.cpp:
// Ir/op = (Ir(2n) - Ir(n)) / n, argv[1] = n.
#include <cstdlib>

struct alignas(64) Line { unsigned char b[64]; };
struct alignas(32) Pair { double a[4]; };

int main(int argc, char** argv) {
    const long iters = (argc > 1) ? atol(argv[1]) : 200000;
    Line* keep[64] = {};
    for (long k = 0; k < iters; k++) {
        Line* l = new Line;
        l->b[0] = (unsigned char)k;
        Pair* p = new Pair[3];
        p[0].a[0] = (double)k;
        delete[] p;
        int s = (int)(k & 63);
        delete keep[s];
        keep[s] = l;
    }
    for (auto* l : keep) delete l;
    return 0;
}
