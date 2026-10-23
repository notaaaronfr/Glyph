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
```

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

## Language reference (v0.2)

| Feature | Syntax |
|---|---|
| Types | `Int` (64-bit, overflow is an error), `Str`, `Bool`, `Unit`, `List[T]`, `Any` (interpreter only) |
| Function | `fn name(a: Int) -> Int uses io requires ... ensures ... { ... }` |
| Variables | `let x = 1`, `let xs: List[Int] = []`, `x = x + 1` |
| Control flow | `if / else if / else`, `while`, `for x in list_or_string`, `return` |
| Operators | `+ - * / %`, `== != < <= > >=`, `and or not`, `xs[i]` |
| Effects | `io` (terminal), `fs` (files), `time` (clock), `rand` (randomness) |
| Pure built-ins | `len str int abs push range join split contains` |
| Effectful built-ins | `print input` (io), `read_file write_file` (fs), `now` (time), `random` (rand) |

Contracts must be pure. Comments start with `#`. There are no semicolons; newlines separate statements. Lists are values: `push(xs, v)` returns a new list and never changes `xs` (it's still O(1)).

## Status and roadmap

Glyph is **early and experimental**: expect breaking changes. Natively compiled programs don't free memory yet.

- [x] Interpreter, static types, effects, contracts, tests, JSON diagnostics
- [x] Native compiler with identical behaviour
- [ ] Records and enums, modules
- [ ] Memory management for native code
- [ ] `glyph fix`: AI repair loop. Fixes are accepted only if they type-check, add no new effects, and pass every contract and test
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
