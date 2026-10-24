# Glyph

**The language AI writes and humans trust.**

[![CI](https://github.com/notaaaronfr/Glyph/actions/workflows/ci.yml/badge.svg)](https://github.com/notaaaronfr/Glyph/actions/workflows/ci.yml)
[![License: MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](#license)

AI now writes much of the world's code, and the bottleneck has moved from *writing* code to *trusting* it. In Glyph, code can't hide what it does: every function signature states what the function may touch, which the compiler checks before anything runs, and what it promises, which is checked on every call. Proving those promises for all inputs is on the roadmap. Read [the vision](VISION.md) for where Glyph is going and what is true today.

```python
fn withdraw(balance: Int, amount: Int) -> Int
  requires amount > 0
  requires amount <= balance        # the caller must guarantee this
  ensures result >= 0               # this function guarantees this
{
  return balance - amount
}

fn report(name: Str, balance: Int) uses io {   # its only power: the terminal
  print(name + ": " + str(balance))
}

test "withdraw subtracts" {
  assert withdraw(10, 3) == 7
}
```

## Features

- **Effects in the signature.** A function lists everything it can touch (`uses io, fs`). Code that writes a file or prints without declaring it is rejected before it runs. Calling a function counts as using its effects, so a signature can't hide anything.
- **Static types.** Type errors are reported before the program runs, with hints.
- **Contracts with blame.** `requires` and `ensures` are checked on every call. A failed precondition is blamed on the caller; a failed postcondition is blamed on the function.
- **Built-in tests.** `test "name" { assert ... }` blocks live next to the code.
- **Native compiler.** `glyph build` produces a standalone executable. The test suite holds it to byte-identical behaviour with the interpreter, down to the error messages.
- **Memory without a garbage collector.** Native code frees every value the moment it's no longer used, and updates values in place when nothing else shares them. [More below](#memory-management).
- **Errors for agents.** With `--json`, every diagnostic is a JSON object with the exact values involved, so an AI can fix its own mistakes in a loop.
- **AI fixes, checked.** `glyph fix` asks Claude to repair errors and failing tests, and accepts a fix only if it passes Glyph's checks. [More below](#glyph-fix-ai-proposes-glyph-checks).

What an AI mistake looks like in Glyph:

```
$ glyph check examples/ai_mistakes.gly
examples/ai_mistakes.gly:5:3: effect error: `tax` performs effect `fs` (by calling `write_file`) but does not declare it
5 |   write_file("/tmp/log.txt", str(amount))
  |   ^
  hint: add `uses fs` to the signature if this is intended
```

## Install

You need [Rust](https://rustup.rs) and, for native builds, a C compiler (`cc`, `clang` or `gcc`).

```sh
git clone https://github.com/notaaaronfr/Glyph.git
cd Glyph
cargo install --path .
```

## Usage

```sh
glyph run     examples/bank.gly             # check, then interpret main
glyph run     examples/bench.gly --native   # compile to native code, then run
glyph build   examples/bench.gly -o bench   # standalone native executable
glyph test    examples/bank.gly             # run test blocks
glyph check   examples/ai_mistakes.gly      # static checks only
glyph effects examples/bank.gly             # what each function may do
glyph test    examples/bug.gly --json       # machine-readable results
glyph fix     examples/bug.gly              # AI-proposed fix, checked by Glyph
glyph run     examples/kernel/hello.gly --target kernel   # boot on bare metal in QEMU
```

## `glyph fix`: AI proposes, Glyph checks

`glyph fix` sends the problems Glyph found to Claude and checks every proposed fix. A fix is accepted only if it:

1. parses and passes every static check (types, names, effects)
2. adds **no new effects**: an AI can't "fix" a bug by quietly granting itself file or network access
3. keeps every `requires` / `ensures` clause and every existing test **unchanged**: it can't delete the check that caught the bug
4. passes all tests

Rejected proposals go back to the model with the reasons, up to `--tries` times (default 3). The file is only changed with `--write`.

**Known limits.** The guard compares contract and test text in the same file the model rewrites, so it can be bypassed: a model can rewrite a helper a contract calls, add a `requires`, or change whitespace inside a test's string literal. Candidate tests also run with real filesystem access. Each known bypass is a case in [`tests/redteam`](tests/redteam), and [the vision](VISION.md#roadmap) describes the replacement, `glyph accept`.

An illustrative session (the model's explanation will vary):

```
$ glyph fix examples/bug.gly
glyph fix: 1 problem(s) in examples/bug.gly
  attempt 1/3: asking claude-opus-5-5 ...
  attempt 1: verified (types, effects, contracts, tests)
The discount was added to the price instead of subtracted.

  {
-   return price + price * percent / 100
+   return price - price * percent / 100
  }

run again with --write to apply this fix to examples/bug.gly
```

What the guard says when a proposal cheats:

```
  attempt 1 rejected:
    - `discount` now declares `uses fs`, which the original did not allow; a fix may not add effects
    - the contract `ensures result <= price` of `discount` was removed or changed; a fix may not weaken contracts
    - test "half price" was changed; a fix may not edit existing tests
```

`glyph fix` needs `ANTHROPIC_API_KEY`, or a profile from `ant auth login`. Use `--model` to pick another Claude model. `--candidate other.gly` verifies a fix you already have, with no AI involved.

## Performance

`glyph build` compiles Glyph to C, then to native code with `cc -O2`. Example timings on Apple Silicon:

| Program | Interpreter | Native |
|---|---|---|
| `fib(32)` | 4.1 s | 0.01 s |
| [`examples/bench.gly`](examples/bench.gly) | 3.1 s | 0.03 s |
| [`examples/memory_bench.gly`](examples/memory_bench.gly) | 3.9 s | 0.06 s |

Python 3 runs `fib(32)` in 0.56 s.

```
source.gly → lexer → parser → checker (names, types, effects) ─┬→ interpreter (run, test)
                                                               └→ codegen → C → cc -O2 → executable
```

## Modules

A program can be split across files. `import lib.geometry` loads `lib/geometry.gly` (relative to the file you run), and everything from it is written with its module name, so code always says where a name comes from. Only `pub` items can be used from outside:

```python
# lib/geometry.gly
pub type Point { x: Int, y: Int }

pub fn dist2(a: Point, b: Point) -> Int {
  return square(a.x - b.x) + square(a.y - b.y)
}

fn square(n: Int) -> Int { return n * n }    # private to this file
```

```python
# app.gly
import lib.geometry

fn main() uses io {
  let p = geometry.Point { x: 3, y: 4 }
  print(geometry.dist2(p, p))
}
```

Effects are checked across files: calling `geometry.report()`, which `uses io`, requires the caller to declare `io` too. Each module is loaded once; import cycles and missing modules are reported with the chain or path involved. Errors, including runtime errors in native code, point at the module's own file and line. `glyph test` also runs the tests inside imported modules.

## Glyph OS: running on bare metal

Glyph programs can run with no operating system at all. `--target kernel` builds a kernel image for an ARM64 machine and boots it in QEMU:

```
$ glyph run examples/kernel/hello.gly --target kernel
Hello from Glyph OS!
No Windows, no macOS, no Linux: Glyph is running directly on the CPU.
  [x] boot
  [x] memory
  [x] serial port
status: ready / busy (42%)
built a list of 100000 items on the kernel heap
fib(25) = 75025
took 8 ms
```

The kernel platform ([src/kernel.c](src/kernel.c)) boots the CPU and provides a memory allocator, the serial port (`print` and `input`) and the hardware timer (`now`).

**Memory protection is on.** At boot the kernel switches on the MMU and caches with page tables that give each region of memory only the access it needs: code is read-only, data is never executable, device registers are uncached, and address 0 plus a guard region below the stack are unmapped. Bugs that would silently corrupt memory are caught and explained instead:

| Fault | Kernel says |
|---|---|
| Writing into code | `kernel panic: write to read-only memory` |
| Reading through a null pointer | `kernel panic: null pointer access` |
| Recursing past the end of the stack | `kernel panic: stack overflow` |
| Jumping into data | `kernel panic: tried to run memory that isn't code` |

**Interrupts drive the kernel.** A hardware timer interrupts 100 times a second, and the serial port interrupts when a key arrives. While a program waits, in `input()` or `sleep(ms)`, the CPU sleeps until the next interrupt instead of spinning. Measured with the shell waiting 3 seconds for input:

| | Host CPU used while waiting |
|---|---|
| Polling (v0.10) | 2.68 s of 3 s |
| Interrupts (v0.11) | 0.03 s of 3 s |

Glyph code itself can't do these things. The protection guards against bugs in the runtime and, later, in drivers and other programs. The tests trigger each fault on purpose with `GLYPH_KERNEL_CFLAGS=-DTN_SELFTEST=1` through `4`.

Separating code from data also made kernel programs about 8 to 15 times faster under QEMU, which re-translates code whenever a page holding code is written. QEMU doesn't model cache timing, so the effect of turning the caches on only shows on real hardware. Everything else is ordinary Glyph: records, enums, generics, modules, contracts, and the same reference-counted memory management. Errors show the same messages and source lines as on a desktop.

**Effects are what the hardware provides.** The kernel target has no file system yet, so a program whose functions declare `uses fs` is rejected at compile time instead of failing at runtime:

```
save.gly:1:16: effect error: `save` uses effect `fs`, which the kernel target doesn't provide
  hint: the kernel target provides: io, time, rand
```

The kernel also enforces effects at run time; see [Effects are permissions](#effects-are-permissions).

### Several apps at once

Give `run` several files and each becomes an app, running at the same time:

```
$ glyph run counter.gly clock.gly busy.gly crasher.gly --target kernel
counter: 1
busy: fib(28) = 317811
...
crasher.gly:12:15: contract error: precondition of `withdraw` violated: `amount <= balance`
[Glyph OS: app crasher stopped with exit code 1; the other apps keep running]
counter: 5
clock: 919 ms
counter: done
Glyph OS: all 4 apps finished
  counter      exit 0   cpu 0 ms   allowed io, time
  clock        exit 0   cpu 0 ms   allowed io, time
  busy         exit 0   cpu 200 ms   allowed io
  crasher      exit 1   cpu 0 ms   allowed io, time
```

- **Isolated.** Each app has its own stack, with a guard region below it, and its own heap. Apps are compiled separately, so they share no memory, not even runtime state.
- **Preemptive.** The timer interrupt switches apps every 10 ms, so an app that computes without pausing can't freeze the others.
- **Efficient waiting.** Apps in `sleep` or `input` are parked; when nothing is ready, the CPU idles.
- **Crash isolation.** An app that fails a contract, divides by zero or runs out of memory stops alone. The machine powers off when every app has finished, with the first failing app's exit code.
- **Whole lines.** Each `print` comes out in one piece, so output from different apps never mixes mid-line.

### Effects are permissions

The kernel grants each app exactly the effects its `main` declares, and checks every service call against them: printing and reading input need `io`, the clock and `sleep` need `time`, and random numbers need `rand`. `busy` above declares only `io`, so the kernel won't let it read the clock. Error reports are always allowed, so an app with no effects can still say why it failed.

The compiler already checks that Glyph code never uses an effect it didn't declare. The kernel checks again at run time, so an app whose compiled code was changed still can't get more than it declared through the kernel's services. Apps still run in kernel mode, so code that bypasses those services entirely is not stopped yet; user mode is the next step. The tests simulate exactly that: `GLYPH_KERNEL_GRANT="clock=io"` makes the kernel grant `clock` less than its code uses.

```
$ GLYPH_KERNEL_GRANT="clock=io" glyph run counter.gly clock.gly --target kernel
counter: 1
[Glyph OS: denied: app clock tried to read the clock, which needs `time`, but it is only allowed io]
[Glyph OS: app clock stopped with exit code 77; the other apps keep running]
counter: 2
...
```

The limit today: apps still run at the kernel's privilege level, in one address space, so code that skipped the kernel's services and touched the hardware directly wouldn't be checked. Running apps in user mode (EL0), with system calls and their own page tables, closes that gap and is the next step.

The demo apps are in [examples/kernel/apps](examples/kernel/apps). Up to 7 apps can run at once. There's also a small interactive shell, [examples/kernel/shell.gly](examples/kernel/shell.gly), with `help`, `echo`, `fib`, `sum`, `uptime` and `exit`.

To use the kernel target you need QEMU, clang and lld: `brew install qemu llvm lld` on macOS, or `apt install qemu-system-arm clang lld` on Debian or Ubuntu. CI boots the kernel on every change.

## Memory management

Natively compiled Glyph uses **precise reference counting with in-place reuse**, in the style of [Perceus](https://www.microsoft.com/en-us/research/publication/perceus-garbage-free-reference-counting-with-reuse/) (Koka, Lean 4):

- **No garbage collector, no pauses.** Every value is freed at the exact moment its last owner is done with it.
- **No leaks from cycles, by construction.** Glyph values are immutable and built bottom-up, so reference cycles can't exist, and reference counting alone reclaims everything. No cycle collector is needed.
- **In-place updates.** When a value has exactly one owner, `xs = push(xs, v)`, `p.x = v`, `line.to.y = v` and `c.log = push(c.log, v)` reuse its memory instead of copying. Your code keeps value semantics; the machine gets mutation's speed.
- **Moves instead of copies.** The compiler moves values on their last use in `x = f(x)` and `return x`, so the in-place updates above also work through function calls.
- **Deep structures are safe to free.** Freeing uses an explicit work list, not recursion, so dropping a structure a million levels deep can't overflow the stack.

On [`examples/memory_bench.gly`](examples/memory_bench.gly), which builds 1.4 million short-lived values:

| | Before (v0.6, never freed) | v0.7 |
|---|---|---|
| Peak memory | 349 MB | **29 MB** |
| Time | 0.12 s | **0.06 s** |
| Objects still allocated at exit | all of them | **0** |

[`tests/memory/reuse.gly`](tests/memory/reuse.gly) runs 300,000 updates using just **14 allocations**.

Every test program is also compiled with AddressSanitizer in CI, which aborts on any use-after-free or double free, and checked for zero leaks. To see the numbers for your own program:

```sh
GLYPH_MEMSTATS=1 glyph run app.gly --native
# glyph memstats: 1415650 allocated, 1415650 freed, 0 live at exit, peak 40023 live
```

## Language reference (v0.13)

| Feature | Syntax |
|---|---|
| Types | `Int` (64-bit, overflow is an error), `Str`, `Bool`, `Unit`, `List[T]`, records, enums, generics, `Any` (interpreter only) |
| Records | `type Point { x: Int, y: Int }`, `Point { x: 1, y: 2 }`, `p.x`, `p.x = 5`, `line.to.y = 3` |
| Enums | `enum Shape { Circle { radius: Int }, Empty }`, `Circle { radius: 2 }`, `Empty` |
| Match | `match s { Circle { radius } => { ... } Empty => { ... } }`, `Rect { w: width } => ...`, `_ => ...` |
| Generics | `type Pair[A, B] { ... }`, `enum Tree[T] { ... }`, `fn first[T](xs: List[T]) -> Option[T]` |
| Built-in types | `Option[T]` (`Some { value }`, `None`), `Result[T, E]` (`Ok { value }`, `Err { error }`) |
| Modules | `import lib.geometry`, `pub fn` / `pub type` / `pub enum`, `geometry.area(s)`, `geometry.Point { ... }` |
| Function | `fn name(a: Int) -> Int uses io requires ... ensures ... { ... }` |
| Variables | `let x = 1`, `let xs: List[Int] = []`, `x = x + 1` |
| Control flow | `if / else if / else`, `while`, `for x in list_or_string`, `return` |
| Operators | `+ - * / %`, `== != < <= > >=`, `and or not`, `xs[i]` |
| Effects | `io` (terminal), `fs` (files), `time` (clock, sleeping), `rand` (randomness) |
| Pure built-ins | `len str int abs push range join split contains`, `parse_int` (returns `Option[Int]` instead of crashing) |
| Effectful built-ins | `print input` (io), `read_file write_file` (fs), `now sleep` (time), `random` (rand) |

Contracts must be pure, and `ensures` sees the arguments as they were passed in, even if the body reassigns them. Comments start with `#`. There are no semicolons; newlines separate statements.

Lists and records are values: `push(xs, v)` returns a new list and never changes `xs` (it's still O(1)), and `q = p` followed by `q.x = 5` leaves `p` unchanged. `==` compares contents.

```python
type Point {
  x: Int
  y: Int
}

type Line { from: Point, to: Point }

fn move_right(p: Point, by: Int) -> Point
  requires by >= 0
  ensures result.x == p.x + by
{
  p.x = p.x + by
  return p
}
```

An enum value is one of several variants. `match` must handle every variant (or end with `_`), so adding a variant later flags every `match` that forgets it, before the program runs:

```python
enum Shape {
  Circle { radius: Int }
  Rect { w: Int, h: Int }
  Empty
}

fn area(s: Shape) -> Int {
  match s {
    Circle { radius } => { return 3 * radius * radius }
    Rect { w, h } => { return w * h }
    Empty => { return 0 }
  }
}
```

```
shapes.gly:9:3: type error: `match` doesn't handle: Triangle
9 |   match s {
  |   ^
  hint: add an arm for each, or `_ => { ... }` for the rest
```

Records, enums and functions can be generic. Type arguments are inferred, and the native compiler generates a specialised copy of each generic function for every type it's used with, so generic code runs as fast as hand-written code:

```python
fn first[T](xs: List[T]) -> Option[T] {
  if len(xs) == 0 { return None }
  return Some { value: xs[0] }
}

fn safe_div(a: Int, b: Int) -> Result[Int, Str] {
  if b == 0 { return Err { error: "division by zero" } }
  return Ok { value: a / b }
}

let x: Option[Int] = first([])     # None: the annotation says what `T` is
print(first(["a", "b"]))           # Some { value: "a" }
```

## Status and roadmap

Glyph is **early and experimental**: expect breaking changes.

- [x] Interpreter, static types, effects, contracts, tests, JSON diagnostics
- [x] Native compiler, held to identical behaviour by the test suite
- [x] Records
- [x] Enums and exhaustive `match`
- [x] Generics, with built-in `Option[T]` and `Result[T, E]`
- [x] Memory management for native code: precise reference counting with in-place reuse
- [ ] Reuse of matched enum values (Perceus "reuse analysis"), borrowed parameters
- [x] Modules, with `pub` visibility and effects checked across files
- [x] `glyph fix`: AI repair loop. Fixes are accepted only if they type-check, add no effects, keep every contract and test, and pass (with [known bypasses](tests/redteam))
- [ ] `uses ai`: model calls as a typed, declared effect
- [ ] `glyph build --tune`: learned, meaning-preserving optimization
- [ ] Finer capabilities (`uses net("api.example.com")`), property tests, compile-time contract proofs
- [ ] Editor support (LSP), package manager
- [x] Glyph runs on bare metal (`--target kernel`): boots in QEMU, serial I/O, timer, kernel heap
- [x] Memory protection on the kernel: MMU, caches, read-only code, no-execute data, null and stack guards
- [x] Interrupts on the kernel: GIC, 100 Hz timer tick, interrupt-driven serial input, `sleep` that idles the CPU
- [x] Several apps at once: preemptive scheduler, per-app stacks and heaps, crash isolation
- [x] Effects as permissions: the kernel grants each app what its `main` declares and stops an app that asks for more
- [ ] Glyph OS next steps: apps in user mode (EL0) with system calls and per-app address spaces, messages between apps, a device driver interface
- [ ] **Glyph OS** as a whole system where every program's effects are declared, checked by the compiler and enforced by the kernel. [VISION.md](VISION.md#glyph-os-a-shipped-track-held-to-the-gate) says how OS work is ordered

The phased plan, with an exit test for every phase, is in [VISION.md](VISION.md#roadmap).

## Contributing

Contributions of every size are welcome: examples, tests, error messages, docs and language features. Start with [CONTRIBUTING.md](CONTRIBUTING.md), and please follow the [Code of Conduct](CODE_OF_CONDUCT.md).

## License

Glyph is dual-licensed under either of

- [MIT License](LICENSE-MIT)
- [Apache License, Version 2.0](LICENSE-APACHE)

at your option. Unless you explicitly state otherwise, any contribution you intentionally submit for inclusion in Glyph shall be dual-licensed as above, without any additional terms or conditions.
