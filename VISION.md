# The Glyph vision

## The problem

For seventy years, programming languages were designed around one bottleneck: **humans writing code**. Python, for example, exists to make writing code easy.

That bottleneck is disappearing. AI now writes a large and growing share of the world's code, faster than any person. The new bottleneck is **trust**: knowing what AI-written code actually does before you run it.

Today, trusting AI code means reading every line and hoping you didn't miss the one that writes to disk, calls a server or breaks an edge case. That doesn't scale.

## The idea

**Glyph is a language where code can't hide what it does.**

A glyph is a symbol with one exact meaning. In Glyph, every function signature is like that: a complete, machine-checked statement of what the function may touch and what it promises.

```python
fn withdraw(balance: Int, amount: Int) -> Int
  requires amount <= balance     # what callers must guarantee
  ensures result >= 0            # what this function guarantees
{
  return balance - amount
}

fn save_report(r: Str) uses fs { # the only power this function has
  write_file("report.txt", r)
}
```

- **Effects** (`uses io, fs, ...`) are the complete list of a function's powers. The compiler rejects code that does more than it declares, directly or through anything it calls.
- **Contracts** (`requires` / `ensures`) state behaviour and are checked on every call, with blame: the caller broke the rule, or the function did.
- **Types and tests** are built in, and every diagnostic is available as JSON so AI agents can read and act on it.

A human reviewer doesn't need to read 500 lines. They read the signatures, which the compiler guarantees are true.

## Principles

1. **Explicit beats convenient.** AI doesn't mind typing, so Glyph favours code that says everything over code that is short. Inference that hides meaning is not a feature.
2. **AI proposes, Glyph proves.** AI belongs everywhere in the toolchain: writing code, fixing bugs, tuning performance. But nothing an AI produces is accepted without passing types, effects, contracts and tests. In particular, an AI fix can never silently add a new effect.
3. **Write high, run low.** Code is written at a high level, where intent is visible and checkable. The compiler does the low-level work. Glyph compiles to native code, and the interpreter and native compiler must behave identically.
4. **Errors are interfaces.** Every error says what went wrong, where, with which values, and how to fix it, for humans and machines alike.
5. **Small and sharp.** A feature joins the language only if it makes programs more checkable or more useful without making them less clear.

## Where we're going

**Near term: the language.** Enums and pattern matching (records shipped in v0.4), modules, memory management for native code, a faster interpreter, editor support (LSP), and a package manager.

**AI in the toolchain.**
- `glyph fix` (shipped in v0.3): an AI repair loop. A proposed fix is accepted only if it type-checks, adds no new effects, keeps every contract and test unchanged, and passes.
- `uses ai`: calling a model as a typed, declared effect, with structured output, retries and cost limits built in.
- `glyph build --tune`: the compiler benchmarks optimization choices and learns which are fastest. Every choice it can make preserves meaning.

**Stronger guarantees.** Finer capabilities (`uses fs.read("./data")`, `uses net("api.example.com")`), property-based testing generated from contracts, and proving contracts at compile time with an SMT solver.

**The north star: Glyph OS.** An operating system where effects aren't just checked by the compiler but enforced by the kernel. An app declared `uses net, io` would be unable to read your files, because the system grants exactly what the signature states. Permissions become part of the code, verifiable before anything runs. Oberon, Singularity and Midori showed that a language and an OS designed together can be safer than either alone. Glyph aims to bring that idea to the AI era.

The path there is long, and every step makes Glyph useful on its own. If this vision resonates with you, [come build it](CONTRIBUTING.md).
