//! `glyph fix`: an AI repair loop with guard rails. AI proposes, Glyph proves.
//!
//! A model proposes a corrected file. Glyph accepts it only if it parses, passes
//! every static check, adds no new effects, keeps every original contract and
//! test unchanged, and passes all tests. Rejected proposals go back to the model
//! with the reasons, up to `--tries` times.

use crate::ast::Program;
use crate::error::Sources;
use crate::{check, interp, loader};
use serde_json::{json, Value};
use std::collections::{BTreeSet, HashMap};
use std::sync::mpsc;
use std::time::Duration;

pub const DEFAULT_MODEL: &str = "claude-opus-5-5";
const TEST_TIMEOUT: Duration = Duration::from_secs(10);

pub struct FixOpts {
    pub tries: usize,
    pub write: bool,
    pub model: String,
    /// Verify this file as the proposed fix instead of asking a model (offline / tests).
    pub candidate: Option<String>,
}

pub fn run(file: &str, src: &str, o: &FixOpts) -> i32 {
    let problems = diagnose(file, src);
    if problems.is_empty() {
        eprintln!("{}: no problems found, nothing to fix", file);
        return 0;
    }
    eprintln!("glyph fix: {} problem(s) in {}", problems.len(), file);
    let base = Baseline::of(file, src);
    if !base.exact {
        eprintln!("  note: the file doesn't parse, so contract and test preservation can't be checked; effects still are");
    }

    if let Some(path) = &o.candidate {
        let cand = match std::fs::read_to_string(path) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("error: cannot read {}: {}", path, e);
                return 2;
            }
        };
        return match verify(&base, file, &cand) {
            Ok(()) => accept(file, src, &cand, None, o.write),
            Err(reasons) => {
                eprintln!("  rejected:");
                print_reasons(&reasons);
                1
            }
        };
    }

    let auth = match credentials() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("error: {}", e);
            return 2;
        }
    };
    let mut messages = vec![json!({
        "role": "user",
        "content": format!(
            "File `{}`:\n```glyph\n{}```\n\nProblems reported by Glyph:\n\n{}\nFix these problems.",
            file, src, problems.join("\n")
        ),
    })];
    for attempt in 1..=o.tries {
        eprintln!("  attempt {}/{}: asking {} ...", attempt, o.tries, o.model);
        let (content, text) = match call_model(&auth, &o.model, &messages) {
            Ok(r) => r,
            Err(e) => {
                eprintln!("error: {}", e);
                return 1;
            }
        };
        let proposal: Value = match serde_json::from_str(&text) {
            Ok(v) => v,
            Err(e) => {
                eprintln!("error: the model's reply was not valid JSON: {}", e);
                return 1;
            }
        };
        let source = proposal["source"].as_str().unwrap_or_default().to_string();
        let explanation = proposal["explanation"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        match verify(&base, file, &source) {
            Ok(()) => {
                eprintln!(
                    "  attempt {}: verified (types, effects, contracts, tests)",
                    attempt
                );
                return accept(file, src, &source, Some(&explanation), o.write);
            }
            Err(reasons) => {
                eprintln!("  attempt {} rejected:", attempt);
                print_reasons(&reasons);
                // Append-only history: echo the model's turn back unchanged.
                messages.push(json!({"role": "assistant", "content": content}));
                messages.push(json!({
                    "role": "user",
                    "content": format!(
                        "Glyph rejected that fix:\n- {}\n\nPropose a new complete file.",
                        reasons.join("\n- ")
                    ),
                }));
            }
        }
    }
    eprintln!(
        "glyph fix: no verified fix after {} attempt(s); the file was not changed",
        o.tries
    );
    1
}

fn print_reasons(reasons: &[String]) {
    for r in reasons {
        for (i, line) in r.lines().enumerate() {
            eprintln!("    {} {}", if i == 0 { "-" } else { " " }, line);
        }
    }
}

