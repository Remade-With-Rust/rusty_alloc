#!/usr/bin/env bash
# Prove the H-22 rules can still FAIL.
#
# "A gate you have never seen fail is not a gate" — this repo learned that
# the expensive way when corpus/linux-gates.sh reported success on a build
# that did not compile (docs/LEDGER.md, AUDIT 2026-08-06). A ruleset that
# silently stops matching is the same defect wearing a different hat: the CI
# job goes green forever and nobody notices.
#
# So: synthesise a file containing exactly the shapes the rules exist to
# catch, and require every rule to fire on it.
set -uo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

mkdir -p "$tmp/crates/rusty_alloc/src"
cat > "$tmp/crates/rusty_alloc/src/alloc.rs" <<'EOF'
fn discarded_results() {
    let _ = self.adopt_segment(aseg);
    let _ = self.retire_span(seg, p);
}
fn vanishing_error_path() {
    debug_assert!(false, "not in list");
}
pub struct S {
    pub base: usize,
}
fn ungated_counter() {
    (*h).stats.allocs += 1;
}
fn c_build() {
    cc::Build::new().compile("x");
}
EOF

out="$(semgrep --config "$root/tools/semgrep-rules.yml" --quiet --json "$tmp" 2>/dev/null)"
fired="$(printf '%s' "$out" | python3 -c '
import json,sys
d = json.load(sys.stdin)
ids = {r["check_id"].split(".")[-1] for r in d["results"]}
print(" ".join(sorted(ids)))
')"

expected="debug-assert-false-as-error-path discarded-lifecycle-result no-c-build-dependency pointer-stored-as-integer unconditional-hot-path-counter"
missing=""
for rule in $expected; do
    case " $fired " in *" $rule "*) ;; *) missing="$missing $rule";; esac
done

if [ -n "$missing" ]; then
    echo "SELFTEST FAILED: these rules did not fire on code that violates them:$missing"
    echo "fired: $fired"
    exit 1
fi
echo "SELFTEST OK: all 5 rules fire on the shapes they exist to catch"
