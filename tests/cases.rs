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

/// Runs `glyph` with extra environment variables.
fn glyph_env(args: &[&str], env: &[(&str, &str)]) -> String {
    let out = Command::new(env!("CARGO_BIN_EXE_glyph"))
        .args(args)
        .envs(env.iter().copied())
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

/// Native builds instrumented with AddressSanitizer (any use-after-free or double
/// free aborts the program) and Glyph's own allocation counters.
const CHECKED: [(&str, &str); 3] = [
    (
        "GLYPH_CFLAGS",
        "-fsanitize=address -fno-omit-frame-pointer -g",
    ),
    ("GLYPH_MEMSTATS", "1"),
    ("ASAN_OPTIONS", "detect_leaks=0"),
];

fn gly_files(dir: &str) -> Vec<String> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut files: Vec<String> = std::fs::read_dir(root.join(dir))
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|x| x == "gly"))
        .map(|p| p.strip_prefix(root).unwrap().to_str().unwrap().to_string())
        .collect();
    files.sort();
    files
}

#[test]
fn native_memory_is_safe_and_leak_free() {
    let mut files = gly_files("tests/memory");
    files.push("examples/memory_bench.gly".into());
    for f in &files {
        let out = glyph_env(&["run", f, "--native"], &CHECKED);
        assert!(out.starts_with("exit: 0"), "{}:\n{}", f, out);
        assert!(!out.contains("AddressSanitizer"), "{}:\n{}", f, out);
        assert!(out.contains(", 0 live at exit"), "{} leaked:\n{}", f, out);
        // The memory checks must not change behaviour.
        let interp = glyph(&["run", f]);
        let stdout = |s: &str| s.split("--- stderr ---").next().unwrap().to_string();
        assert_eq!(stdout(&interp), stdout(&out), "{}: output differs", f);
    }
}

#[test]
fn native_error_paths_are_memory_safe() {
    for f in gly_files("tests/cases") {
        let out = glyph_env(&["run", &f, "--native"], &CHECKED);
        assert!(!out.contains("AddressSanitizer"), "{}:\n{}", f, out);
    }
}

#[test]
fn updates_with_one_owner_happen_in_place() {
    let out = glyph_env(
        &["run", "tests/memory/reuse.gly", "--native"],
        &[("GLYPH_MEMSTATS", "1")],
    );
    let allocated: i64 = out
        .split("glyph memstats: ")
        .nth(1)
        .and_then(|s| s.split(' ').next())
        .and_then(|n| n.parse().ok())
        .unwrap_or_else(|| panic!("no memstats:\n{}", out));
    // 300,000 updates; without in-place reuse this would be hundreds of thousands.
    assert!(allocated < 50, "{} allocations:\n{}", allocated, out);
}

#[test]
fn tests_inside_imported_modules_run() {
    let out = glyph(&["test", "tests/cases/modules.gly"]);
    assert!(out.starts_with("exit: 0"), "{}", out);
    assert!(out.contains("PASS  lib.geometry: distance"), "{}", out);
}

/// Boots `file` with `glyph run --target kernel` in QEMU, feeding `input` to the serial
/// port. Returns None (the test is skipped) if QEMU or a bare-metal clang is missing,
/// unless GLYPH_REQUIRE_KERNEL is set, as in CI.
fn boot(file: &str, input: &str) -> Option<String> {
    use std::io::Write;
    let have = |cmd: &str| {
        Command::new(cmd)
            .arg("--version")
            .output()
            .is_ok_and(|o| o.status.success())
    };
    let cc = std::env::var("GLYPH_KERNEL_CC").unwrap_or_else(|_| {
        [
            "/opt/homebrew/opt/llvm/bin/clang",
            "/usr/local/opt/llvm/bin/clang",
        ]
        .iter()
        .find(|p| Path::new(p).exists())
        .map_or("clang".to_string(), |p| p.to_string())
    });
    if !have("qemu-system-aarch64") || !have(&cc) {
        assert!(
            std::env::var_os("GLYPH_REQUIRE_KERNEL").is_none(),
            "kernel tests need qemu-system-aarch64, clang and lld"
        );
        eprintln!("skipping kernel test: qemu-system-aarch64 or clang not found");
        return None;
    }
    let mut child = Command::new(env!("CARGO_BIN_EXE_glyph"))
        .args(["run", file, "--target", "kernel"])
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("failed to run glyph");
    child
        .stdin
        .take()
        .unwrap()
        .write_all(input.as_bytes())
        .unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(120);
    while child.try_wait().unwrap().is_none() {
        if std::time::Instant::now() > deadline {
            child.kill().ok();
            panic!("{} did not finish booting and exit within 120 s", file);
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    let out = child.wait_with_output().unwrap();
    Some(
        format!(
            "exit: {}\n--- stdout ---\n{}--- stderr ---\n{}",
            out.status.code().unwrap_or(-1),
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        )
        .replace('\r', ""),
    )
}

#[test]
fn kernel_boots_and_runs_glyph() {
    let Some(out) = boot("examples/kernel/hello.gly", "") else {
        return;
    };
    assert!(out.starts_with("exit: 0"), "{}", out);
    assert!(out.contains("Hello from Glyph OS!"), "{}", out);
    assert!(out.contains("built a list of 100000 items"), "{}", out);
    assert!(out.contains("fib(25) = 75025"), "{}", out);
}

#[test]
fn kernel_shell_reads_the_serial_port() {
    let Some(out) = boot("examples/kernel/shell.gly", "fib 20\nsum 1 2 3\nexit\n") else {
        return;
    };
    assert!(out.starts_with("exit: 0"), "{}", out);
    assert!(out.contains("fib(20) = 6765"), "{}", out);
    assert!(out.contains("sum = 6"), "{}", out);
    assert!(out.contains("powering off"), "{}", out);
}

#[test]
fn kernel_reports_errors_with_source_lines() {
    let Some(out) = boot("tests/cases/postcondition.gly", "") else {
        return;
    };
    assert!(out.starts_with("exit: 1"), "{}", out);
    assert!(
        out.contains("postcondition of `discount` violated: `result <= price`"),
        "{}",
        out
    );
    assert!(out.contains("5 |   ensures result <= price"), "{}", out);
}

#[test]
fn kernel_target_rejects_effects_it_cannot_provide() {
    let out = glyph(&["build", "tests/kernel/uses_fs.gly", "--target", "kernel"]);
    assert!(out.starts_with("exit: 1"), "{}", out);
    assert!(
        out.contains("`save` uses effect `fs`, which the kernel target doesn't provide"),
        "{}",
        out
    );
}
