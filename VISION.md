# The Glyph vision

> Glyph is at **v0.13** (commit `078ad3b`). Every capability below carries a label. **[v0.7]** means the code did it at v0.7 and still does; **[v0.8]** to **[v0.13]** mark what later releases added. **[P0]** to **[P6]** name the roadmap phase that makes it true (P0 has two parts, P0a and P0b), and every phase ends in a script that checks it. A sentence with no label is a design commitment, not a capability. Where this document and the code disagree, the code is right and this document has a bug.

## Mission

Glyph is the language AI agents write their tools and harnesses in. Every change an agent makes is checked against promises a human owns, and the check is strict enough to merge on without reading the code and to train on as a reward.

## The problem

AI writes code faster than people can review it. The code under review now includes the agent's own harness: its prompts, planner, tool router, retry and budget logic. A mistake in a harness is not one bad function. It is a loop that repeats the mistake, and an agent allowed to rewrite its harness can rewrite the judge.

Formal verification should replace most of that review. Today's verified languages fail agents in three ways.

1. **Models are not fluent in them.** On the same tasks, the union of nine frontier models solved 82% in Dafny, 44% in Verus and 27% in Lean (Vericoding, arXiv 2509.22908). One model in AlgoVeri scored 56%, 26% and 9% (arXiv 2602.09464).
2. **Their verifiers get gamed.** Agents write `assume(false)`, `sorry` and `ensures true`. They add preconditions, ship degenerate algorithms and edit the proof context around a fixed statement. One self-improving generator spread `assume(false)` to every program it wrote (AlphaVerus, arXiv 2412.06176). A prover under RL raised its pass rate by editing context its checker left writable (LongCat, arXiv 2603.21065). If the files an agent writes can contain an escape hatch, the verifier is not a reward. It is a puzzle the agent learns to solve.
3. **None of them controls authority.** A verified Dafny method, Verus function or Lean program can still reach the filesystem or the network. Nothing in those languages says which files a tool may touch, and nothing stops a harness from writing its own evaluation log.

Glyph's own guard has the same problems today. `glyph fix` [v0.3] protects specs by comparing the text of two copies of the same file, and seven bypasses reproduce at v0.13 (commit `078ad3b`). **G1**: a helper a contract depends on can be edited. **G2**: whitespace inside string literals is normalized away. **G3**: an added `requires` is accepted. **G4**: an unparseable baseline drops all protection. **G5**: `ensures` is only checked at runtime, on the inputs tests happen to use. **G6**: a rejected candidate's tests have real filesystem effects. **G7**: the interpreter and native code disagree on Unicode whitespace in `int`. The test timeout also stops waiting for its worker thread without stopping it. P0a fixes G7 and records G1 to G6 in `tests/redteam`.

## The bet: the trust boundary belongs in the language

The properties that make a checker safe to train against can be built into a language and cannot be bolted onto one.

- **Specs are owned, code is proposed.** Contracts, spec functions, owner tests, grants and `extern` assumptions live in spec units (`*.spec.gly`). Agents write impl units (`*.gly`). The impl grammar has no way to write a public contract, an `assume`, an `extern`, a grant or a spec function. A cheat does not parse, so there is nothing to compare and no guard to keep in sync. **[P0]**
- **No heap in the proofs.** Glyph values are immutable and have value semantics [v0.7]. The native runtime updates a value in place only when nothing else can observe it [v0.7]. Verification conditions mention arithmetic, records, enums and lists, never aliasing, frames or ownership.
- **One meaning.** A single Core representation is executed by the interpreter, checked by the verifier and compiled by the native backend, so evaluation order and built-in behaviour agree by construction. **[P0]** for the interpreter, **[P1]** for the verifier and native backend.
- **Decidable where possible, and labelled.** Every proof obligation reports its tier. **D** is quantifier-free linear arithmetic over integers, records, enums and list length and indexing. **Q** adds bounded `all`/`any` over lists and recursive spec functions, with triggers chosen by the compiler, never by users. **N** is nonlinear arithmetic. String properties are not in any tier yet and report `unsupported`. **[P1]**
- **No ambient authority.** Effects say what kind of thing a function may do [v0.7], and on Glyph OS the kernel refuses an app any effect its `main` does not declare [v0.13]. Capabilities say which resource [P3]. A capability is an unforgeable value, such as a `Dir[Read]` scoped to a path or a `Model` with a token budget, and only a spec's `grants` block creates one. Handles such as open files are linear, so using a handle after closing it, or leaking it, does not compile. **[P3]**
- **Deterministic cost.** Fuel is part of the semantics: a step count that both backends meter identically. Timeouts, budgets and rewards never depend on the machine. **[P0]** interpreter, **[P1]** native.

