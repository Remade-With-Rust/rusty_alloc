# An unbacked page reached `page_extend` on Windows, and a re-commit whose failure is ignored

**Status:** one crash in a shipping consumer, **not reproduced**; mechanism A
**fixed in the working tree 2026-10-04, uncommitted** (section 8); which of the
two mechanisms caused the crash is still **open** ·
**Date:** 2026-10-04 · **Seen on:** 1.1.6 (`secure` on, `purge_delay = 0`) ·
**Code checked against:** HEAD `86980ec` (2.2.2) · **Platform:** Windows 11,
x86-64 · **From:** the MATA desktop session (`mata-master`, branch
`hc-0-connectivity`)

---

## 0. The answer in one paragraph

The MATA desktop app died with an access violation **writing** a heap address
inside `page_extend`: the allocator handed out a page whose memory was not
backed. The app runs with immediate purging and decommit, and the machine was
1.9 GB from its commit limit. There are two candidate mechanisms and the
evidence does not choose between them. **(A)** `span_recommit` discards the
result of `os::commit`, and on Windows a commit can fail, so a span whose
re-commit failed is used anyway. That is a defect in the source whether or not
it caused this crash. **(B)** A purged span reached a reuse path that does not
re-commit it at all, which is the family `docs/LEDGER.md` records under M8.
Section 5 says what to change for (A) and section 6 what would tell the two
apart.

## 1. What was seen

| | |
|---|---|
| Process | `desktop.exe` (MATA desktop, debug build), global allocator `rusty_alloc` 1.1.6 through `mata-alloc` |
| Configuration | `secure` feature on; `mata_alloc::configure(Profile::LongLived)` sets `purge_delay = 0`; `purge_decommits` left at its default, 1 |
| When | 2026-10-04 14:16:21 local, about seven minutes after launch |
| Exception | `0xC0000005`, access violation **writing** `0x0000017ab8310d80` |
| Faulting code | `desktop.exe+0x412f3` = `rusty_alloc::page::block_set_next` (`page.rs:309`) |
| System memory when the dump was written | commit charge 125.8 GB, limit 127.7 GB, peak 128.4 GB; 1.7 GB of 31.7 GB physical memory available |
| Process memory when the dump was written | 2.61 GB committed (pagefile usage), 0.13 GB working set |

The call path, innermost first. It is an ordinary small allocation, a `String`
being built by `serde_json`:

```
rusty_alloc::page::block_set_next          page.rs:309
rusty_alloc::page::page_extend             page.rs:1113
rusty_alloc::heap::Heap::malloc_generic_walk
rusty_alloc::heap::Heap::malloc_generic    heap.rs:457
rusty_alloc::alloc::malloc_slow            alloc.rs:322
rusty_alloc::alloc::malloc                 alloc.rs:222
rusty_alloc_api::...::alloc                rusty_alloc-api lib.rs:146
alloc::str::to_owned  <-  serde_json parse_str  <-  SyncEntry::deserialize
```

The fault address is a normal heap address, not null and not a wild value:
`rax = rdx = 0x17ab8310d80`, the block being linked, and `rcx = 0`.

**Method.** The Windows Application event log (events 1000 and 1001) gave the
exception code and offset. `llvm-symbolizer --relative-address` against the
binary and its PDB resolved the offset. The local crash dump
(`%LOCALAPPDATA%\CrashDumps\desktop.exe.12424.dmp` on the machine that ran it)
gave the faulting address, the registers, the stack and the memory figures:
exception stream 6, thread list stream 3, system memory stream 21, process
counters stream 22. The stack listing is a scan of the faulting thread's stack
for return addresses inside the image, so it can include a stale frame; the
frames above are the ones that form a consistent chain. The source lines are
1.1.6's, read from the cargo registry copy.

**The previous build.** The build before this one, with the allocator and its
configuration unchanged, ran for 31 minutes on the same machine an hour earlier
and was closed normally. So this is not a crash on every run.

## 2. Mechanism A: the re-commit result is dropped

