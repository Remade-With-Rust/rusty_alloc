# Shared light heap — small allocations of lightly-used threads share one heap

Status: PROTOTYPE behind `--cfg ra_shared_light_heap` (off by default).

## The gap

Every thread owns a heap, and a heap serves each size class from its own
page, so a thread's floor is about one OS page per size class it touches,
whatever it keeps live. rusty_WRT measured it on Linux (x86_64 musl, 4 KiB
pages, 3 live blocks in each of 15 classes from 16 B to 4 KiB):

| allocator | KiB per thread |
|---|---|
| rusty_alloc 2.2.7, default geometry | ~97 |
| rusty_alloc 2.2.7, `ra_small_profile` | ~94 |
| musl malloc | ~44 |

Freeing half the blocks and calling `collect(true)` returned nothing: every
class still holds a live block. Collapsing odhcpd from 5 threads to 1 saved
450 KiB of PSS. Long-lived daemon threads (a netlink listener, a timer, a ubus
thread) touch 10–15 classes and then sit idle, so the floor is all cost.

## Design

A thread starts LIGHT. Until it is promoted, its small allocations come from
one process-wide SHARED heap behind a lock.

1. **The fast path does not change.** A light thread's TLS heap stays the
   empty sentinel, so every allocation it makes misses and reaches the
   first-allocation slow path (`malloc_first` / `zalloc_first`), which is
   where the shared-or-own decision is made.
