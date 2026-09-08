#!/usr/bin/env bash
# Build the crates that ship this allocator, BEFORE and AFTER the upgrade.
#
# A major version is a promise that downstreams can keep compiling, and nothing
# inside this repo can check it. 2.0.0 redefines `default-features = false` to
# mean `no_std` (which additionally refuses to build without
# `--cfg ra_single_threaded`) and adds a field to `heap::Heap`. Both are
# invisible here and land on consumers.
#
#   BASELINE   the consumer exactly as it is on disk -- its pinned rusty_alloc
#              from crates.io. Establishes it was green to begin with, so a red
#              CANDIDATE can be blamed on us rather than on it.
#   CANDIDATE  the same consumer with every rusty_alloc* requirement rewritten
#              to this working tree.
#
# `[patch.crates-io]` CANNOT do this: a patch has to satisfy the original
# requirement, and `=1.1.6` is not satisfied by 2.0.0. Simulating the upgrade
# therefore means rewriting the requirement, which is done in a COPY -- this
# script never writes inside a consumer's checkout.
#
#   bash tools/corpus/run.sh            # compile gate (fast)
#   bash tools/corpus/run.sh --test     # compile + that consumer's test suite
set -uo pipefail
root="$(cd "$(dirname "$0")/../.." && pwd)"
work="${TMPDIR:-/tmp}/ra-corpus"
mode="check"
[ "${1:-}" = "--test" ] && mode="test"

ALLOC="$root/crates/rusty_alloc"
API="$root/crates/rusty_alloc_api"

pass=0; fail=0; skip=0
declare -a ROWS

# Rewrite every rusty_alloc* requirement in a tree to point at this checkout.
# Handles the three shapes seen in the corpus: a bare version, a version with
# `default-features`, and a workspace-level entry.
repoint() {
  local tree="$1"
  find "$tree" -name Cargo.toml -not -path "*/target/*" -print0 |
    while IFS= read -r -d '' f; do
      python3 - "$f" "$ALLOC" "$API" <<'PY'
import re, sys
path, alloc, api = sys.argv[1], sys.argv[2], sys.argv[3]
s = open(path, encoding='utf-8').read()
orig = s
# `name = { version = "...", ... }` -> keep the rest, swap in a path
def patch(m):
    name, body = m.group(1), m.group(2)
    tgt = api if 'api' in name else alloc
    body = re.sub(r'version\s*=\s*"[^"]*"\s*,?\s*', '', body)
    body = body.strip().strip(',').strip()
    inner = f'path = "{tgt}"' + (f', {body}' if body else '')
    return f'{name} = {{ {inner} }}'
s = re.sub(r'^(rusty[_-]alloc(?:[_-]api)?)\s*=\s*\{([^}]*)\}', patch, s, flags=re.M)
# `name = "1.1.6"` -> path form
s = re.sub(r'^(rusty[_-]alloc(?:[_-]api)?)\s*=\s*"[^"]*"',
           lambda m: f'{m.group(1)} = {{ path = "{api if "api" in m.group(1) else alloc}" }}', s, flags=re.M)
if s != orig:
    open(path, 'w', encoding='utf-8').write(s)
PY
    done
}

# Some consumers live in a workspace whose ROOT is not in this checkout --
# `packages/spacedb/*` inherit their sibling dependencies with
# `workspace = true`, and mata-master carries no manifest above them. Without a
# root, cargo cannot parse the manifest at all, so the corpus could not answer
# for SpaceDB either way.
#
# They inherit only DEPENDENCIES (every package field is literal), so a root is
# reconstructable: list the sibling crate directories as members and declare
# each as a path dependency. Written into the COPY, never the checkout.
synth_workspace() {
  local tree="$1"
  python3 - "$tree" <<'PY'
import pathlib, sys, re
root = pathlib.Path(sys.argv[1])
if (root / 'Cargo.toml').exists():
    sys.exit(0)
members = sorted(d.name for d in root.iterdir() if (d / 'Cargo.toml').exists())
if not members:
    sys.exit(0)
names = {}
for m in members:
    t = (root / m / 'Cargo.toml').read_text(encoding='utf-8')
    n = re.search(r'^name\s*=\s*"([^"]+)"', t, re.M)
    if n:
        names[n.group(1)] = m
lines = ['# SYNTHESISED by tools/corpus/run.sh -- this checkout has no workspace',
         '# root above these crates, and they inherit their sibling deps from one.',
         '[workspace]', 'resolver = "2"',
         'members = [' + ', '.join(f'"{m}"' for m in members) + ']',
         '', '[workspace.dependencies]']
for n, d in sorted(names.items()):
    lines.append(f'{n} = {{ path = "{d}" }}')
(root / 'Cargo.toml').write_text('\n'.join(lines) + '\n', encoding='utf-8')
print(f'  synthesised a workspace root over {len(members)} crates', file=sys.stderr)
PY
}