Three walls follow from this, and each catches the failures of the one before it. The **static** wall: the compiler derives authority and obligations from source, and the gate refuses growth. The **dynamic** wall: the host grants capabilities, and guest code has no other way out. The **evidential** wall: every decision rests on the owner's spec and on evidence the host wrote, never on anything the candidate wrote.

## The core promise

> If `glyph accept` records **proved** for a function, then no input its spec allows can make that function, under Glyph's reference semantics, return a result its spec forbids, fail at runtime, run forever, perform an undeclared effect, or touch a resource its spec does not grant. A candidate's reward is a deterministic function of the owner's spec, the toolchain, the reward policy and recorded external responses, and the candidate can write none of them.

One counterexample falsifies it. It can be an impl unit and an input that `glyph accept` passes but that violates the spec when run, or a candidate whose reward changes by any route other than changing its own behaviour. Each such counterexample is a soundness bug, handled as a security issue under `SECURITY.md`, and becomes a permanent case in the red-team suite.

A gate that rejects everything would satisfy that sentence, so the promise has a second half: at least 90% of a curated corpus of known-good candidates must be accepted. Both halves are measured on every release (M1).

**Today [v0.13] only one part holds.** A function cannot perform an effect kind (`io`, `fs`, `time`, `rand`) it does not declare, directly or through anything it calls, and on Glyph OS the kernel also refuses it at runtime. Contracts are checked at runtime on the calls that actually happen, not proved. Phases P0 to P4 make the rest true.

## A Glyph module

```python
# compact.spec.gly: owned by the project. Agents never write *.spec.gly files.
type Msg { role: Role, tokens: Int }

spec fn total(xs: List[Msg]) -> Int decreases len(xs) {   # spec context: Int is a mathematical integer
  if len(xs) == 0 { return 0 }
  return xs[0].tokens + total(drop(xs, 1))
}

pub fn fit(msgs: List[Msg], limit: Int) -> List[Msg]      # keep the newest messages that fit
  requires limit >= 0 and all m in msgs: m.tokens >= 0
  ensures result == drop(msgs, len(msgs) - len(result))   # a suffix, in order
  ensures total(result) <= limit                          # it fits
  ensures len(result) == len(msgs) or total(drop(msgs, len(msgs) - len(result) - 1)) > limit  # one more would not
```

```python
# compact.gly: written by an agent. `fit` gets its contracts from the spec; this file cannot add any.
fn fit(msgs: List[Msg], limit: Int) -> List[Msg] {
  let i = len(msgs)
  let used = 0
  while i > 0 and msgs[i - 1].tokens <= limit - used     # `used + tokens <= limit` could overflow; Glyph proves this cannot
    invariant 0 <= i and i <= len(msgs) and 0 <= used and used <= limit
    invariant used == total(drop(msgs, i))
    decreases i
  {
    used = used + msgs[i - 1].tokens
    i = i - 1
  }
  return drop(msgs, i)
}
```

Without the last `ensures`, `return []` would satisfy the contract. `glyph spec audit` [P2] reports that before any agent trains against the spec.

## Trust model

