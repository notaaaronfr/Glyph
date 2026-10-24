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
    boot_apps(&[file], input, "")
}

/// Like `boot`, with extra C flags for the kernel build (GLYPH_KERNEL_CFLAGS).
fn boot_with(file: &str, input: &str, cflags: &str) -> Option<String> {
    boot_apps(&[file], input, cflags)
}

/// Boots Glyph OS with several apps at once.
fn boot_apps(files: &[&str], input: &str, cflags: &str) -> Option<String> {
    boot_granted(files, input, cflags, "")
}

/// Like `boot_apps`, with the kernel granting apps other effects than they declare
/// (GLYPH_KERNEL_GRANT), to test that the kernel enforces them.
fn boot_granted(files: &[&str], input: &str, cflags: &str, grant: &str) -> Option<String> {
    let file = files.join(" ");
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
        .arg("run")
        .args(files)
        .args(["--target", "kernel"])
        .env("GLYPH_KERNEL_CFLAGS", cflags)
        .env("GLYPH_KERNEL_GRANT", grant)
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
    // Bad input must not crash the OS: empty lines, non-numbers, out-of-range values.
    let input = "fib 20\n\nfib abc\nfib 50\nsum 1 two\nsum 1 2 3\nsleep 50\nexit\n";
    let Some(out) = boot("examples/kernel/shell.gly", input) else {
        return;
    };
    assert!(out.starts_with("exit: 0"), "{}", out);
    assert!(out.contains("not a number: abc"), "{}", out);
    assert!(out.contains("fib needs a number from 0 to 40"), "{}", out);
    assert!(out.contains("not a number: two"), "{}", out);
    assert!(
        out.contains("(the CPU idled until a timer interrupt woke it)"),
        "{}",
        out
    );
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

/// The MMU is on: each deliberate fault (built in with -DTN_SELFTEST=n) must be
/// caught by the page tables and explained, instead of corrupting memory.
#[test]
fn kernel_memory_protection_catches_faults() {
    let cases = [
        (1, "write to read-only memory"),
        (2, "null pointer access"),
        (3, "stack overflow"),
        (4, "tried to run memory that isn't code"),
    ];
    for (n, expected) in cases {
        let flags = format!("-DTN_SELFTEST={}", n);
        let Some(out) = boot_with("examples/kernel/hello.gly", "", &flags) else {
            return;
        };
        assert!(out.starts_with("exit: 70"), "self-test {}:\n{}", n, out);
        assert!(
            out.contains(&format!("kernel panic: {}", expected)),
            "self-test {}:\n{}",
            n,
            out
        );
        assert!(
            !out.contains("Hello from Glyph OS"),
            "self-test {}:\n{}",
            n,
            out
        );
    }
}

#[test]
fn sleep_waits_in_every_backend() {
    for args in [
        vec!["run", "tests/kernel/sleep.gly"],
        vec!["run", "tests/kernel/sleep.gly", "--native"],
    ] {
        let out = glyph(&args);
        assert!(out.starts_with("exit: 0"), "{:?}:\n{}", args, out);
        assert!(
            out.contains("slept at least 300 ms: true"),
            "{:?}:\n{}",
            args,
            out
        );
        assert!(
            out.contains("and not much longer: true"),
            "{:?}:\n{}",
            args,
            out
        );
    }
}

/// On the kernel, `sleep` idles the CPU until timer interrupts wake it, so this
/// only finishes if the interrupt controller and timer work.
#[test]
fn kernel_timer_interrupts_wake_sleep() {
    let Some(out) = boot("tests/kernel/sleep.gly", "") else {
        return;
    };
    assert!(out.starts_with("exit: 0"), "{}", out);
    assert!(out.contains("tick 3"), "{}", out);
    assert!(out.contains("slept at least 300 ms: true"), "{}", out);
    assert!(out.contains("and not much longer: true"), "{}", out);
}

/// The spinner never pauses; only preemption by the timer lets the ticker run before
/// it finishes.
#[test]
fn kernel_preempts_apps_that_never_pause() {
    let apps = ["tests/kernel/spinner.gly", "tests/kernel/ticker.gly"];
    let Some(out) = boot_apps(&apps, "", "") else {
        return;
    };
    assert!(out.starts_with("exit: 0"), "{}", out);
    let at = |s: &str| {
        out.find(s)
            .unwrap_or_else(|| panic!("no {:?} in:\n{}", s, out))
    };
    assert!(
        at("ticker: 3") < at("spinner: done"),
        "no preemption:\n{}",
        out
    );
    assert!(at("spinner: started") < at("ticker: 1"), "{}", out);
    assert!(out.contains("spinner: fib(32) = 2178309"), "{}", out);
    assert!(out.contains("Glyph OS: all 2 apps finished"), "{}", out);
}

/// One app failing stops only that app.
#[test]
fn kernel_isolates_a_crashing_app() {
    let apps = [
        "examples/kernel/apps/crasher.gly",
        "examples/kernel/apps/counter.gly",
    ];
    let Some(out) = boot_apps(&apps, "", "") else {
        return;
    };
    assert!(out.starts_with("exit: 1"), "{}", out);
    assert!(
        out.contains("precondition of `withdraw` violated"),
        "{}",
        out
    );
    assert!(
        out.contains("app crasher stopped with exit code 1"),
        "{}",
        out
    );
    let at = |s: &str| {
        out.find(s)
            .unwrap_or_else(|| panic!("no {:?} in:\n{}", s, out))
    };
    assert!(at("app crasher stopped") < at("counter: done"), "{}", out);
    assert!(out.contains("crasher      exit 1"), "{}", out);
    assert!(out.contains("counter      exit 0"), "{}", out);
}

#[test]
fn kernel_grants_apps_their_declared_effects() {
    let apps = [
        "examples/kernel/apps/counter.gly",
        "tests/kernel/dice.gly",
        "tests/kernel/pure_fail.gly",
    ];
    let Some(out) = boot_apps(&apps, "", "") else {
        return;
    };
    assert!(out.starts_with("exit: 1"), "{}", out);
    assert!(
        out.contains("dice: rolled 20 times, total in range: true"),
        "{}",
        out
    );
    assert!(out.contains("counter: done"), "{}", out);
    // An app with no effects can still report its own failure.
    assert!(out.contains("precondition of `half` violated"), "{}", out);
    assert!(!out.contains("denied"), "{}", out);
    assert!(
        out.contains("counter      exit 0   cpu 0 ms   allowed io, time"),
        "{}",
        out
    );
    assert!(out.contains("allowed io, rand"), "{}", out);
    assert!(
        out.contains("pure_fail    exit 1   cpu 0 ms   allowed nothing"),
        "{}",
        out
    );
}

#[test]
fn kernel_stops_an_app_that_exceeds_its_effects() {
    // As if clock's compiled code had been changed: it reads the clock and sleeps,
    // but the kernel only grants it `io`. dice gets nothing at all.
    let apps = [
        "examples/kernel/apps/clock.gly",
        "tests/kernel/dice.gly",
        "examples/kernel/apps/counter.gly",
    ];
    let Some(out) = boot_granted(&apps, "", "", "clock=io;dice=") else {
        return;
    };
    assert!(out.starts_with("exit: 77"), "{}", out);
    assert!(
        out.contains("denied: app clock tried to read the clock, which needs `time`, but it is only allowed io"),
        "{}",
        out
    );
    assert!(
        out.contains("denied: app dice tried to get random numbers, which needs `rand`, but it is only allowed nothing"),
        "{}",
        out
    );
    assert!(!out.contains("clock: "), "{}", out);
    assert!(!out.contains("dice: rolled"), "{}", out);
    assert!(out.contains("counter: done"), "{}", out);
    assert!(out.contains("clock        exit 77"), "{}", out);
    assert!(out.contains("dice         exit 77"), "{}", out);
    assert!(out.contains("counter      exit 0"), "{}", out);
}

#[test]
fn kernel_grant_override_cannot_widen_permissions() {
    // busy declares only `io`; the override asks for more, and must not get it.
    let apps = [
        "examples/kernel/apps/busy.gly",
        "examples/kernel/apps/counter.gly",
    ];
    let Some(out) = boot_granted(&apps, "", "", "busy=io,time,rand") else {
        return;
    };
    assert!(out.starts_with("exit: 0"), "{out}");
    let busy = out
        .lines()
        .find(|l| l.starts_with("  busy "))
        .unwrap_or_else(|| panic!("no summary line for busy in:\n{out}"));
    assert!(busy.ends_with("allowed io"), "{out}");
}
