//! Trace generation + replay with G1 invariant checking (plan §4).
//!
//! `gen` produces deterministic synthetic traces (cfrac-like small-alloc churn)
//! until real recorded traces land. `replay --check` executes a trace against
//! an allocator arm and verifies, per op:
//! G1a alignment (≥ 8, and the requested alignment)
//! G1b usable_size(p) ≥ requested size
//! G1c zalloc memory reads zero
//! G1d canary: block contents survive until free (fill at alloc, verify at free)
//! G1e no overlap with any live block (interval check)

use std::collections::BTreeMap;
use std::io::Write;

use crate::trace::{Op, Reader, Record, Writer};

/// Which allocator executes the trace.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Arm {
    /// rusty_alloc (the crate under test).
    Rusty,
    /// The process allocator (std::alloc) — harness sanity arm.
    System,
}

impl Arm {
    /// Parse an arm name.
    pub fn parse(s: &str) -> Option<Arm> {
        match s {
            "ra" | "rusty" => Some(Arm::Rusty),
            "sys" | "system" => Some(Arm::System),
            _ => None,
        }
    }
}

fn arm_alloc_aligned(arm: Arm, size: usize, zero: bool, align: usize) -> *mut u8 {
    match arm {
        Arm::Rusty => match (zero, align > 8) {
            (false, false) => rusty_alloc::alloc::malloc(size),
            (true, false) => rusty_alloc::alloc::zalloc(size),
            (false, true) => rusty_alloc::alloc::malloc_aligned(size, align),
            (true, true) => rusty_alloc::alloc::zalloc_aligned(size, align),
        },
        Arm::System => {
            let l = std::alloc::Layout::from_size_align(size.max(1), align).unwrap();
            // SAFETY: valid non-zero layout.
            unsafe {
                if zero {
                    std::alloc::alloc_zeroed(l)
                } else {
                    std::alloc::alloc(l)
                }
            }
        }
    }
}

/// # Safety
/// `p` came from `arm_alloc_aligned(arm, size, _, align)` and is freed once,
/// with the allocation's alignment.
unsafe fn arm_free_aligned(arm: Arm, p: *mut u8, size: usize, align: usize) {
    match arm {
        Arm::Rusty => {
            // SAFETY: forwarded contract.
            unsafe { rusty_alloc::alloc::free(p) }
        }
        Arm::System => {
            let l = std::alloc::Layout::from_size_align(size.max(1), align).unwrap();
            // SAFETY: forwarded contract; same layout as at alloc.
            unsafe { std::alloc::dealloc(p, l) }
        }
    }
}

fn arm_usable(arm: Arm, p: *const u8, size: usize) -> usize {
    match arm {
        // SAFETY: p is live (called between alloc and free).
        Arm::Rusty => unsafe { rusty_alloc::alloc::usable_size(p) },
        Arm::System => size, // std has no usable_size; identity keeps G1b trivial
    }
}

struct Live {
    ptr: usize,
    size: usize,
    align: usize,
    canary: u8,
}