| Capability | Status | What is guaranteed | Under which assumptions |
|---|---|---|---|
| Static types, records, enums, generics, exhaustive `match` | [v0.7] | Type errors before anything runs | `Any` is an unchecked hole; it leaves the language in [P1] |
| Modules | [v0.8] | Imported items get qualified names; import cycles and missing modules are errors | A2. The loader decides which code a program contains, so it is trusted |
| Effect kinds, checked statically and transitively | [v0.7] | A function performs only the effect kinds it declares | A2. In the interpreter and native executables, effects are **not enforced at runtime**: a program that declares `fs` touches the real filesystem |
| `requires` / `ensures` with blame | [v0.7] | Checked at runtime on each call that happens | Runtime only. `check --json` reporting `ok` does not mean a contract holds for all inputs (G5) |
| Interpreter and native output identical | [v0.7] on the snapshot suite | Byte-identical output on every program in `tests/cases` | Tested, not proved. G7 (`int` whitespace and string quoting) was a divergence until P0a |
| Reference counting with in-place reuse, ASan in CI | [v0.7] | No use-after-free or double free on the memory suite | `runtime.c` is trusted for native claims |
| Glyph OS: bare metal, MMU, interrupts, several isolated apps with preemptive scheduling | [v0.9] to [v0.12] | Apps boot and run on QEMU's AArch64 `virt` machine, each with its own heap | A5. Tested in QEMU in CI, not proved |
| Glyph OS enforces declared effects as permissions | [v0.13] | The kernel grants each app the effects its `main` declares and refuses the rest | A2, A5. Apps still run in kernel mode, so code that skips the kernel API is not stopped (user mode is the next OS step). The grant table is built by trusted code from the checked program, and a test override in that code, `GLYPH_KERNEL_GRANT`, can change it |
| `glyph fix` guard | [v0.7], **known bypassable** | Rejects added effects and edited contract or test text | G1 to G4 and G6 bypass it. Replaced by `glyph accept` [P0] |
| Spec and impl units, content identities, acceptance records | [P0b] | Spec parts come only from spec units; literals are exact; no spec unit means no acceptance | A2, A8 |
| Candidates run against a virtual world under fuel, in a child process | [P0b] | No effect outside the virtual world; infinite loops end as `exhausted` | A1, A5 |
| Proofs in tiers D and Q, invariants, termination, replayed counterexamples | [P1] | `proved` means totally correct for the spec | A1 to A4, A6 |
| Capabilities, grants, linear handles, `uses ai` | [P3] | Accepted code touches only resources reachable from its grants | A1, A2, A5 |
| Reward policies, stability trials, sealed tests, `cost` clauses | [P4] | Rewards are bit-identical across supported platforms | A8, A9 |
| Harness ledger: promote, approve, rollback, canaries | [P5] | No generation goes live with authority its parent lacked, without a human signature | A2, A5 |
| Solver certificates, second solver, Lean-formalized Core | [P6] | Tier-D soundness proved; solver answers cross-checked | A3, A4 shrink |

**Assumptions.**

- **A1. Reference semantics.** Core Glyph means what `glyph-run` does. Native code is tested against it, not proven equal.
- **A2. Front end.** Parsing and elaboration into Core are correct.
- **A3. Proof soundness.** A valid verification condition implies the property under A1. Counterexample replay and the red-team suite test this; Lean proves tier D in [P6].
- **A4. Solver.** The pinned SMT solver is right when it answers `unsat`. Certificates shrink this in [P6].
- **A5. Platform.** Rust memory safety (trusted crates forbid `unsafe`), the OS process boundary and the hardware. For Glyph OS: the kernel's C, the AArch64 MMU and the machine QEMU emulates. Side channels are out of scope.
- **A6. The spec says what its owner meant.** Never guaranteed. Spec audit measures it.
- **A7. `extern` contracts in spec units are true.** Every record lists the externs it relied on.
- **A8. Reproducible toolchain.** A `ToolchainId` names a pinned Rust toolchain, `--locked` dependencies and a pinned solver binary, built reproducibly per platform. Without this, cross-machine replay cannot be measured.
- **A9. Recorded external responses.** During acceptance, model and service calls are served from recorded tapes, so a verdict is a function of the candidate. Live calls happen only in evaluation runs, which are seeded and whose first record is final.

**Trusted:** `glyph-front`, `glyph-run`, `glyph-verify`, `glyph-accept`, the prelude, the pinned solver, reward policies and sealed splits. `glyph-kernel` (the kernel and the code that builds its grant table and boot image) is trusted for claims about Glyph OS, and counts toward the same trusted-base budget (M10). `glyph-native` and `runtime.c` are trusted only for claims about native binaries. CI derives this list from crate metadata.
**Not trusted:** models, agents, prompts, impl units, candidate-written tests, `glyph fix`, the CLI, the formatter, editor tooling and every harness implementation.

## Verification as a training signal

**The environment [P4].** `glyph env serve` speaks JSON lines over stdio. `reset` returns a task: its spec units, the language reference card and the write set. `step` takes file *contents* for the write set, never paths, runs `glyph accept` in a fresh process, and returns the record and the reward. A file outside the write set is rejected before it is parsed.

**The verdict has three outcomes.** `Rejected` (a rule failed), `Unknown` (fuel or solver budget exhausted, or an unsupported construct) and `Accepted` at an assurance level: `checked` < `tested` < `proved`. `Unknown` pays exactly what `Rejected` pays, so running out the clock never beats failing, and the two are logged apart so a curriculum can tell hard from wrong. Probes can only lower a verdict, never raise one, so a buggy probe costs reward and never soundness.