fn accept(file: &str, old: &str, new: &str, explanation: Option<&str>, write: bool) -> i32 {
    if let Some(e) = explanation.filter(|e| !e.is_empty()) {
        println!("{}\n", e);
    }
    print!("{}", diff(old, new));
    if write {
        if let Err(e) = std::fs::write(file, new) {
            eprintln!("error: cannot write {}: {}", file, e);
            return 1;
        }
        eprintln!("\napplied the verified fix to {}", file);
    } else {
        eprintln!("\nrun again with --write to apply this fix to {}", file);
    }
    0
}

/// Everything Glyph would complain about: syntax, static checks, failing tests.
fn diagnose(file: &str, src: &str) -> Vec<String> {
    let (prog, sources, mut errs) = match loader::load(file, src) {
        Ok(l) => (l.program, l.sources, l.errors),
        Err((errs, sources)) => return errs.iter().map(|e| e.render(&sources)).collect(),
    };
    if let Err(more) = check::check(&prog) {
        errs.extend(more);
    }
    if !errs.is_empty() {
        return errs.iter().map(|e| e.render(&sources)).collect();
    }
    run_tests(&prog, &sources)
}

/// Runs every test on a separate thread, so a fix that loops forever can't hang us.
fn run_tests(p: &Program, sources: &Sources) -> Vec<String> {
    let (tx, rx) = mpsc::channel();
    let (prog, sources) = (p.clone(), sources.clone());
    std::thread::Builder::new()
        .stack_size(1 << 29)
        .spawn(move || {
            let mut failures = vec![];
            for t in &prog.tests {
                if let Err(e) = interp::Interp::new(&prog).run_test(t) {
                    failures.push(format!(
                        "test \"{}\" failed:\n{}",
                        t.name,
                        e.render(&sources)
                    ));
                }
            }
            let _ = tx.send(failures);
        })
        .expect("failed to start test thread");
    rx.recv_timeout(TEST_TIMEOUT).unwrap_or_else(|_| {
        vec![format!(
            "the tests did not finish within {} seconds (an infinite loop?)",
            TEST_TIMEOUT.as_secs()
        )]
    })
}