/// Replay `path` on `arm`; returns (ops, live_at_end) or an error string.
pub fn replay(path: &str, arm: Arm, check: bool) -> Result<(u64, usize), String> {
    let stats0 = rusty_alloc::alloc::stats();
    let f = std::fs::File::open(path).map_err(|e| format!("{path}: {e}"))?;
    let mut r = Reader::new(std::io::BufReader::new(f)).map_err(|e| format!("{path}: {e}"))?;
    // block id → live allocation; BTreeMap doubles as the overlap oracle.
    let mut live: BTreeMap<u64, Live> = BTreeMap::new();
    let mut by_addr: BTreeMap<usize, (usize, u64)> = BTreeMap::new(); // addr → (size, id)
    let mut ops = 0u64;

    macro_rules! fail {
        ($($t:tt)*) => { return Err(format!($($t)*)) };
    }

    while let Some(rec) = r.next_record().map_err(|e| e.to_string())? {
        ops += 1;
        match rec.op {
            Op::Malloc | Op::Zalloc => {
                let zero = rec.op == Op::Zalloc;
                let size = rec.size as usize;
                let align = if rec.align_log2 > 0 {
                    1usize << rec.align_log2
                } else {
                    8
                };
                let p = arm_alloc_aligned(arm, size, zero, align);
                if p.is_null() {
                    fail!("op {ops}: allocation of {size} (align {align}) failed");
                }
                let addr = p as usize;
                if check {
                    // G1a alignment
                    if !addr.is_multiple_of(align) {
                        fail!("op {ops}: G1a alignment {align} violated (addr {addr:#x})");
                    }
                    // G1b usable
                    let us = arm_usable(arm, p, size);
                    if us < size {
                        fail!("op {ops}: G1b usable {us} < size {size}");
                    }
                    // G1e overlap: nearest live block below must end before us,
                    // and we must end before the next one above.
                    if let Some((&a, &(s, id))) = by_addr.range(..=addr).next_back()
                        && a + s.max(1) > addr
                    {
                        fail!("op {ops}: G1e overlap with live block {id} at {a:#x}+{s}");
                    }
                    if let Some((&a, &(_, id))) = by_addr.range(addr + 1..).next()
                        && addr + size.max(1) > a
                    {
                        fail!("op {ops}: G1e overlap with live block {id} at {a:#x}");
                    }
                    // G1c zero
                    if zero {
                        // SAFETY: p is a live block of ≥ size bytes.
                        let bytes = unsafe { std::slice::from_raw_parts(p, size) };
                        if bytes.iter().any(|&b| b != 0) {
                            fail!("op {ops}: G1c zalloc returned non-zero memory");
                        }
                    }
                    // G1d canary fill (id-derived pattern)
                    let canary = (rec.ptr as u8) | 1;
                    // SAFETY: p is a live block of ≥ size bytes.
                    unsafe { std::ptr::write_bytes(p, canary, size) };
                    live.insert(
                        rec.ptr,
                        Live {
                            ptr: addr,
                            size,
                            align,
                            canary,
                        },
                    );
                    by_addr.insert(addr, (size, rec.ptr));
                } else {
                    live.insert(
                        rec.ptr,
                        Live {
                            ptr: addr,
                            size,
                            align,
                            canary: 0,
                        },
                    );
                }
            }
            Op::Free => {
                let Some(l) = live.remove(&rec.old_ptr) else {
                    fail!("op {ops}: free of unknown block {}", rec.old_ptr);
                };
                if check {
                    // G1d canary verify
                    // SAFETY: block is still live until the free below.
                    let bytes = unsafe { std::slice::from_raw_parts(l.ptr as *const u8, l.size) };
                    if bytes.iter().any(|&b| b != l.canary) {
                        fail!("op {ops}: G1d canary corrupted in block {}", rec.old_ptr);
                    }
                    by_addr.remove(&l.ptr);
                }
                // SAFETY: tracked live block, freed exactly once here.
                unsafe { arm_free_aligned(arm, l.ptr as *mut u8, l.size, l.align) };
            }
            Op::Realloc => {
                let Some(l) = live.remove(&rec.old_ptr) else {
                    fail!("op {ops}: realloc of unknown block {}", rec.old_ptr);
                };
                let newsize = rec.size as usize;
                let np = match arm {
                    // SAFETY: tracked live block; invalidated on move.
                    Arm::Rusty => unsafe { rusty_alloc::alloc::realloc(l.ptr as *mut u8, newsize) },
                    Arm::System => {
                        let ol = std::alloc::Layout::from_size_align(l.size.max(1), 8).unwrap();
                        // SAFETY: tracked live block with its original layout.
                        unsafe { std::alloc::realloc(l.ptr as *mut u8, ol, newsize.max(1)) }
                    }
                };
                if np.is_null() {
                    fail!("op {ops}: realloc({}) to {newsize} failed", rec.old_ptr);
                }
                let addr = np as usize;
                if check {
                    // G1f: the preserved prefix must carry the old canary.
                    let keep = l.size.min(newsize);
                    // SAFETY: np is live with ≥ newsize usable bytes.
                    let bytes = unsafe { std::slice::from_raw_parts(np, keep) };
                    if bytes.iter().any(|&b| b != l.canary) {
                        fail!("op {ops}: G1f realloc lost prefix of block {}", rec.old_ptr);
                    }
                    let us = arm_usable(arm, np, newsize);
                    if us < newsize {
                        fail!("op {ops}: G1b usable {us} < newsize {newsize}");
                    }
                    by_addr.remove(&l.ptr);
                    let canary = (rec.ptr as u8) | 1;
                    // SAFETY: np live with ≥ newsize bytes.
                    unsafe { std::ptr::write_bytes(np, canary, newsize) };
                    live.insert(
                        rec.ptr,
                        Live {
                            ptr: addr,
                            size: newsize,
                            align: 8,
                            canary,
                        },
                    );
                    by_addr.insert(addr, (newsize, rec.ptr));
                } else {
                    live.insert(
                        rec.ptr,
                        Live {
                            ptr: addr,
                            size: newsize,
                            align: 8,
                            canary: 0,
                        },
                    );
                }
            }
            Op::ThreadStart | Op::ThreadEnd => {} // single-threaded replay in M2
        }
    }
    // Free the tail so arms can be looped.
    for (_, l) in live.iter() {
        // SAFETY: still-live tracked blocks, freed once.
        unsafe { arm_free_aligned(arm, l.ptr as *mut u8, l.size, l.align) };
    }
    // Strict leak gate (ra arm only — counters are ours): every alloc this
    // replay caused must have been freed, including realloc moves.
    if check && arm == Arm::Rusty {
        let s = rusty_alloc::alloc::stats();
        let (da, df) = (s.allocs - stats0.allocs, s.frees - stats0.frees);
        if da != df {
            return Err(format!("G1g leak: {da} allocs vs {df} frees this replay"));
        }
        println!(
            "COUNTERS: allocs={da} frees={df} generic={} pages_fresh={} retired={} segs={}(-{}) large={} huge={} realloc(in-place/moved)={}/{}",
            s.generic - stats0.generic,
            s.pages_fresh - stats0.pages_fresh,
            s.pages_retired - stats0.pages_retired,
            s.segments - stats0.segments,
            s.segments_freed - stats0.segments_freed,
            s.large_allocs - stats0.large_allocs,
            s.huge_allocs - stats0.huge_allocs,
            s.realloc_in_place - stats0.realloc_in_place,
            s.realloc_moved - stats0.realloc_moved,
        );
    }
    Ok((ops, live.len()))
}