**The reward is a Glyph program [P4].** A reward policy is a pure function from the record to basis points (0 to 10000), proved like any other Glyph function, for example to return 0 whenever an obligation is refuted. Released policies are never edited; a change is a new `PolicyId`.

- `binary-v1` pays 10000 only when every spec obligation is proved and stable, every owner and sealed test passes and every cost bound holds.
- `shaped-v1` gives partial credit per spec obligation proved. Obligations the agent creates (helper contracts, invariants, overflow checks) are gates that earn no credit, so scaffolding cannot raise a score.
- The record is a vector: authority verdict, spec verdict, every obligation's status, probe results, test and cost results. Hard-rule failures are logged by cause, so a trainer sees hacking attempts rise before any succeed.

**How each documented hack is closed.**

| Hack seen in the literature or in Glyph | Defense | Phase |
|---|---|---|
| `assume(false)`, `sorry`, `admit` | Not in the impl grammar: a parse error | P0 |
| `ensures true`, deleted or edited tests | Contracts and owner tests exist only in spec units | P0 |
| Added `requires` (G3), edited spec helper (G1) | Same rule; `spec diff` labels a narrowed domain | P0, P1 |
| Edited context around a fixed statement | The whole spec closure is content-hashed into `SpecId` | P0 |
| Literal tricks (G2), missing baseline (G4) | Identities keep literals exact; no spec unit means no acceptance | P0 |
| Runtime-only contracts (G5) | Records separate `proved` from `tested`; rewards count proofs | P1 |
| Side effects while evaluated (G6), leaked workers | Virtual world, fuel, child process | P0 |
| Backend differences (G7) | Built-ins defined once, in a Glyph prelude; differential fuzzing | P0, P1 |
| Forged or deleted evidence (a harness rewriting its own logs or outcomes) | Journals, outcomes and scores are written by the host; no guest import writes them | P0, P5 |
| Bypassing the sandbox (STOP measured how often self-improved scaffolds did this, arXiv 2310.02304) | The sandbox is not a setting the guest can see; the host builds the grant from the spec | P0 |
| Trivial and vacuous solutions | Spec audit runs trivial candidates and checks every `requires` is satisfiable | P2, P4 |
| Degenerate algorithms | `cost` clauses over fuel, checked on sealed large inputs | P4 |
| Overfitting to visible tests | Sealed tests, published as commit-reveal hashes; promotions report only the decision | P4 |
| Re-rolling noise until a pass | `RecordId` hashes the inputs, not the verdict, and the first record is final | P0 |
| Unstable proofs, lucky timeouts | Five stability trials with shuffled assertions and seeds; a flip makes the obligation `unknown` | P4 |
| Trusted-base leaks (`native_decide`, the `apply?` bug) | Proofs never evaluate compiled code; counterexamples are replayed; certificates in P6 | P1 |
| Nondeterministic model calls | Acceptance uses recorded tapes (A9) | P4 |
| Reward tampering | Checker, policy and records sit outside the candidate's write set and process | P0, P4 |
| Slow erosion across epochs | Each new epoch must still reject the whole red-team corpus and must not lower an anchor suite that never rotates | P5 |

**What stays open is the spec.** In AWS Cedar, all 20 pre-launch mismatches came from specs and none from proofs. Glyph cannot certify intent, so it makes writing and reviewing specs a first-class workflow. `glyph spec extract` drafts a spec unit from an existing file. `glyph spec audit` attacks it with trivial candidates, vacuity checks and mutants. A breaker agent, rewarded only for finding a weak spec and never sharing a reward with an implementing agent, runs before any spec is locked. `glyph spec diff` classifies changes by implication, so a human reviews "the domain narrowed for these inputs", not a text diff. A human signs every spec lock. That signature is the review Glyph keeps, and M12 measures what it costs.

## Harness engineering: an agent changing its own harness

A harness is a Glyph program [P5]. Its spec, owned by the project, fixes four things.

- **Grants**: a workspace directory, an append-only log directory, a model with a token budget, and nothing else.
- **Protocol types**: a tool runs only on a linear `Receipt`, and only the logging module can create one. "Every tool call is logged first" becomes a type rule.
- **Contracts** on what matters: the budget is never exceeded, compaction keeps the newest turns that fit.
- **Owner tests and sealed held-out tasks.**

The agent rewrites the impl units: planner, prompt construction, tool choice, retries, compaction. The loop:

