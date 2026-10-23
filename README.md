# Glyph

**The language AI writes and humans trust.**

[![CI](https://github.com/notaaaronfr/Glyph/actions/workflows/ci.yml/badge.svg)](https://github.com/notaaaronfr/Glyph/actions/workflows/ci.yml)
[![License: MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](#license)

AI now writes much of the world's code, and the bottleneck has moved from *writing* code to *trusting* it. In Glyph, code can't hide what it does: every function signature is a complete, machine-checked statement of what the function may touch and what it promises. Read [the vision](VISION.md).

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
- **Native compiler.** `glyph build` produces a standalone executable that behaves identically to the interpreter, down to the error messages.
- **Errors for agents.** With `--json`, every diagnostic is a JSON object with the exact values involved, so an AI can fix its own mistakes in a loop.
- **AI fixes, verified.** `glyph fix` asks Claude to repair errors and failing tests, and accepts a fix only if Glyph can verify it. [More below](#glyph-fix-ai-proposes-glyph-proves).

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
glyph fix     examples/bug.gly              # AI-proposed fix, verified by Glyph
```

## `glyph fix`: AI proposes, Glyph proves

`glyph fix` sends the problems Glyph found to Claude and checks every proposed fix. A fix is accepted only if it:

1. parses and passes every static check (types, names, effects)
2. adds **no new effects**: an AI can't "fix" a bug by quietly granting itself file or network access
3. keeps every `requires` / `ensures` clause and every existing test **unchanged**: it can't delete the check that caught the bug
4. passes all tests

Rejected proposals go back to the model with the reasons, up to `--tries` times (default 3). The file is only changed with `--write`.

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
| `fib(32)` | 4.1 s | 0.02 s |
| [`examples/bench.gly`](examples/bench.gly) | 3.1 s | 0.03 s |

Python 3 runs `fib(32)` in 0.56 s.

```
source.gly → lexer → parser → checker (names, types, effects) ─┬→ interpreter (run, test)
                                                               └→ codegen → C → cc -O2 → executable
```

## Language reference (v0.5)

| Feature | Syntax |
|---|---|
| Types | `Int` (64-bit, overflow is an error), `Str`, `Bool`, `Unit`, `List[T]`, records, enums, `Any` (interpreter only) |
| Records | `type Point { x: Int, y: Int }`, `Point { x: 1, y: 2 }`, `p.x`, `p.x = 5`, `line.to.y = 3` |
| Enums | `enum Shape { Circle { radius: Int }, Empty }`, `Circle { radius: 2 }`, `Empty` |
| Match | `match s { Circle { radius } => { ... } Empty => { ... } }`, `Rect { w: width } => ...`, `_ => ...` |
| Function | `fn name(a: Int) -> Int uses io requires ... ensures ... { ... }` |
| Variables | `let x = 1`, `let xs: List[Int] = []`, `x = x + 1` |
| Control flow | `if / else if / else`, `while`, `for x in list_or_string`, `return` |
| Operators | `+ - * / %`, `== != < <= > >=`, `and or not`, `xs[i]` |
| Effects | `io` (terminal), `fs` (files), `time` (clock), `rand` (randomness) |
| Pure built-ins | `len str int abs push range join split contains` |
| Effectful built-ins | `print input` (io), `read_file write_file` (fs), `now` (time), `random` (rand) |

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

## Status and roadmap

Glyph is **early and experimental**: expect breaking changes. Natively compiled programs don't free memory yet.

- [x] Interpreter, static types, effects, contracts, tests, JSON diagnostics
- [x] Native compiler with identical behaviour
- [x] Records
- [x] Enums and exhaustive `match`
- [ ] Generic types (`Option[T]`, `Result[T, E]`), modules
- [ ] Memory management for native code
- [x] `glyph fix`: AI repair loop. Fixes are accepted only if they type-check, add no effects, keep every contract and test, and pass
- [ ] `uses ai`: model calls as a typed, declared effect
- [ ] `glyph build --tune`: learned, meaning-preserving optimization
- [ ] Finer capabilities (`uses net("api.example.com")`), property tests, compile-time contract proofs
- [ ] Editor support (LSP), package manager
- [ ] The north star: **Glyph OS**, where the kernel enforces effects as permissions

See [VISION.md](VISION.md) for the full picture.

## Contributing

Contributions of every size are welcome: examples, tests, error messages, docs and language features. Start with [CONTRIBUTING.md](CONTRIBUTING.md), and please follow the [Code of Conduct](CODE_OF_CONDUCT.md).

## License

Glyph is dual-licensed under either of

- [MIT License](LICENSE-MIT)
- [Apache License, Version 2.0](LICENSE-APACHE)

at your option. Unless you explicitly state otherwise, any contribution you intentionally submit for inclusion in Glyph shall be dual-licensed as above, without any additional terms or conditions.