fn norm(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// What the original file allowed: the limits a fix must stay within.
struct Baseline {
    /// "fn name" / "test name" -> declared effects.
    effects: HashMap<String, BTreeSet<String>>,
    /// Every effect declared anywhere: the most a brand-new function may use.
    all_effects: BTreeSet<String>,
    /// Function name -> its "requires ..." / "ensures ..." clauses.
    contracts: HashMap<String, Vec<String>>,
    /// Test name -> normalised body text.
    tests: HashMap<String, String>,
    /// False when the original didn't parse and only a textual scan was possible.
    exact: bool,
}

impl Baseline {
    fn of(file: &str, src: &str) -> Self {
        let mut b = Baseline {
            effects: HashMap::new(),
            all_effects: BTreeSet::new(),
            contracts: HashMap::new(),
            tests: HashMap::new(),
            exact: false,
        };
        let Ok(p) = loader::load(file, src).map(|l| l.program) else {
            b.all_effects = scan_uses(src);
            return b;
        };
        b.exact = true;
        for f in &p.funcs {
            let effs: BTreeSet<String> = f.effects.iter().map(|e| e.0.clone()).collect();
            b.all_effects.extend(effs.iter().cloned());
            b.effects.insert(format!("fn {}", f.name), effs);
            b.contracts.insert(f.name.clone(), contract_texts(f));
        }
        for t in &p.tests {
            let effs: BTreeSet<String> = t.effects.iter().map(|e| e.0.clone()).collect();
            b.all_effects.extend(effs.iter().cloned());
            b.effects.insert(format!("test {}", t.name), effs);
            b.tests.insert(t.name.clone(), norm(&t.text));
        }
        b
    }
}

fn contract_texts(f: &crate::ast::Func) -> Vec<String> {
    let req = f
        .requires
        .iter()
        .map(|e| format!("requires {}", norm(&e.text)));
    let ens = f
        .ensures
        .iter()
        .map(|e| format!("ensures {}", norm(&e.text)));
    req.chain(ens).collect()
}

/// Effect names after every `uses`, for files that don't parse.
fn scan_uses(src: &str) -> BTreeSet<String> {
    let words: Vec<&str> = src
        .split(|c: char| !(c.is_alphanumeric() || c == '_' || c == ','))
        .filter(|w| !w.is_empty())
        .collect();
    let mut out = BTreeSet::new();
    let mut i = 0;
    while i < words.len() {
        if words[i] == "uses" {
            i += 1;
            while i < words.len() {
                let w = words[i];
                out.extend(w.split(',').filter(|s| !s.is_empty()).map(String::from));
                if !w.ends_with(',') && !words.get(i + 1).is_some_and(|n| n.starts_with(',')) {
                    break;
                }
                i += 1;
            }
        }
        i += 1;
    }
    out
}

/// The guard rails. A fix must not gain powers or weaken its own specification.
fn guard(base: &Baseline, cand: &Program) -> Vec<String> {
    let mut v = vec![];
    let mut check_effects = |kind: &str, name: &str, effects: &[(String, crate::error::Pos)]| {
        let key = format!("{} {}", kind, name);
        let allowed = base
            .effects
            .get(&key)
            .filter(|_| base.exact)
            .unwrap_or(&base.all_effects);
        for (e, _) in effects {
            if !allowed.contains(e) {
                let what = if kind == "fn" {
                    format!("`{}`", name)
                } else {
                    format!("test \"{}\"", name)
                };
                v.push(format!(
                    "{} now declares `uses {}`, which the original did not allow; a fix may not add effects",
                    what, e
                ));
            }
        }
    };
    for f in &cand.funcs {
        check_effects("fn", &f.name, &f.effects);
    }
    for t in &cand.tests {
        check_effects("test", &t.name, &t.effects);
    }
    if !base.exact {
        return v;
    }
    for (name, clauses) in &base.contracts {
        let Some(f) = cand.funcs.iter().find(|f| &f.name == name) else {
            v.push(format!("function `{}` was removed", name));
            continue;
        };
        let now = contract_texts(f);
        for c in clauses {
            if !now.contains(c) {
                v.push(format!(
                    "the contract `{}` of `{}` was removed or changed; a fix may not weaken contracts",
                    c, name
                ));
            }
        }
    }
    for (name, body) in &base.tests {
        match cand.tests.iter().find(|t| &t.name == name) {
            None => v.push(format!(
                "test \"{}\" was removed; a fix may not delete tests",
                name
            )),
            Some(t) if norm(&t.text) != *body => v.push(format!(
                "test \"{}\" was changed; a fix may not edit existing tests",
                name
            )),
            Some(_) => {}
        }
    }
    v
}

fn verify(base: &Baseline, file: &str, cand: &str) -> Result<(), Vec<String>> {
    let (prog, sources, mut errs) = match loader::load(file, cand) {
        Ok(l) => (l.program, l.sources, l.errors),
        Err((errs, sources)) => return Err(errs.iter().map(|e| e.render(&sources)).collect()),
    };
    if let Err(more) = check::check(&prog) {
        errs.extend(more);
    }
    if !errs.is_empty() {
        return Err(errs.iter().map(|e| e.render(&sources)).collect());
    }
    let violations = guard(base, &prog);
    if !violations.is_empty() {
        return Err(violations);
    }
    let failures = run_tests(&prog, &sources);
    if failures.is_empty() {
        Ok(())
    } else {
        Err(failures)
    }
}

/// A compact line diff: changed lines with two lines of context.
fn diff(a: &str, b: &str) -> String {
    let (a, b): (Vec<&str>, Vec<&str>) = (a.lines().collect(), b.lines().collect());
    let (n, m) = (a.len(), b.len());
    let mut lcs = vec![vec![0u32; m + 1]; n + 1];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            lcs[i][j] = if a[i] == b[j] {
                lcs[i + 1][j + 1] + 1
            } else {
                lcs[i + 1][j].max(lcs[i][j + 1])
            };
        }
    }
    let mut ops = vec![];
    let (mut i, mut j) = (0, 0);
    while i < n || j < m {
        if i < n && j < m && a[i] == b[j] {
            ops.push((' ', a[i]));
            i += 1;
            j += 1;
        } else if j >= m || (i < n && lcs[i + 1][j] >= lcs[i][j + 1]) {
            ops.push(('-', a[i]));
            i += 1;
        } else {
            ops.push(('+', b[j]));
            j += 1;
        }
    }
    let near_change = |k: usize| {
        let lo = k.saturating_sub(2);
        let hi = (k + 2).min(ops.len() - 1);
        (lo..=hi).any(|x| ops[x].0 != ' ')
    };
    let mut out = String::new();
    let mut skipped = false;
    for (k, (op, line)) in ops.iter().enumerate() {
        if near_change(k) {
            if skipped && !out.is_empty() {
                out += "  ...\n";
            }
            skipped = false;
            out += &format!("{} {}\n", op, line);
        } else {
            skipped = true;
        }
    }
    out
}

