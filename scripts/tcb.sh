#!/bin/bash
# Reports the size of Glyph's trusted base and fails if it breaks the vision's M10 limits:
# at most 20,000 lines across crates whose Cargo metadata says `trust = "acceptance"` or
# `trust = "kernel"` (their Rust, plus the kernel's C), each with `#![forbid(unsafe_code)]`
# in its crate root.
set -euo pipefail
cd "$(dirname "$0")/.."

budget=20000
total=0
status=0

while IFS=$'\t' read -r name trust root; do
  dir=$(dirname "$(dirname "$root")")
  lines=$(find "$dir/src" \( -name '*.rs' -o -name '*.c' -o -name '*.h' \) -print0 | xargs -0 cat | wc -l | tr -d ' ')
  printf '%-14s %-11s %6s lines\n' "$name" "$trust" "$lines"
  case "$trust" in
    acceptance | kernel)
      total=$((total + lines))
      if ! grep -q '^#!\[forbid(unsafe_code)\]' "$root"; then
        echo "error: $name is trusted but $root lacks #![forbid(unsafe_code)]" >&2
        status=1
      fi
      ;;
  esac
done < <(cargo metadata --format-version 1 --no-deps --offline |
  jq -r '.packages[] | [.name, (.metadata.glyph.trust // "missing"), ([.targets[] | select(.kind[0] == "lib" or .kind[0] == "bin")][0].src_path)] | @tsv' |
  sort)

echo "trusted total: $total of $budget lines"
if [ "$total" -gt "$budget" ]; then
  echo "error: the trusted base exceeds $budget lines" >&2
  status=1
fi
if cargo metadata --format-version 1 --no-deps --offline | jq -e '.packages[] | select(.metadata.glyph.trust == null)' >/dev/null; then
  echo "error: every crate must declare [package.metadata.glyph] trust" >&2
  status=1
fi
exit $status