Line numbers are 1.1.6 / HEAD.

1. **Purging is on and it decommits.** `purge_delay = 0` makes a freed span
   purge at once. With `purge_decommits = 1`, `os::purge` calls `decommit`,
   which on Windows is `VirtualFree(MEM_DECOMMIT)` (`prim/windows.rs:163` /
   `:173`). The span's first page is marked `purged`.
2. **Reuse re-commits, and ignores the answer.** `span_recommit`
   (`segment.rs:768` / `:843`; called from the first-fit loop at `:663` in
   HEAD):

   ```rust
   (*seg).pages[idx].purged = false;
   let area = page_area(seg, idx);
   let _ = os::commit(area, len * SEGMENT_SLICE_SIZE);
   ```

   `os::commit` returns `Result<bool, PrimError>`. On Windows it is
   `VirtualAlloc(ptr, size, MEM_COMMIT, PAGE_READWRITE)` (`prim/windows.rs:152`
   / `:153`). Windows has no overcommit, so that call fails when the system has
   no commit left. The `purged` flag has already been cleared, so nothing
   remembers that the span is not backed.
3. **The page layer writes to it.** `page_extend` (`page.rs:1113`) links the
   page's fresh blocks with `block_set_next`. The first store lands on reserved,
   uncommitted memory.

Two more call sites drop the same result, on the path that hands a whole
segment back for re-tenanting: `segment.rs:394` and `:905` in 1.1.6, `:426` and
`:1043` in HEAD. Both also drop the result of `os::protect(base, bytes, false)`
on the line before.

## 3. Mechanism B: a reuse path that does not re-commit

`docs/LEDGER.md` (around lines 2910 and 3100 at HEAD) describes this family:
the M8 P0 was guard pages recycled while still no-access, and a later forced
purge "reaches spans whose reuse path does not re-commit them — the M8 defect
exactly (Windows `MEM_DECOMMIT` faults on touch)". The fault here has that
shape too. `span_recommit` reads the `purged` flag of the span's **first**
page only, so any way for a decommitted range to sit behind a first page whose
flag is clear would skip the commit. That was not traced for this note: no such
path has been shown, and none has been ruled out. With `secure` on, a guard
page left no-access is a third way to the same fault.

## 4. What is established and what is not

**Established:**
- The exception, its address and its kind (a write), the function it happened
  in, and the call path.
- The consumer runs with immediate purging and decommit.
- The three `let _ = os::commit(...)` sites exist in 1.1.6 and in HEAD.
- The system's commit charge was 98.5% of its limit when the dump was written,
  and had exceeded the current limit earlier (peak above limit).

**Not established:**
- Which mechanism it was. The dump has no memory-region list, so the state of
  the faulting page is not recorded, and nothing logs the `VirtualAlloc` result.
- For A: 1.9 GB of commit was still free when the dump was written, and a span
  re-commit is far smaller than that. A needs the headroom to have been gone at
  the instant of the call. The machine was being pushed to its limit by other
  processes throughout, so that is plausible and unproven.
- For B: nothing beyond the shape of the fault and the family's history.

## 5. Suggested change for A

Treat a failed re-commit as a failed allocation.

- `span_recommit` returns whether the span is backed. On failure it leaves
  `purged` set and the caller does not use the span: put it back on the free
  list and either try the next span or fail the allocation. The public entry
  then returns null, and Rust's `handle_alloc_error` ends the process with
  "memory allocation of N bytes failed".
- The two re-tenanting sites do the same: a segment that cannot be made
  accessible and committed again is released, not handed on.
- The `os::protect(.., false)` results on those paths get the same treatment,
  because a span left no-access fails in the same way.

The process still ends when the machine is truly out of memory. What changes:
the report names the real cause, callers that allocate fallibly
(`try_reserve`) get the null they asked for, and the allocator no longer hands
out memory it does not have.

## 6. Tests, and telling A from B

