#!/bin/bash
# Exit test for roadmap phase P0a (VISION.md). Exits 0 only when every condition holds.
set -euo pipefail
cd "$(dirname "$0")/../.."

cargo fmt --all --check
cargo clippy --release --workspace --all-targets --locked -- -D warnings
# The QEMU kernel tests skip where qemu, clang and lld are missing. CI sets
# GLYPH_REQUIRE_KERNEL=1 on Linux, so there they fail instead of skipping.
env -u GLYPH_BLESS cargo test --release --workspace --locked
scripts/tcb.sh

for case in tests/redteam/g{1,2,3,4,5,6}_*/; do
  test -f "$case/base.gly"
  test -f "$case/candidate.gly"
done
test -f tests/cases/int_whitespace.expected
test -f tests/cases/string_quoting.expected
# `set -e` ignores a failing `! cmd`, so the overclaim check exits explicitly.
if grep -q "which the compiler guarantees are true" VISION.md README.md; then
  echo "error: the docs still claim the compiler guarantees contracts" >&2
  exit 1
fi

echo "P0a exit test passed"
