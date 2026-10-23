//! Snapshot tests. Every `tests/cases/*.gly` program is run twice, by the interpreter
//! (`glyph run`) and by the native compiler (`glyph run --native`). Both runs must
//! produce identical output, and that output must match `tests/cases/<name>.expected`.
//!
//! To create or update snapshots after an intended change:
//!     GLYPH_BLESS=1 cargo test
//! Then review the diff of the .expected files before committing.

use std::path::Path;
use std::process::Command;

fn glyph(args: &[&str]) -> String {
    let out = Command::new(env!("CARGO_BIN_EXE_glyph"))
        .args(args)
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("failed to run glyph");
    format!(
        "exit: {}\n--- stdout ---\n{}--- stderr ---\n{}",
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

#[test]
fn cases_match_snapshots_in_both_modes() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let bless = std::env::var_os("GLYPH_BLESS").is_some();
    let mut cases: Vec<_> = std::fs::read_dir(root.join("tests/cases"))
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|x| x == "gly"))
        .collect();
    cases.sort();
    assert!(!cases.is_empty(), "no test cases found");

    let mut failures = vec![];
    for case in &cases {
        let rel = case.strip_prefix(root).unwrap().to_str().unwrap();
        let interp = glyph(&["run", rel]);
        let native = glyph(&["run", rel, "--native"]);
        if interp != native {
            failures.push(format!(
                "{}: interpreter and native output differ\n=== interpreter ===\n{}\n=== native ===\n{}",
                rel, interp, native
            ));
            continue;
        }
        let expected_path = case.with_extension("expected");
        if bless {
            std::fs::write(&expected_path, &interp).unwrap();
            continue;
        }
        match std::fs::read_to_string(&expected_path) {
            Ok(expected) if expected == interp => {}
            Ok(expected) => failures.push(format!(
                "{}: output changed\n=== expected ===\n{}\n=== actual ===\n{}",
                rel, expected, interp
            )),
            Err(_) => failures.push(format!(
                "{}: missing snapshot; run `GLYPH_BLESS=1 cargo test` to create it",
                rel
            )),
        }
    }
    assert!(failures.is_empty(), "\n\n{}", failures.join("\n\n"));
}

#[test]
fn example_tests_pass() {
    let out = glyph(&["test", "examples/bank.gly"]);
    assert!(out.starts_with("exit: 0"), "{}", out);
    assert!(out.contains("3 passed, 0 failed"), "{}", out);
}

#[test]
fn failing_test_is_reported_as_json() {
    let out = glyph(&["test", "examples/bug.gly", "--json"]);
    assert!(out.starts_with("exit: 1"), "{}", out);
    assert!(out.contains(r#""ok":false"#), "{}", out);
    assert!(out.contains(r#""kind":"contract""#), "{}", out);
}

#[test]
fn effects_report_lists_capabilities() {
    let out = glyph(&["effects", "examples/bank.gly"]);
    assert!(out.contains("deposit              pure"), "{}", out);
    assert!(out.contains("report               io"), "{}", out);
}

/// `glyph fix --candidate` verifies a proposed fix offline, with no model involved.
fn fix_with(candidate: &str) -> String {
    glyph(&[
        "fix",
        "tests/fix/broken.gly",
        "--candidate",
        &format!("tests/fix/{}.gly", candidate),
    ])
}

#[test]
fn fix_accepts_an_honest_fix() {
    let out = fix_with("good");
    assert!(out.starts_with("exit: 0"), "{}", out);
    assert!(
        out.contains("+   return price - price * percent / 100"),
        "{}",
        out
    );
}

#[test]
fn fix_rejects_new_effects() {
    let out = fix_with("cheat_effect");
    assert!(out.starts_with("exit: 1"), "{}", out);
    assert!(out.contains("`discount` now declares `uses fs`"), "{}", out);
}

#[test]
fn fix_rejects_weakened_contracts() {
    let out = fix_with("cheat_contract");
    assert!(out.starts_with("exit: 1"), "{}", out);
    assert!(
        out.contains("contract `ensures result <= price` of `discount` was removed"),
        "{}",
        out
    );
}

#[test]
fn fix_rejects_edited_tests() {
    let out = fix_with("cheat_test");
    assert!(out.starts_with("exit: 1"), "{}", out);
    assert!(out.contains("test \"half price\" was changed"), "{}", out);
}

#[test]
fn fix_rejects_fixes_that_still_fail() {
    let out = fix_with("still_failing");
    assert!(out.starts_with("exit: 1"), "{}", out);
    assert!(out.contains("left side = 150, right side = 100"), "{}", out);
}

#[test]
fn fix_does_nothing_for_a_healthy_file() {
    let out = glyph(&["fix", "examples/bank.gly"]);
    assert!(out.starts_with("exit: 0"), "{}", out);
    assert!(out.contains("nothing to fix"), "{}", out);
}
