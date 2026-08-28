// Run the allocator's self-test inside a real WebAssembly VM (Node).
//
// `cargo test` cannot execute wasm32-unknown-unknown, so this instantiates the
// cdylib and calls its `ra_selftest` export. Exit code 0 = pass.
//
// Usage: node bench/wasm-selftest.mjs <path-to.wasm>
import { readFile } from "node:fs/promises";
import process from "node:process";

const ERRORS = {
  1: "malloc returned null",
  2: "small-block pattern mismatch (blocks overlap or were clobbered)",
  3: "usable_size smaller than the requested size",
  4: "zalloc returned non-zero bytes",
  5: "realloc returned null",
  6: "realloc lost the preserved prefix",
  7: "large allocation returned null",
  8: "large-block pattern mismatch",
  9: "churn allocation returned null (page recycling failed)",
  10: "block was not word-aligned",
  11: "steady-state allocation returned null",
  12: "steady-state cycles GREW linear memory - segment/huge recycling failed (the v1.1.4 wasm leak)",
};

const path = process.argv[2];
if (!path) {
  console.error("usage: node wasm-selftest.mjs <path-to.wasm>");
  process.exit(2);
}

const bytes = await readFile(path);
const module = await WebAssembly.compile(bytes);

const needed = WebAssembly.Module.imports(module);
if (needed.length > 0) {
  console.log(`note: module declares ${needed.length} import(s):`);
  for (const i of needed) console.log(`  ${i.module}.${i.name} (${i.kind})`);
}

// Supply stubs for whatever the module asks for, so a stray std import cannot
// stop us from exercising the allocator itself.
const imports = {};
for (const i of needed) {
  imports[i.module] ??= {};
  imports[i.module][i.name] =
    i.kind === "function"
      ? () => {
          throw new Error(`unexpected host call: ${i.module}.${i.name}`);
        }
      : i.kind === "memory"
        ? new WebAssembly.Memory({ initial: 1 })
        : 0;
}

const instance = await WebAssembly.instantiate(module, imports);
const { ra_selftest, ra_memory_bytes } = instance.exports;

if (typeof ra_selftest !== "function") {
  console.error("FAIL: module does not export ra_selftest");
  process.exit(2);
}

const before = ra_memory_bytes ? ra_memory_bytes() >>> 0 : 0;
const t0 = process.hrtime.bigint();
const rc = ra_selftest();
const t1 = process.hrtime.bigint();
const after = ra_memory_bytes ? ra_memory_bytes() >>> 0 : 0;

const mib = (n) => (n / (1024 * 1024)).toFixed(2);
console.log(`linear memory: ${mib(before)} MiB -> ${mib(after)} MiB`);
console.log(`selftest ran in ${Number(t1 - t0) / 1e6} ms`);

// Set exitCode rather than calling process.exit(): forcing exit while stdout
// still has buffered writes trips a libuv assertion on Windows, which looked
// like a crash AFTER the test had already passed.
if (rc === 0) {
  console.log("WASM SELFTEST PASSED");
  process.exitCode = 0;
} else {
  console.error(`WASM SELFTEST FAILED: code ${rc} - ${ERRORS[rc] ?? "unknown"}`);
  process.exitCode = 1;
}

// ---------------------------------------------------------------------------
// Waste gate (F5, docs/plans/segment-tax.md): marginal linear memory per
// live block, measured with a FRESH INSTANCE per data point — with a shared
// instance every row after the first is served from recycled memory and
// reads zero, which is how the segment tax stayed invisible. Methodology and
// probe are the segment-tax report's own (hold hi, hold lo, difference).
//
// Bounds are the segment geometry, not aspirations: floor(511/slices) spans
// pack per 32 MiB segment; a span above 255 slices cannot pair with ITSELF,
// so single-size probes above ~16 MiB legitimately read a whole segment and
// their rows are regression pins. The discriminating row is the MIX: a
// 20 MiB span (320 slices) and an 11.875 MiB span (190 slices) share one
// segment (510 <= 511) — before the span-routing fix the 20 MiB block took a
// dedicated 32 MiB huge reservation and the pair cost ~48 MiB marginal.
if (rc === 0) {
  const MIB = 1024 * 1024;
  const inst = async () => (await WebAssembly.instantiate(module, imports)).exports;
  const single = async (bytes, lo, hi) => {
    const a = (await inst()).ra_hold(bytes, lo) >>> 0;
    const b = (await inst()).ra_hold(bytes, hi) >>> 0;
    return (b - a) / (hi - lo);
  };
  const mix = async (x, y, lo, hi) => {
    const a = (await inst()).ra_hold_mix(x, y, lo) >>> 0;
    const b = (await inst()).ra_hold_mix(x, y, hi) >>> 0;
    return (b - a) / (hi - lo);
  };

  const rows = [
    ["8 MiB single (packs 3/segment)",        () => single(8 * MIB, 1, 9),                     12 * MIB],
    ["16 MiB - 128 KiB single (packs 2)",     () => single(16 * MIB - 128 * 1024, 1, 9),       20 * MIB],
    ["20 MiB single (regression pin)",        () => single(20 * MIB, 1, 9),                    34 * MIB],
    ["20 MiB + 11.875 MiB pair (F1 gate)",    () => mix(20 * MIB, 190 * 65536, 1, 5),          34 * MIB],
    ["25.1 MiB + 6 MiB pair",                 () => mix(402 * 65536, 6 * MIB, 1, 5),           34 * MIB],
    ["33 MiB single (huge; F2 will lower)",   () => single(33 * MIB, 1, 3),                    66 * MIB],
  ];

  let wasteFailed = false;
  console.log("--- waste gate (marginal linear memory per block/pair) ---");
  for (const [label, run, bound] of rows) {
    const m = await run();
    const ok = m <= bound;
    if (!ok) wasteFailed = true;
    console.log(
      `${ok ? "PASS" : "FAIL"}  ${label}: ${(m / MIB).toFixed(2)} MiB (bound ${(bound / MIB).toFixed(0)} MiB)`,
    );
  }
  if (wasteFailed) {
    console.error("WASTE GATE FAILED: a marginal cost exceeded its geometric bound (the segment tax)");
    process.exitCode = 1;
  } else {
    console.log("WASTE GATE PASSED");
  }
}