- **Mock prim.** `prim/mock.rs` makes commit a bookkeeping no-op that always
  succeeds. Add a switch that makes the next N commits fail, and have the mock
  track which ranges are decommitted so a write to one is detectable. Then:
  allocate, free with `purge_delay = 0`, fail the next commit, allocate again.
  Today that returns a pointer into a decommitted range; it should return null.
  The same range tracking catches B without any commit failing: every pointer
  handed out must lie in a committed range.
- **Windows, real pages.** A Job Object with `JOB_OBJECT_LIMIT_PROCESS_MEMORY`
  set just above the test's current commit makes `MEM_COMMIT` fail on demand
  without exhausting the machine. Same sequence; expect null and no access
  violation.
- **In the field.** A counter of failed commits, readable through the stats
  interface, would settle it on the next crash: non-zero means A. The MATA app
  can also be made to write dumps with the memory-region list
  (`MiniDumpWithFullMemoryInfo`), which records whether the faulting page was
  reserved or no-access.

## 7. Who is exposed

- A: any consumer on Windows with purging on (`purge_delay >= 0`) and
  `purge_decommits = 1`, when the machine runs out of commit. The process does
  not have to be large: this one held 2.6 GB.
- B, if it exists: the same configuration, at any time.
- The default configuration (`purge_delay = -1`) never sets `purged`, so
  `span_recommit` returns early. `mata-alloc` turns purging on in
  `configure(Profile::LongLived)`, which the MATA desktop app calls.
- Unix was not examined for this note.

## 8. What was changed for A (2026-10-04, uncommitted)

- **`span_recommit` returns whether the span is backed** (`segment.rs`). On a
  failed `os::commit` it leaves `purged` set, counts the failure, and
  `span_alloc` returns null with the span still on the free list. Both callers
  in `heap.rs` already treat null as "this segment has no room" and move on; a
  fresh segment then fails its own commit as an ordinary allocation failure.
- **`restore_for_reuse`** (`segment.rs`) is the guard-lift + re-commit that
  `segment_free` and `huge_free` each did inline with both results dropped.
  When either step fails, the segment is not recycled: released to the OS if
  it is a reservation of its own (release needs neither step), retired in
  place, its used bit left set, if it is chunks of an arena (arena memory is
  committed once at reservation and handed out as-is, and a chunk cannot be
  released alone). `arena::owns` tells the two apart. `huge_free` restores only
  chunk-multiple segments, as before.
- **`stats::commit_failures()`**, a process-wide counter, also printed by
  `print_process` as `failed commits N`. Non-zero after a fault in the page
  layer means the machine ran out of commit (A); zero points at B.
- **Test:** `segment::recommit_tests::a_failed_recommit_is_a_failed_span_not_an_unbacked_one`,
  through a `cfg(test)`-only, thread-local "fail the next N commits" switch in
  `os::commit` (the miri-only mock in `prim/mock.rs` would not reach native
  runs, and on Windows the purge is a real `MEM_DECOMMIT`, which is what makes
  the test reproduce the fault). **Poisoned:** with the check in `span_alloc`
  disabled, the test fails at "a span whose re-commit failed was handed out".
- Verified on Windows: the lib and 10 integration suites pass (88 tests),
  `secure` on as well (71), clippy `-D warnings` with `secure`, `cargo fmt`,
  `cargo check --target wasm32-unknown-unknown`, and the unsafe census (965 ->
  973, recorded in `UNSAFE.md`: +3 shipped, +5 test-only).

**Not covered by a test:** the two release sites. Exercising them needs a
whole segment freed while purged, with the failure injected on that path; the
logic is small and reviewed, but it has not been reproduced.

**Still open:** B. The counter and a `MiniDumpWithFullMemoryInfo` dump from
the MATA app are what will tell A from B on the next crash.

**Context on the crash itself:** at 14:16 the same machine was also running
the `bench/alloc-eval` replays of a recorded Endless Sky battle in WSL (about
2 GB per run), and the WSL VM restarted twice around then from host memory
exhaustion. That load very likely contributed to the 98.5 % commit charge the
dump recorded, which makes A's precondition more plausible than section 4
assumed. It does not rule out B.
