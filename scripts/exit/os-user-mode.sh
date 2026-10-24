#!/bin/bash
# Exit test for the Glyph OS user-mode milestone (VISION.md, v0.14): apps run at EL0,
# each in its own address space, and every attack from user mode stops only the
# attacker. Needs qemu-system-aarch64, clang and lld; fails instead of skipping.
set -euo pipefail
cd "$(dirname "$0")/../.."

GLYPH_REQUIRE_KERNEL=1 env -u GLYPH_BLESS cargo test --release --locked --test cases -- \
  kernel_user_mode_stops_attacks_on_the_kernel_and_other_apps \
  --exact
GLYPH_REQUIRE_KERNEL=1 env -u GLYPH_BLESS cargo test --release --locked --test cases -- kernel_

echo "OS user-mode exit test passed"