// ---- Claude API (raw HTTP: there is no official Rust SDK) ----

struct Auth {
    header: &'static str,
    value: String,
    oauth: bool,
}

/// `ANTHROPIC_API_KEY`, or else the active profile of the `ant` CLI (`ant auth login`).
fn credentials() -> Result<Auth, String> {
    if let Some(key) = std::env::var("ANTHROPIC_API_KEY")
        .ok()
        .filter(|k| !k.is_empty())
    {
        return Ok(Auth {
            header: "x-api-key",
            value: key,
            oauth: false,
        });
    }
    let token = std::process::Command::new("ant")
        .args(["auth", "print-credentials", "--access-token"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|t| !t.is_empty());
    match token {
        Some(t) => Ok(Auth {
            header: "authorization",
            value: format!("Bearer {}", t),
            oauth: true,
        }),
        None => {
            Err("no Anthropic credentials: set ANTHROPIC_API_KEY, or run `ant auth login`".into())
        }
    }
}

const SYSTEM: &str = "\
You repair programs written in Glyph, a small statically typed language. You get one \
file and the problems Glyph reported, and you return the complete corrected file.

Glyph in brief:
- `fn name(a: Int, b: List[Str]) -> Int uses io, fs requires <cond> ensures <cond> { ... }`. \
Types: Int, Str, Bool, Unit, List[T], Any. A function without `->` returns Unit.
- `uses` lists the function's effects: io (print, input), fs (read_file, write_file), \
time (now, sleep), rand (random). Calling a function counts as performing its effects. \
`requires`/`ensures` are contracts; `result` is the return value inside `ensures`; contracts must be pure.
- Statements: `let x = e`, `let xs: List[Int] = []`, `x = e`, `if c { } else if c { } else { }`, \
`while c { }`, `for x in list_or_string { }`, `return e`, `assert c`. No semicolons, no `break`/`continue`.
- Operators: + - * / % (Int; + also joins Str and List), == != < <= > >=, and, or, not, xs[i].
- Pure built-ins: len, str, int (crashes on bad input), parse_int (returns Option[Int]), abs, push \
(returns a new list), range, join, split, contains.
- Records: `type Point { x: Int, y: Int }` (fields separated by commas or newlines), \
literals `Point { x: 1, y: 2 }` (every field required), access `p.x`, update `p.x = 5` (records are \
values: this updates only `p`), structural `==`.
- Enums: `enum Shape { Circle { radius: Int }, Empty }` (variants separated by commas or newlines); \
values `Circle { radius: 2 }`, `Empty`. Read them with `match s { Circle { radius } => { ... } Empty => { ... } }`; \
`match` must cover every variant or end with `_ => { ... }`; `Circle { radius: r }` binds `r`.
- Generics: `type Pair[A, B] { ... }`, `enum Tree[T] { ... }`, `fn first[T](xs: List[T]) -> Option[T]`; \
type arguments are inferred. Built in: `enum Option[T] { Some { value: T }, None }` and \
`enum Result[T, E] { Ok { value: T }, Err { error: E } }`. Inside generic code, values of type `T` \
support only `==`, `!=`, printing and storing.
- Modules: `import lib.geometry` loads lib/geometry.gly; its `pub` items are used as \
`geometry.name` (functions, types, variants, patterns). Only the file being fixed may be changed.
- Tests: `test \"name\" { assert ... }`.

Glyph verifies every fix you propose and rejects it if it adds an effect the original did \
not declare, removes or changes an existing requires/ensures clause, or removes or edits an \
existing test. Those are the human's specification. Fix the code so it meets the specification, \
changing as little as possible. If the only possible fix would break one of those rules, say \
so in the explanation and return the file unchanged.

Reply with JSON: `explanation` (one or two sentences on what you changed and why) and \
`source` (the complete corrected file).";

/// Returns the response's content blocks (to echo back) and its concatenated text.
fn call_model(auth: &Auth, model: &str, messages: &[Value]) -> Result<(Value, String), String> {
    let base =
        std::env::var("ANTHROPIC_BASE_URL").unwrap_or_else(|_| "https://api.anthropic.com".into());
    let url = format!("{}/v1/messages", base.trim_end_matches('/'));
    let mut betas = vec!["server-side-fallback-2026-07-01"];
    if auth.oauth {
        betas.push("oauth-2025-04-20");
    }
    let body = json!({
        "model": model,
        "max_tokens": 16000,
        "system": SYSTEM,
        "messages": messages,
        "thinking": {"type": "adaptive"},
        "fallbacks": "default",
        "output_config": {
            "effort": "high",
            "format": {
                "type": "json_schema",
                "schema": {
                    "type": "object",
                    "properties": {
                        "explanation": {"type": "string"},
                        "source": {"type": "string"}
                    },
                    "required": ["explanation", "source"],
                    "additionalProperties": false
                }
            }
        }
    })
    .to_string();
    let agent = ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(600))
        .build();

    let mut delay = Duration::from_secs(2);
    let raw = loop {
        let res = agent
            .post(&url)
            .set("content-type", "application/json")
            .set("anthropic-version", "2026-10-21")
            .set("anthropic-beta", &betas.join(","))
            .set(auth.header, &auth.value)
            .send_string(&body);
        match res {
            Ok(r) => {
                break r
                    .into_string()
                    .map_err(|e| format!("cannot read the API response: {}", e))?
            }
            // Rate limits, overload and server errors are worth retrying.
            Err(ureq::Error::Status(code, _))
                if (code == 429 || code >= 500) && delay.as_secs() <= 16 =>
            {
                eprintln!(
                    "    API returned {}, retrying in {}s",
                    code,
                    delay.as_secs()
                );
                std::thread::sleep(delay);
                delay *= 2;
            }
            Err(ureq::Error::Status(code, r)) => {
                return Err(format!(
                    "API error {}: {}",
                    code,
                    r.into_string().unwrap_or_default()
                ))
            }
            Err(e) => return Err(format!("cannot reach the Claude API: {}", e)),
        }
    };
    let v: Value =
        serde_json::from_str(&raw).map_err(|e| format!("invalid API response: {}", e))?;
    match v["stop_reason"].as_str() {
        Some("refusal") => return Err("the model declined this request".into()),
        Some("max_tokens") => {
            return Err("the model's reply was cut off (the file may be too large)".into())
        }
        _ => {}
    }
    let text: String = v["content"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|b| b["type"] == "text")
        .filter_map(|b| b["text"].as_str())
        .collect();
    Ok((v["content"].clone(), text))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scan_uses_reads_effect_lists() {
        let s = scan_uses("fn a() uses io, fs {\nfn b() uses time {\n print(\"uses\") }");
        assert_eq!(s.into_iter().collect::<Vec<_>>(), ["fs", "io", "time"]);
    }

    #[test]
    fn diff_marks_changed_lines() {
        let d = diff("a\nb\nc\n", "a\nB\nc\n");
        assert!(d.contains("- b") && d.contains("+ B"), "{}", d);
    }
}