2. **Frees do not change.** Segments the shared heap allocates or adopts are
   stamped with a reserved owner id, `SHARED_TID`, which matches no thread.
   Every free of a shared block therefore fails the owner compare and takes
   the existing cross-thread route (the page's `xthread_free` list, or the
   shared box's delayed list for a full page), which the shared heap drains
   under its lock. The heap applies the stamp itself
   (`shared_light::stamp_if_shared`) to every segment it creates or adopts,
   keyed on the HEAP, not on a thread flag: a first draft keyed it on "this
   thread is running the shared heap", which would have stamped a heap that a
   re-entrant hook created mid-operation as shared.
3. **Big allocations never go shared.** Only sizes up to `SMALL_OBJ_SIZE_MAX`
   (8 KiB default, 512 B under `ra_small_profile`) are served there. Any larger
   request, any aligned request, and any first-class-heap operation promotes
   the thread on the spot.
4. **Promotion is one-way, on bytes allocated over the thread's life.** Once a
   light thread has allocated `SHARED_LIGHT_BYTES` (64 KiB) from the shared
   heap it gets its own heap and never returns. A busy thread is promoted
   within its first few hundred allocations.
5. **Excluded threads:** the FIRST thread that ever allocates (in practice
   the main thread, the one most likely to be busy), a thread in a
   non-default subprocess (adoption reads the caller's subprocess tag), and
   any re-entry while the thread already holds the shared lock (a hook that
   allocates). All get their own heap at once. The first-thread rule came
   from rusty_WRT's boot census (Results): without it a busy main thread
   started light, promoted, and paid for pages in both heaps, and a
   single-threaded process paid for the shared heap at all.
6. **Contention promotes.** A light thread spins at most `SPIN_LIMIT` (128)
   for the lock and then takes its own heap. Finding the lock taken means
   allocating at the same moment as another thread, which is what busy
   threads do. The first draft waited (spin, then yield); 32 threads storming
   the shared phase together then ran 3.2x slower than per-thread heaps. The
   bound also means no allocation ever waits unboundedly on this lock,
   including on one whose holder `ExitProcess` killed (the 2.2.7 hang class),
   so the draft's `ExitProcess` escape is gone.
7. `collect(true)` on a light thread collects the shared heap (try-lock)
   rather than creating a heap only to collect it.

Blocks a thread allocated while light stay in shared pages after it is
promoted. They are freed normally and their pages retire on the shared
heap's next generic pass or collect.

**A thread can change heaps mid-life, at a moment chosen by contention.** A
block it freed while light is recycled by the shared heap, for whichever
light thread asks next, and not necessarily for it. Nothing in the malloc
contract promises otherwise, but one existing test did assume it:
`alloc_core::block_reuse_same_size_class` failed 2 times in 60 under the flag
in the small profile, and a diagnostic showed the test thread starting on the
sentinel and ending on a real heap each time. That test now promotes its
thread first.

## Gates

| gate | how | pass |
|---|---|---|
| Flag-off identity | opscan Ir/op, main vs branch, no flag | identical on every op |
| Busy threads, flag on | opscan Ir/op, flag on vs off | identical (promotion is start-up, cancelled by 2N−N) |
| Real programs, flag on | icount lua / perl / sqlite | within the run-to-run Ir noise |
| Footprint | `bench/light-threads.c`, T = 0..8, smaps Anonymous | per-thread slope well below 97 KiB |
| Big allocations never shared | test over every size class | nothing above `SMALL_OBJ_SIZE_MAX` in a `SHARED_TID` segment |
| Promotion happens | test: a thread allocating past the threshold owns a heap | yes, and its later blocks are in its own segments |
| Pages come back | test: free every light block, collect | shared pages retire |
| Cross-thread frees | stress: light threads freeing each other's blocks | full suite + new stress clean |
| Exit with the shared lock held | `exit_hang` variant | process exits |
| Both geometries | full suite with and without `ra_small_profile` | green |

## Results (2026-10-30, prototype at the branch head)

x86_64 glibc under WSL2; instruction counts are callgrind; "main" is
`5807296`.

**Flag off: byte-identical machine code.** The override library built from
the branch without the cfg has `.text` (310,963 bytes), `.rodata`, `.data`,
`.tdata` and `.eh_frame` identical to main's. `.data.rel.ro` differs in nine
words, all panic-location line numbers moved by the inserted lines (+8 in
`heap.rs`, +29 in `init.rs`).

**Footprint** (`bench/light-threads.c`, Anonymous KiB, median of 5):

| arm | T=0 | T=16 | KiB per thread |
|---|---|---|---|
| glibc | 100 | 804 | 44.0 |
| main | 164 | 1772 | 100.5 |
| flag on, waiting lock (first draft) | 168 | 912 | 46.5 |
| flag on, contention promotes | 168 | 1084 | 57.3 |
| flag on, + first thread never light | 168 | 1092 | 57.8 |

The probe starts all its threads at once, so with contention-promotes some
collide and promote; daemon threads that start one after another stay
nearer the first-draft figure. With the first-thread rule, one thread costs
what main does (T=1: 272 KiB against main's 268).

**rusty_WRT boot census** (real daemons, x86_64 musl, Anonymous PSS KiB, two
boots per arm, at `2122d4e`, i.e. BEFORE the first-thread rule):

| daemon | threads | flag off | flag on | |
|---|---|---|---|---|
| procd | 3 | 756 / 752 | 656 / 660 | −96 |
| odhcpd | 1 | 240 / 240 | 312 / 312 | +72 |
| logd | 1 | 132 / 132 | 160 / 160 | +28 |
| dnsmasq | 1 | 108–112 | 108–112 | 0 |
| system | | 7022 / 7040 | 7037 / 7029 | neutral |

The single-threaded daemons paid for the shared heap and then for their own
heap after promoting, which cancelled the multi-threaded win. That is what
the first-thread rule removes: re-measured, single-threaded perl and sqlite
are within 200 Ir of the flag-off build (−181 and −196 on 772 M and 316 M).
The census has not been re-run with the rule yet.

**Busy threads** (`bench/opscan.sh` method, Ir/op, flag on vs main): identical
on 12 of 13 ops; `mixed` +0.01 Ir/op (0.01 %), consistent with the shared
phase shifting where periodic collects land. Not verified further.

**Real programs** (whole-program Ir, flag on vs main): perl +4,020
(+0.0005 %), sqlite +2,821 (+0.0009 %), both deterministic; lua within its
own run-to-run noise (1.7 M Ir between two main runs, because its hash seed
follows allocation addresses).

**Cost of the shared phase** (`bench/light-storm.c`, one thread per storm, so
uncontended; allocator Ir only): an allocation plus its free costs 96.7 Ir
against main's 35.8, which also pays a per-thread heap build and teardown.
That is paid until promotion: about 15.6 K Ir for a 256-allocation thread,
about 250 K Ir at worst (64 KiB of 16-byte blocks), once per thread.

**Contention, wall clock:** NOT RESOLVED. With the waiting lock, storms of 4
and 32 threads read 1.37x and 3.2x slower than main, against null arms of
1.07x and 1.62x, which is why contention now promotes. Re-measured after the
change, the null arm (main against main) read 1.25x and 2.66x: the host was
at 58 % CPU from other work, and the clock could not resolve the effect.
Needs a quiet box.

**Gate tests** (`tests/shared_light_heap.rs`, both geometries): small requests
shared; nothing above `SHARED_MAX_SIZE` shared (every size from cutoff−64 to
cutoff+64, and medium, large and huge); promotion at the threshold; aligned
requests promote; contention promotes; 8 light threads sharing pages, freed
by another thread, pages return to zero; a 16-thread cross-free stress; exit
with the lock held by a thread `ExitProcess` killed. The exit test was
checked against a build with its escape disabled: the child hung and the
test failed.

## Not done in the prototype

- loom model of the shared lock + remote free into a `SHARED_TID` page.
- Wall-clock contention on a quiet box (above).
- rusty_WRT's boot census re-run with the first-thread rule.
- Giving a promoted thread's shared pages back (or letting it adopt them):
  a busy SECOND thread still pays for pages in both heaps after it
  promotes, bounded by the 64 KiB it took while light.
- A runtime switch: the threshold and spin limit are constants.