1. The live generation proposes a successor into a scratch directory through a `propose` capability, its only route into the ledger. It holds no capability for the spec, the records, the policy or the `live` pointer.
2. The gate, which is toolchain code and never harness code, runs `glyph accept` on the proposal against the harness spec. It derives the proposal's authority and compares it with the parent's. Authority the parent held passes. Authority the parent lacked but the spec allows is `needs_human`, shown as a one-screen authority diff. Authority beyond the spec is rejected.
3. `glyph eval` compares the proposal with its parent on a sealed held-out split, under the frozen policy, from host-written evidence only.
4. `glyph promote` moves `live` by compare-and-swap, only with a valid record and a held-out gain above the noise margin. Running it twice changes nothing. Runs already in flight finish on the generation they started on.
5. A canary watches the next runs against the spec's thresholds for failure rate, denials and budget overruns. A breach triggers `glyph rollback`, which is the same compare-and-swap pointed backwards, with its own record.
6. The spec, toolchain, policy and held-out split change only between epochs, through governance.

Under the trust model, a proposal cannot gain authority without a human, skip logging, change its own reward, write its own evidence or promote itself. A promoted harness is not guaranteed to be better at anything the held-out split does not measure.

## Concurrency and speed

Harnesses wait on many model and tool calls at once. Glyph gives them structured concurrency without shared mutable state [P3]: a `parallel` block runs independent host calls concurrently, every child finishes or is cancelled before the block returns, and results are joined in source order. The program still has one meaning, so replay and proofs stay deterministic. The Rust host executes the calls in parallel. Guest code never shares memory between threads.

Speed comes from the native backend [v0.7] and the reuse analysis behind it, not from the toolchain being written in Rust. Rust makes the compiler, verifier and host fast and memory-safe. Generated programs are C today, and their speed is measured against that.

## Fluency: the cold start

A new language starts with near-zero model fluency, and this is the largest risk in the plan. Two things blunt it.

**Safety does not depend on fluency.** Spec identity, authority and evidence provenance are computed by the toolchain, not proved by the agent. A clumsy agent gets low reward. It does not get a weaker gate.

**Fluency is attacked directly.**

- **Familiar shapes.** Code looks like Python, types like Rust and specs like Dafny (`requires`, `ensures`, `invariant`, `decreases`, `all x in xs:`), the verification language models handle best. Value semantics remove `old()`, `reads` and `modifies`.
- **No tactics or proof terms.** Agents write invariants, asserts and lemma functions, and the solver does the rest.
- **The whole language fits on a reference card** of at most 3,000 words, generated from the grammar and the primitive table, and placed in every prompt.
- **Diagnostics are repair instructions**: stable codes, JSON, and counterexamples written as Glyph values with a replayed trace [P1].
- **A verified corpus.** `glyph port` translates a Dafny subset; ported programs count only once Glyph verifies them [P2]. Test-validated translation, as in MultiPL-T, lifted StarCoderBase-15B's OCaml pass@1 from 6.9 to 19.9.
- **The reward as the distribution channel.** *Hypothesis:* if Glyph-Bench is the cheapest verified-coding environment that is safe to train against, labs will train on it, and fluency will follow.

**Measured in public [P2].** Fluency is measured against Dafny on the same tasks and published every release. The project has chosen to keep building the language whatever the number says, so the measurement is advisory, not a kill switch. If Glyph reaches less than 0.3 of the same model's Dafny success rate, a public review weighs lowering Glyph specs to Dafny as an additional path. The work of P0 and P1 carries over to that path either way.

## Positioning

| | Glyph (target) | Dafny | Verus | Lean 4 | Bend 2 | Vercel Zero | Python + container + reviewer |
|---|---|---|---|---|---|---|---|
| Proof style | SMT, no tactics | SMT | SMT | Kernel and tactics | Hand-written terms | None | None |
| Escape hatches where agents write | None, by grammar | `assume`, `{:axiom}`, `{:verify false}` | `assume`, `admit`, `external_body` | `sorry`, axioms, `native_decide` | Holes and unsafe definitions in the raw checker | n/a | n/a |
| Spec separate from code | By file kind and hash | No | No | External comparator workflow | `LAWS.bend` import check | No | No |
| Authority | Effects [v0.7], kernel-enforced on Glyph OS [v0.13], capabilities [P3] | None | Rust's ambient std | IO, no capabilities | `Process.run` unsandboxed | Effects in signatures | Coarse, per container |
| Who decides a rewrite is better | Gate and ledger [P5] | No one | No one | No one | No one | No one | A human reading diffs |
| Measured fluency | Not yet measured | 82% union, 67.5% best model | 44%, 30.9% | 27%, 17.9% | Unknown | Unknown | Highest |