run_one() {
  local name="$1" path="$2" feats="$3" pkg="$4" synth="$5" note="$6"
  if [ ! -d "$path" ]; then
    echo "  SKIP  $name -- $path not on this machine"
    ROWS+=("SKIP|$name|not on this machine")
    skip=$((skip + 1))
    return
  fi
  local fargs=()
  [ -n "$feats" ] && fargs=(--features "$feats")
  # `-p` where the consumer names one: applying a `rusty-alloc` feature across a
  # whole workspace can install the global allocator twice (rusty_zstd's CLI
  # bins and `rusty_alloc_default` both claim it), which is this harness picking
  # the wrong target rather than anything being wrong downstream.
  [ -n "${pkg:-}" ] && fargs+=(-p "$pkg")

  echo "== $name"
  echo "   $note"

  local tag; tag="$(echo "$name" | tr -c 'A-Za-z0-9_.-' '_' | sed 's/_*$//')"
  local dst="$work/$tag" base_dir="$path"
  copy_tree() {
    rm -rf "$1"; mkdir -p "$1"
    # `target/` and `.git/` are the whole cost of the copy; exclude both.
    (cd "$path" && tar -cf - --exclude=./target --exclude=./.git .) | (cd "$1" && tar -xf -) 2>/dev/null
  }

  # BASELINE. Normally the checkout itself; when a root has to be synthesised
  # the baseline needs a copy too, or there is nothing to compare against.
  if [ "${synth:-}" = "yes" ]; then
    base_dir="$work/${tag}__base"
    copy_tree "$base_dir"
    synth_workspace "$base_dir"
  fi
  local base_out base_rc
  base_out="$(cd "$base_dir" && cargo "$mode" --quiet "${fargs[@]}" 2>&1)"
  base_rc=$?

  # CANDIDATE: a copy, repointed at this tree.
  copy_tree "$dst"
  [ "${synth:-}" = "yes" ] && synth_workspace "$dst"
  repoint "$dst"
  local cand_out cand_rc
  cand_out="$(cd "$dst" && cargo "$mode" --quiet "${fargs[@]}" 2>&1)"
  cand_rc=$?

  if grep -q "failed to find a workspace root" <<<"$base_out"; then
    echo "  SKIP  workspace root absent from this checkout -- cannot answer here"
    ROWS+=("SKIP|$name|workspace root not in this checkout")
    skip=$((skip + 1))
    return
  fi
  if [ $base_rc -ne 0 ] && [ $cand_rc -ne 0 ]; then
    echo "  BASELINE ALREADY RED -- not ours"
    ROWS+=("BASELINE-RED|$name|red before the upgrade too")
    skip=$((skip + 1))
  elif [ $base_rc -ne 0 ]; then
    echo "  baseline red, candidate GREEN (consumer is behind, not broken by us)"
    ROWS+=("OK|$name|baseline red, candidate green")
    pass=$((pass + 1))
  elif [ $cand_rc -eq 0 ]; then
    echo "  PASS  both arms green"
    ROWS+=("PASS|$name|both arms green")
    pass=$((pass + 1))
  elif ! grep -qiE "rusty[_-]alloc" <<<"$cand_out"; then
    # The candidate is red but nothing in the error mentions us. A consumer can
    # be broken for its own reasons -- a missing dependency, a resolver shift
    # from the rewritten manifest -- and blaming the upgrade for it would make
    # this harness worse than useless: it would cry wolf until someone muted it.
    echo "  UNRELATED  candidate red, but no rusty_alloc symbol in the error"
    echo "$cand_out" | grep -E "^error" | head -2 | sed 's/^/        /'
    ROWS+=("UNRELATED|$name|$(echo "$cand_out" | grep -E '^error' | head -1 | cut -c1-70)")
    skip=$((skip + 1))
  else
    echo "  FAIL  2.0.0 BREAKS THIS CONSUMER"
    echo "$cand_out" | grep -E "^error" | head -3 | sed 's/^/        /'
    local first
    first="$(echo "$cand_out" | grep -E "^error" | head -1 | cut -c1-110)"
    ROWS+=("FAIL|$name|$first")
    fail=$((fail + 1))
  fi
}

echo "downstream corpus ($mode) against $root"
echo
python3 - "$root/tools/corpus/corpus.toml" <<'PY' > "$work.list" 2>/dev/null || mkdir -p "$(dirname "$work.list")"
import re, sys
s = open(sys.argv[1], encoding='utf-8').read()
for blk in s.split('[[consumer]]')[1:]:
    g = lambda k: (re.search(rf'^{k}\s*=\s*"(.*)"', blk, re.M) or [None, ''])[1]
    print('|'.join([g('name'), g('path'), g('features'), g('pkg'), g('synth'), g('note')]))
PY
mkdir -p "$work"
# `|`, not tab: tab is IFS WHITESPACE, so bash collapses runs of it and an empty
# `features` field silently shifts every column after it. That fed each
# consumer's NOTE to `--features` and turned all five baselines red -- the
# harness reporting on itself, again.
while IFS='|' read -r n p f pk sy note; do
  [ -n "$n" ] && run_one "$n" "$p" "$f" "$pk" "$sy" "$note"
done < "$work.list"

echo
printf '%-14s %-26s %s\n' RESULT CONSUMER DETAIL
for r in "${ROWS[@]}"; do
  IFS='|' read -r a b c <<<"$r"
  printf '%-14s %-26s %s\n' "$a" "$b" "$c"
done
echo
echo "$pass passed, $fail broken by the upgrade, $skip skipped"
[ $fail -gt 0 ] && exit 1
exit 0
