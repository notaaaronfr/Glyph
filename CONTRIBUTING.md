# Contributing to Glyph

Thank you for helping build Glyph! All kinds of contributions are welcome: bug reports, example programs, documentation, error-message improvements, and language features.

New here? Read [VISION.md](VISION.md) first. It explains what Glyph is for, and it's the yardstick for every design decision.

## Getting started

You need [Rust](https://rustup.rs) (stable) and a C compiler (`cc`, `clang` or `gcc`) for the native backend.

```sh
git clone https://github.com/notaaaronfr/Glyph.git
cd Glyph
cargo build --release
./target/release/glyph run examples/bank.gly
cargo test --release
```

## How the code is organised

The workspace is split along the trust boundary described in [VISION.md](VISION.md#trust-model). Each crate declares its trust level in `[package.metadata.glyph]`, and `scripts/tcb.sh` counts the trusted lines.

```
crates/
  glyph-front/     trusted: what a program means
    lexer.rs       source text → tokens
    parser.rs      tokens → syntax tree (ast.rs)
    loader.rs      finds and loads imported modules, gives their items qualified names
    check.rs       static checks: names, arity, types, effects
    error.rs       diagnostics (human-readable and JSON)
  glyph-run/       trusted: the reference semantics
    interp.rs      tree-walking interpreter (glyph run, glyph test)
  glyph-kernel/    trusted for claims about Glyph OS
    kernel.c       the kernel for `--target kernel`: boot, MMU, interrupts, scheduler,
                   per-app heaps, serial port, timer, effect permissions
    kernel_api.h   what the kernel provides to apps (each app is compiled against it)
    lib.rs         each app's grant table, the boot image build, QEMU
  glyph-native/    trusted only for claims about native binaries
    codegen.rs     native compiler: syntax tree → C (hosted, or an app for the kernel)
    runtime.c      small C runtime embedded in every native executable and app
src/               the `glyph` command, not trusted
  fix.rs           `glyph fix`: AI repair loop, its guard rails, and the Claude API client
  main.rs          command-line interface
tests/
  cases/           snapshot tests: *.gly programs and their *.expected output
                   (cases/lib/ holds modules they import)
  cases.rs         the test harness
  redteam/         candidates `glyph fix` must reject without side effects, and their recorded state
  redteam.rs       fails when a case's behaviour changes, so a fix flips its case in the same change
  fix/             a broken program, an honest fix and "cheating" fixes for the guard tests
  memory/          programs checked for leaks, use-after-free and in-place reuse
  kernel/          kernel-target tests (the boot tests also use examples/kernel/)
scripts/
  tcb.sh           trusted-base size and `forbid(unsafe_code)` check (needs jq)
  exit/            one script per roadmap phase; a phase is done when its script passes
docs/decisions/    why the project is shaped the way it is
examples/          example programs
```

A typical language feature touches, in order: `lexer.rs` → `parser.rs`/`ast.rs` → `check.rs` → `interp.rs` → `codegen.rs` (and maybe `runtime.c`) → a test case. A change to a trusted crate is a change to what Glyph promises, so it needs a test that would have caught the old behaviour.

## The golden rule: interpreter and native must agree

Glyph has two backends. **For every program, `glyph run` and `glyph run --native` must produce byte-for-byte identical output, including error messages.** The test suite enforces this. If you add or change behaviour, change both backends together.

## Tests

Every `tests/cases/*.gly` file is run through both backends, and the output is compared with `tests/cases/<name>.expected`.

To add a test, create `tests/cases/my_feature.gly` with a comment on its first line saying what it tests. Then generate the snapshot:

```sh
GLYPH_BLESS=1 cargo test --release
```

Read the generated `.expected` file carefully: it becomes the definition of correct. Commit both files.

Before opening a pull request, run:

```sh
cargo fmt --all
cargo clippy --release --workspace --all-targets -- -D warnings
cargo test --release --workspace
scripts/tcb.sh
```

CI runs the same checks on Linux and macOS. The kernel tests boot QEMU; locally they are skipped unless `qemu-system-aarch64`, clang and lld are installed (`brew install qemu llvm lld`).

## Memory in the native backend

Native code uses reference counting. If you touch `codegen.rs` or `runtime.c`, follow the ownership rules (they're also described at the top of `runtime.c`):

- Temporaries own their value; variables own theirs until their scope ends. Reading a variable *borrows* it.
- Functions take ownership of their arguments and return an owned result.
- Runtime functions borrow their arguments and return owned results, except where a comment says they take ownership (`tn_list_push`, `tn_rec_set`, constructors).
- In codegen, use `own()` when a value is stored or passed on, and let statement and scope cleanup drop the rest.

`cargo test` checks this: every program in `tests/memory/` and `tests/cases/` is built with AddressSanitizer and checked for use-after-free, double free and leaks. Add a case to `tests/memory/` when you add a feature that creates heap values.

## Error messages

Error messages are part of Glyph's interface, for humans and for AI agents. A good error:

- says **what** went wrong in plain words, with the exact values involved
- points at the **right place** (for a precondition, that's the caller; for a postcondition, the function)
- gives a **hint** with a concrete fix when one exists

Improving an unclear error is a great first contribution.

## Proposing language changes

Changes to syntax or semantics deserve discussion before code. Open an issue using the feature request template and explain:

- the problem, with a realistic example
- the proposed syntax and behaviour
- how it fits the principles in [VISION.md](VISION.md), in particular whether it keeps code explicit and checkable

## Good first contributions

- New example programs in `examples/`
- More test cases for existing features and edge cases
- Clearer error messages and hints
- New pure built-in functions (implemented in both `interp.rs` and `runtime.c`)
- Documentation and README improvements

## Pull requests

- Keep each PR focused on one change.
- Describe what changed and why, and link the related issue.
- Add or update tests.
- Be patient and kind in review; everyone here is volunteering their time.

By contributing, you agree that your contributions are licensed under the project's dual MIT / Apache-2.0 license, and you agree to follow the [Code of Conduct](CODE_OF_CONDUCT.md).

## PR Template

### Summary

<!-- Briefly explain what this PR does and why. -->

### Changes

- 
- 

### Testing

<!-- Explain how you verified the changes. -->

- [ ] Tests pass
- [ ] Manually tested where applicable

### Related Issue

<!-- Example: Closes #123 -->
Closes #

### Screenshots / Notes

<!-- Add screenshots for UI changes or any additional context. Remove if not applicable. -->