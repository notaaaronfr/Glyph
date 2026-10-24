# 0001. Glyph's direction: the language is the trust boundary

- Status: accepted
- Date: 2026-10-24
- Decided by: the project owner, after a three-way design comparison

## Context

Glyph v0.13 checks effects statically and contracts at runtime, has modules, and boots on bare metal as Glyph OS, whose kernel enforces each app's declared effects as permissions. Its only acceptance mechanism, the `glyph fix` guard, compares the text of contracts and tests in the same file the agent rewrites, and seven bypasses (G1 to G7) reproduce at commit `078ad3b`. The old vision claimed reviewers could trust signatures "which the compiler guarantees are true", which was false for contracts.

The project's goals are verification strong enough to merge on without reading the code, a reward signal that RL and self-improving agents cannot game, and safe harness engineering, where an agent rewrites its own environment.

The evidence that shaped the decision:

- Model fluency in a verification language dominates agent success: 82% Dafny, 44% Verus, 27% Lean on the same tasks (arXiv 2509.22908).
- Agents routinely game verifiers through escape hatches, weakened specs, added preconditions and edited proof context (arXiv 2412.06176, 2603.21065, 2602.09464).
- Weak specs, not checker bugs, cause most residual failures (AWS Cedar: 20 of 20 pre-launch mismatches came from specs).
- Vercel Zero already ships agent-friendly diagnostics and effects without verification. Bend 2 pitches proofs with a Lean-formalized kernel.

## Options compared

Three drafts were written independently, each committed to a different moat, then scored by a judge from another model family against a fixed rubric.

1. **Language first.** Specs live in owned spec units, the agent's grammar has no escape hatches, and one Core representation is run, verified and compiled. Judge score 27/30.
2. **Acceptance kernel first.** A small trusted kernel issues signed verdicts; Glyph is its best front end, and Dafny is accepted as an oracle during the cold start. Judge score 22/30. It lost on reward determinism (budgets in CPU milliseconds, record ids that include the verdict) and the thinnest harness story.
3. **Harness runtime first.** A Wasm capability runtime with epochs and a ledger; proofs cover only small policy functions. Judge score 27/30. It lost because Wasmtime dominates its trusted base, it freezes the native backend, and it openly risks making the language optional.

## Decision

Glyph is the language agents write their tools and harnesses in, and the trust boundary lives in the language. The base is option 1. It takes from option 3 the harness ledger (promote, approve, rollback, canaries), host-written evidence, the `needs_human` tier and a floor on accepting good candidates. It takes from option 2 the fast P0a, the three-way verdict and probes that can only lower a verdict. It adds what all three drafts missed: reproducible toolchain builds, recorded model responses during acceptance, honest `unsupported` reporting for string properties, and a spec-authoring workflow. [VISION.md](../../VISION.md) is the full statement.

The owner also decided:

- **The P2 fluency comparison with Dafny is advisory.** It is measured and published, and a ratio below 0.3 opens a public review, but it does not stop language work.
- **Glyph OS is a shipped track, held to the gate.** The comparison above was first run against v0.7, when Glyph OS was only an aspiration, and the owner placed it beyond 1.0. By the time this record was written, v0.9 to v0.13 had shipped the kernel, so the owner revisited that choice: the kernel's effect enforcement is trusted code under the same governance and trusted-base budget, hardening it may proceed alongside P0b to P2, and new OS surface waits until P2 ships.
- **Sealed held-out test splits** are held by maintainers outside the repository and published as commit-reveal hashes.

Not yet decided by the owner: the P5 language-value test against a Python baseline is drafted as advisory, by extension from the fluency decision. The owner was not asked about it, and may want it binding.

## Consequences

- The repository is split into crates along the trust boundary, and trusted crates are marked in their Cargo metadata (P0a). The kernel gets its own trusted crate, `glyph-kernel`.
- The guard in `src/fix.rs` is replaced, not patched: spec and impl units make G1 to G4 unrepresentable (P0b).
- Every capability claim in documentation carries a shipped or phase label, and phases end in exit scripts under `scripts/exit/`.
- With the fluency gate advisory, nothing in the plan stops work if models stay poor at Glyph. The published M2 number is the only signal, so it must be measured on schedule.
