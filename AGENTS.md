# AGENTS.md

Glyph exists so an agent's change can be checked against promises a human owns ([VISION.md](VISION.md)).
Hold changes to this repo to the same standard: never make a check pass by weakening it. The crate map,
the order a feature touches files, and the native memory rules are in [CONTRIBUTING.md](CONTRIBUTING.md).

## Checks you must not weaken

- **Interpreter and native agree byte for byte, error messages included.** A behaviour change lands in
  `interp.rs` and in `codegen.rs`/`runtime.c` in the same change.
- **`GLYPH_BLESS=1` rewrites every `.expected` file.** Use it only for a new case or an intended output
  change, then read `git diff tests/cases/` and account for every changed line. Never bless to make a failing test pass.
- **Red-team cases are permanent.** Change a case in `tests/redteam.rs` only to flip `KnownBypass` to
  `Holds`, in the change that fixes it. Never delete one.
- **Kernel tests skip silently** if `qemu-system-aarch64` or clang is missing, and `cargo test` still passes.
  After touching `glyph-kernel` or `--target kernel`, run with `GLYPH_REQUIRE_KERNEL=1`, or say they didn't run.
- Fix clippy warnings rather than silencing them. If you must, use `#[expect(lint, reason = "...")]`, never `#[allow]`.

## Trusted crates

- `glyph-front`, `glyph-run` and `glyph-kernel` are trusted (`trust` in `[package.metadata.glyph]`), and
  `glyph-native` is trusted for claims about native binaries. Each keeps `#![forbid(unsafe_code)]`.
- Don't add crates.io dependencies to them. `scripts/tcb.sh` counts only each crate's own lines, so a
  dependency grows the trusted base without tripping the budget.
- New syntax or semantics needs an accepted issue first. New Glyph OS surface waits for P2. Every capability
  claim in docs carries a `[vX.Y]` or `[Pn]` label, and nothing planned is described as shipped.

## Style

- Private modules with an explicitly exported public API; internals are `pub(crate)`. Name modules for what
  they own, not `utils` or `common`.
- On enums defined in this workspace, list every variant instead of `_ =>`, so a new variant flags every site
  that must handle it. `_` is fine for strings, chars and numbers. Treat existing enum wildcards as debt:
  replace one when you edit its function.
- No opaque positional `bool`/`Option` parameters (`foo(false)` is unreadable). Prefer enums or named methods.
- Don't add trivial wrappers that only rename a call. Extracting a named step from a long function is fine.
- New traits need concise doc comments explaining their role and what implementations must guarantee.

## Tests

- Bug fixes need a regression test that fails without the fix, usually a `tests/cases/*.gly` case (it runs
  both backends) or a red-team case.
- Prefer whole-object equality over field-by-field checks. Don't add tests for statically defined values.
- Don't add negative tests for removed logic, unless the removal is a soundness or authority property (an
  escape hatch, `Any`, a guard rule). Those get a red-team case.
- Tests are offline. `glyph fix` tests use `--candidate` and never call the model API. Use temporary paths
  (`CARGO_TARGET_TMPDIR`), restore shared state, and wait on observable events, not fixed sleeps.

## Repository

- **Never set git identity** (`user.name`/`user.email`, `-c`, `GIT_AUTHOR_*`/`GIT_COMMITTER_*`). Commits
  carry the ambient git config's identity. Push and open PRs as the active `gh` account.
- **Never add `Co-authored-by` trailers**, in commits, squash messages or credit for merged work, even
  when a tool adds one by default. They misattribute commits to unrelated GitHub accounts.
- Open PRs with [`.github/pull_request_template.md`](.github/pull_request_template.md).
