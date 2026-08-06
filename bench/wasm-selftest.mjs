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