/// Deterministic xorshift.
pub struct Rng(pub u64);

impl Rng {
    /// Next raw value.
    pub fn next_u64(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    /// Uniform in [0, n).
    pub fn below(&mut self, n: usize) -> usize {
        (self.next_u64() % n as u64) as usize
    }
}

/// cfrac-ish size distribution: mostly tiny, occasional medium, rare large.
fn synth_size(rng: &mut Rng) -> usize {
    match rng.below(1000) {
        0..=799 => 8 + rng.below(120),            // 8..128 B — the bulk
        800..=949 => 128 + rng.below(896),        // 128..1 KiB
        950..=979 => 1024 + rng.below(63 * 1024), // 1..64 KiB (medium pages)
        980..=997 => 64 * 1024 + rng.below(4 * 1024 * 1024), // large spans (64 KiB..4 MiB)
        _ => 16 * 1024 * 1024 + rng.below(8 * 1024 * 1024), // huge (16..24 MiB), 0.2%
    }
}

/// Generate a synthetic single-threaded trace: `n` churn ops with a bounded
/// live set, LIFO+random-free mix, ~10% zalloc.
pub fn generate(path: &str, n: u64, seed: u64) -> Result<(), String> {
    let f = std::fs::File::create(path).map_err(|e| format!("{path}: {e}"))?;
    let mut w = Writer::new(std::io::BufWriter::new(f)).map_err(|e| e.to_string())?;
    let mut rng = Rng(seed | 1);
    let mut next_id = 1u64;
    let mut live: Vec<(u64, u8)> = Vec::new(); // (id, align_log2)
    w.write(&Record {
        op: Op::ThreadStart,
        align_log2: 0,
        thread: 0,
        size: 0,
        ptr: 0,
        old_ptr: 0,
    })
    .map_err(|e| e.to_string())?;
    for _ in 0..n {
        let do_free = !live.is_empty() && (live.len() > 4000 || rng.below(100) < 45);
        if !live.is_empty() && rng.below(10) == 0 {
            // ~10% realloc of a random live NATURAL-alignment block (realloc
            // does not preserve stronger alignment — matching the C contract).
            let idx = rng.below(live.len());
            let (id, alog) = live[idx];
            if alog == 0 {
                let newsize = synth_size(&mut rng) as u64;
                let new_id = next_id;
                next_id += 1;
                live[idx] = (new_id, 0);
                w.write(&Record {
                    op: Op::Realloc,
                    align_log2: 0,
                    thread: 0,
                    size: newsize,
                    ptr: new_id,
                    old_ptr: id,
                })
                .map_err(|e| e.to_string())?;
            } else {
                // Aligned block drawn: free it instead.
                live.swap_remove(idx);
                w.write(&Record {
                    op: Op::Free,
                    align_log2: 0,
                    thread: 0,
                    size: 0,
                    ptr: 0,
                    old_ptr: id,
                })
                .map_err(|e| e.to_string())?;
            }
        } else if do_free {
            // Mostly LIFO (cache-friendly, mimalloc's sweet spot), sometimes random.
            let idx = if rng.below(4) == 0 {
                rng.below(live.len())
            } else {
                live.len() - 1
            };
            let (id, _) = live.swap_remove(idx);
            w.write(&Record {
                op: Op::Free,
                align_log2: 0,
                thread: 0,
                size: 0,
                ptr: 0,
                old_ptr: id,
            })
            .map_err(|e| e.to_string())?;
        } else {
            let op = if rng.below(10) == 0 {
                Op::Zalloc
            } else {
                Op::Malloc
            };
            // ~15% aligned (16 B .. 64 KiB) — exercises natural-fit AND the
            // oversize-adjust interior-pointer path.
            let alog: u8 = if rng.below(100) < 15 {
                [4u8, 5, 6, 7, 8, 9, 12, 16][rng.below(8)]
            } else {
                0
            };
            let size = synth_size(&mut rng) as u64;
            w.write(&Record {
                op,
                align_log2: alog,
                thread: 0,
                size,
                ptr: next_id,
                old_ptr: 0,
            })
            .map_err(|e| e.to_string())?;
            live.push((next_id, alog));
            next_id += 1;
        }
    }
    for (id, _) in live.drain(..) {
        w.write(&Record {
            op: Op::Free,
            align_log2: 0,
            thread: 0,
            size: 0,
            ptr: 0,
            old_ptr: id,
        })
        .map_err(|e| e.to_string())?;
    }
    w.write(&Record {
        op: Op::ThreadEnd,
        align_log2: 0,
        thread: 0,
        size: 0,
        ptr: 0,
        old_ptr: 0,
    })
    .map_err(|e| e.to_string())?;
    let mut inner = w.finish().map_err(|e| e.to_string())?;
    inner.flush().map_err(|e| e.to_string())?;
    Ok(())
}