- **Dafny** has the same automation style and spec shape, so its fluency should transfer. Glyph drops what makes Dafny unsafe as a reward: escape hatches in agent files, heap frames and no authority model. *Falsified if* Glyph's fluency ratio stays below 1.0 at P5.
- **Verus** pays for Rust's memory model inside its proofs. Glyph has no aliasing. Verus keeps raw performance and its ecosystem. Glyph runs on its own small kernel, but does not target general systems code.
- **Lean** does a different job. Glyph uses it to prove its own core sound [P6]. Agents write programs, not tactics.
- **Bend 2** makes the closest pitch and is ahead on one point: a Lean-formalized kernel. Glyph should win on solver automation instead of hand-written proofs, spec ownership in the grammar instead of an import check, capabilities instead of an unsandboxed `Process.run`, and decidable tiers instead of `Type : Type`.
- **Vercel Zero** has Glyph's agent ergonomics and more momentum. Its guarantees end at effects and diagnostics. Glyph's continue to proofs, authority and reward. If Zero adds a verifier, Glyph still holds what is hardest to retrofit: a hole-free impl grammar and spec ownership the language enforces.
- **Python in a container with a reviewer** is what harnesses use today. It has the most fluency and no answer to "can generation N+1 do anything N could not, and who decided it was better?" P5 measures whether Glyph's answer is worth its cost (see the language-value test).

## Non-goals

- A theorem prover for mathematics. Use Lean.
- Competing with Rust or Verus on raw speed, manual memory or shared-memory threads.
- Escape hatches (`assume`, `unsafe`, `extern`, `verify(false)`) in agent-writable code, now or later.
- Certifying that a spec captures intent. Glyph measures weakness; humans own meaning.
- Being an RL trainer. Glyph supplies the environment, the reward and the records, not the optimizer.
- Lowering to other verifiers or adding other front ends, unless the P2 review recommends lowering to Dafny.
- Dependent types or `Type : Type`.
- Learned compiler tuning, GPU execution and a package registry before v1.0.
- Growing Glyph OS ahead of the gate. Hardening what the kernel already enforces may continue in parallel; new OS surface waits for P2 (see Glyph OS).

## North-star metrics

**North star, N1: audit-clean autonomy.** The share of harness-track tasks in Glyph-Bench that a frontier agent completes at pass@1 with a `proved` record, no sealed-audit failure and no human reading the implementation. Target: at least 40% at P4 and at least 60% at P5. Measured by `bench/run --track harness --pass 1` with the two strongest public models at measurement time, each given the reference card and up to 15 repair rounds.

