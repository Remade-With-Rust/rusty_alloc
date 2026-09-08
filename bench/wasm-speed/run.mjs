// Best-of-N, net of a measured floor, with checksums proving work parity.
import { readFileSync } from 'node:fs';

const WORK = [
  { kind: 1, name: '32 B alloc/free', iters: 200_000, ops: 200_000 },
  { kind: 2, name: '64 mixed, batched', iters: 3_000, ops: 3_000 * 64 },
  { kind: 3, name: 'churn 64 live, 8-512 B', iters: 200_000, ops: 200_000 },
  { kind: 4, name: '2048 B alloc/free', iters: 200_000, ops: 200_000 },
];
const RUNS = 11;

async function load(path) {
  const bytes = readFileSync(path);
  const { instance } = await WebAssembly.instantiate(bytes, {});
  return instance.exports.bench;
}

function best(fn, kind, iters) {
  let ns = Infinity, sum = 0;
  for (let r = 0; r < RUNS; r++) {
    const t0 = process.hrtime.bigint();
    sum = fn(kind, iters) >>> 0;
    const t1 = process.hrtime.bigint();
    const d = Number(t1 - t0);
    if (d < ns) ns = d;
  }
  return { ns, sum };
}

// INTERLEAVED, not arm-A-then-arm-B. Measuring one module to completion and
// then the other lets slow drift -- another process waking up, a thermal step,
// the OS scheduler -- land entirely on one arm. That produced two orderings
// that disagreed in SIGN on the same change. Alternating the arms within each
// repeat and taking the per-arm minimum cancels any drift slower than one
// repeat, which is what makes a 10% effect resolvable at all.
const names = ['dlmalloc', 'rusty_alloc'];
const fns = [await load(process.argv[2]), await load(process.argv[3])];
// Warm EVERY branch of BOTH modules first. V8 tiers wasm up per code path, so
// timing a cold branch against a hot one produced a "floor" that differed 8x
// between two modules running IDENTICAL floor code -- the tell that the
// harness, not the allocator, was being measured.
for (const fn of fns) for (const k of [0, 1, 2, 3, 4]) fn(k, k === 2 ? 300 : 20_000);

const arms = { dlmalloc: { floor: null, rows: {} }, rusty_alloc: { floor: null, rows: {} } };
const acc = new Map(); // "arm|kind" -> {ns, sum}
for (let r = 0; r < RUNS; r++) {
  for (const w of [{ kind: 0, name: '__floor', iters: 200_000 }, ...WORK]) {
    for (let a = 0; a < 2; a++) {
      const t0 = process.hrtime.bigint();
      const sum = fns[a](w.kind, w.iters) >>> 0;
      const d = Number(process.hrtime.bigint() - t0);
      const key = `${a}|${w.name}`;
      const cur = acc.get(key);
      if (!cur || d < cur.ns) acc.set(key, { ns: d, sum });
    }
  }
}
for (let a = 0; a < 2; a++) {
  arms[names[a]].floor = acc.get(`${a}|__floor`);
  for (const w of WORK) arms[names[a]].rows[w.name] = acc.get(`${a}|${w.name}`);
}

const f0 = arms.dlmalloc.floor.ns / 200_000;
const f1 = arms.rusty_alloc.floor.ns / 200_000;
console.log(`\nharness floor (no allocator call): dlmalloc ${f0.toFixed(1)} ns/op, rusty_alloc ${f1.toFixed(1)} ns/op`);
console.log(`${'workload'.padEnd(24)}${'dlmalloc'.padStart(12)}${'rusty_alloc'.padStart(13)}${'speedup'.padStart(10)}   checksums`);
for (const w of WORK) {
  const a = arms.dlmalloc.rows[w.name], b = arms.rusty_alloc.rows[w.name];
  const an = a.ns / w.ops - f0, bn = b.ns / w.ops - f1;
  const ok = a.sum === b.sum ? `match (${a.sum})` : `MISMATCH ${a.sum} vs ${b.sum}`;
  const ar = a.ns / w.ops, br = b.ns / w.ops;
  console.log(`${w.name.padEnd(24)}${an.toFixed(1).padStart(10)} ns${bn.toFixed(1).padStart(11)} ns${(an / bn).toFixed(2).padStart(9)}x   raw ${ar.toFixed(1)}/${br.toFixed(1)}   ${ok}`);
}