| | Metric | Target | Measured by |
|---|---|---|---|
| M1 | Acceptance soundness, both halves | 0 red-team cases accepted (at least 50 cases at P0, 250 at P4, 1,000 at P5) **and** at least 90% of known-good candidates accepted | `cargo test -p glyph-accept --test redteam` |
| M2 | Fluency ratio: Glyph verified pass@1 over the same model's Dafny pass@1, parallel tasks, both run by us | ≥ 0.6 at P2, ≥ 1.0 at P5. Advisory: below 0.3 opens a public review of the Dafny-lowering path | `bench/fluency/gate.sh` |
| M3 | Proof stability | ≥ 99.5% of proved obligations survive 5 perturbation trials | `glyph verify --stability 5` |
| M4 | Annotation overhead | Median (spec + invariant + lemma lines) over impl lines ≤ 1.0 (Cedar was about 4) | `glyph stats --json` |
| M5 | Latency | p95 verify ≤ 2 s per function; accepting a 1,000-line module ≤ 30 s on 8 cores | Bench timing job |
| M6 | Reward integrity | ≤ 0.1% of rewarded episodes fail the post-hoc sealed audit; rewards bit-identical on Linux and macOS | Environment replay job |
| M7 | Weak-spec recall | ≥ 90% of at least 100 planted weak specs flagged | `glyph spec audit bench/weak` |
| M8 | Replay fidelity | 100% of `refuted` verdicts replay | `bench/verify-suite` |
| M9 | Backend parity | 0 open divergences; at least 10⁶ differential-fuzz programs per release | Fuzz job |
| M10 | Trusted base | ≤ 20,000 lines across trusted crates (their Rust, plus the kernel's C), `forbid(unsafe_code)` in each crate's Rust | `scripts/tcb.sh` from crate metadata |
| M11 | Language size | Reference card ≤ 3,000 words | `glyph lang --card \| wc -w` |
| M12 | Spec review cost | Median human minutes per spec lock, reported per release, falling release over release | `spec.lock` review timestamps |
| M13 | Proof coverage of harness code | Share of harness-track obligations that are `proved` rather than `unsupported`, reported per release | `glyph stats --track harness` |
| M14 | Gated self-improvement | Sealed-suite solve rate of the final live harness minus the seed's, same model and per-task budget, ≤ 200 proposals: ≥ +10 points in 2 of 3 runs | Ledger plus `glyph eval --sealed` |

## Roadmap

Each phase ends when `scripts/exit/pN.sh` exits 0.

**P0a, the honest baseline (v0.13.x, days).** This document replaces the old vision. The repository is split into crates along the trust boundary (`glyph-front`, `glyph-run`, `glyph-kernel`, `glyph-native` and the CLI), with trusted crates marked in their metadata and counted by `scripts/tcb.sh`. G1 to G6 land as `tests/redteam` fixtures marked `known_bypass`, so they document the bugs today and fail loudly once fixed. G7 is fixed.
*Exit:* `scripts/exit/p0a.sh` passes. The workspace builds and every existing test passes unchanged, including the QEMU kernel tests in CI. The red-team suite runs in `cargo test` with every fixture in its documented state. G7's interpreter and native outputs match. `scripts/tcb.sh` reports the trusted line count.

**P0b, honest acceptance (v0.14).** Spec and impl units, content identities, `glyph accept` with records and the three-way verdict, a virtual world for candidates in a child process, fuel in place of the wall-clock timeout and its leaked worker thread, built-ins moved to a Glyph prelude, Core as the interpreter's input, a pinned and reproducible toolchain. `glyph fix` becomes a client of `accept`.
*Exit:* every G1 to G6 fixture and every `tests/fix` cheat is `rejected`; the G6 candidate leaves no file on the host; `while true {}` ends as `unknown: fuel` with the thread count back at baseline; two runs of `accept` on the same inputs give the same record id; the honest `tests/fix` candidate is still accepted; `cargo tree -p glyph-accept` has no network crate.

**P1, proofs (v0.15).** Verification conditions for tiers D and Q, with checked 64-bit arithmetic in code and mathematical integers in specs. Invariants and `decreases`. Statuses `proved`, `refuted`, `unproved`, `exhausted`, `unsupported`. Counterexample replay. `glyph spec diff`. Native backend compiled from Core, with fuel. `Any` removed.
*Exit:* on `bench/verify-suite` (at least 150 labelled obligations) every true one is proved, every false one is refuted with a replayed trace, every unsupported one says so, and no false one is proved.

**P2, the fluency gate (v0.16).** The reference card, lemmas, `glyph port`, the trivial-candidate audit, and Glyph-Bench: an algorithm track (AlgoVeri's 77 problems with the same specs) and a harness track (at least 60 tasks). Dafny runs on the same harness.
*Exit:* the card has at most 3,000 words; M2 is measured and published for at least two frontier models; no bench spec is flagged weak. The M2 target is 0.6. Missing it does not block P3, and a result below 0.3 for both models opens the public review.

**P3, authority and concurrency (v0.17).** Capability values and `grants`, attenuation, linear `resource` types, finer effects (`fs.read`, `net`), `uses ai` with schema-checked outputs and token budgets, enforced by the host in both backends and by the Glyph OS kernel. Structured `parallel` blocks over host calls.
*Exit:* `tests/redteam/authority` has at least 60 cases (forged, widened or smuggled capabilities, use after close, leaked handles, over-budget model calls) with 0 escapes in either backend or on Glyph OS; `parallel` programs replay identically across 1,000 seeded schedules.

**P4, the reward (v0.18).** `binary-v1` and `shaped-v1` as proved Glyph functions, stability trials, sealed tests, `cost` clauses, mutation audit, recorded tapes, `glyph env serve`.
*Exit:* at least 250 red-team cases, including a Glyph version of every hack in the table above, and none is rewarded; M7 ≥ 90%; rewards across the bench are bit-identical between the Linux and macOS runners.

**P5, the self-improving harness (v0.19).** A reference harness in `harness/`, `glyph eval`, `propose`, `promote`, `approve`, `rollback`, epochs, canaries and the ledger.
*Exit:* `scripts/exit/p5.sh` runs 100 epochs with a scripted adversarial proposer (it writes the spec, widens a grant, skips the log, forges a journal, calls promote, edits the policy) alongside a model proposer, with 0 invalid promotions, a valid record behind every promotion, and M1 and M14 met. **Language-value test (advisory, like M2, pending the owner's confirmation):** a Python harness run under the same gate in a language-agnostic mode is the baseline, and the comparison on escapes, human-review share and improvement is published. If Python matches Glyph, the language is not yet paying for itself, and the next epoch's priorities say what will change that.

**P6, the small trusted base (v1.0).** Core semantics and tier-D soundness formalized in Lean, solver certificates checked by an independent checker, a second solver as a cross-check.
*Exit:* `lake build` in `formal/` passes and `#print axioms` on the soundness theorem lists only allowlisted axioms; every tier-D bench obligation carries a certificate, with holes counted; M10 holds.

## Glyph OS: a shipped track, held to the gate

Glyph OS is where effects stop being a compiler promise and become permissions the machine enforces. An app whose signature says `uses io` cannot read the clock, because the kernel grants exactly what the signature states. Oberon, Singularity and Midori showed that a language and an operating system designed together can be safer than either alone.

It already runs. Since v0.9 Glyph programs boot on bare metal (`--target kernel`), and the compiler refuses effects the target cannot provide [v0.9]. The kernel protects memory with the MMU [v0.10], is driven by interrupts [v0.11], runs several isolated apps with preemptive scheduling [v0.12], and enforces each app's declared effects as permissions [v0.13].

The kernel is the dynamic wall for code that runs on Glyph OS, so it is held to the same rules as the rest of the trusted base.

- **It is trusted code.** `glyph-kernel` holds the kernel, the app ABI and the code that turns checked signatures into its grant table. It counts toward M10, and changes to it follow the trusted-base governance below.
- **Grants come from the same source as acceptance.** The kernel's grant table is derived from the checked program, the same signatures `glyph accept` reads. From P3, capabilities reach the kernel the way effects do now.
- **Every claim carries a label and an assumption**, as in the trust table. "Enforces effects" means today what the table says, including that apps still run in kernel mode.
- **Order.** Hardening what the kernel already promises may proceed in parallel with P0b to P2. That is the next step upstream named: user mode and separate address spaces, so enforcement holds against code that skips the kernel. New OS surface, such as messages between apps and drivers, waits until P2 ships. Each OS milestone ends in an exit script, like every phase.

## Governance of the trusted base

- **Trusted crates** carry `trust = "acceptance"` or `trust = "kernel"` in their Cargo metadata, and CI runs `scripts/tcb.sh` on them. From P1, CI also derives the trusted paths from that metadata and fails if `CODEOWNERS` disagrees. Changes need two maintainer approvals, at least one not the author, a green red-team run and a passing size check. This is the one place line-by-line human review stays, including for agent-written pull requests. Until a second maintainer exists (target: P1), every record carries `governance: single-maintainer`.
- **Semantics changes** need an RFC in `rfcs/`, and soundness-relevant changes must add red-team cases. Every trusted change yields a new `ToolchainId`, which records and RL runs pin.
- **Reward policies** are append-only. CI fails if a released policy's hash changes.
- **Sealed splits** are held outside the repository, published as commit-reveal hashes, rotated every minor release and retired if they leak.
- **Specs** belong to the project. `glyph spec lock` pins `SpecId`s in `spec.lock`, and CI rejects a spec change without its lock change. Harness specs, ceilings and policies change only by opening a new epoch, which a human signs.
- **Agents** may author pull requests anywhere, labelled as agent-authored. They never approve, never hold signing keys, and bots cannot merge trusted-path changes.
- **Soundness bugs** follow `SECURITY.md`. Affected `ToolchainId`s go into `revoked.toml`, `glyph accept` stops honouring their records, and RL runs re-score.

## Principles

1. **Specs are owned, code is proposed.** AI proposes, Glyph proves, and only humans change what "correct" means.
2. **No escape hatches where agents write.**
3. **The judge is never in the dock.** Evidence comes from the host and specs from their owner, never from the candidate.
4. **Enforce, then prove.** The host makes violations impossible; proofs make behaviour certain.
5. **Every verdict says how it was reached**: proved, tested or assumed, never a bare "ok".
6. **One meaning.** One Core semantics, with every backend held to it.
7. **Explicit beats convenient.** Agents don't mind typing.
8. **Errors are interfaces**, with stable codes, exact values and a concrete next step.
9. **Small and sharp.** A feature joins only if it keeps the language on its reference card and makes programs more checkable.

If this resonates, [come build it](CONTRIBUTING.md). Start with a red-team case.
