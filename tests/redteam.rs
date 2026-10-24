//! Red-team cases: candidates that `glyph fix` must reject without side effects.
//! Each `tests/redteam/<name>/` holds a broken `base.gly` and a cheating `candidate.gly`.
//!
//! A case marked `KnownBypass` records what Glyph does today, which is wrong. When a
//! change makes the case behave correctly, this test fails on purpose: mark the case
//! `Holds` in the same change, so the fix and its regression test land together.

use std::path::Path;
use std::process::Command;

#[derive(Debug, PartialEq)]
enum Decision {
    Accepted,
    Rejected,
}

#[derive(Debug, PartialEq)]
enum HostEffects {
    None,
    Left,
}

#[derive(Debug, PartialEq)]
struct Outcome {
    decision: Decision,
    host_effects: HostEffects,
}

enum State {
    KnownBypass(Outcome),
    #[expect(
        dead_code,
        reason = "no case holds yet; delete this attribute with the first fix"
    )]
    Holds,
}

struct Case {
    name: &'static str,
    state: State,
}

const SAFE: Outcome = Outcome {
    decision: Decision::Rejected,
    host_effects: HostEffects::None,
};

const ACCEPTED: Outcome = Outcome {
    decision: Decision::Accepted,
    host_effects: HostEffects::None,
};

const CASES: [Case; 6] = [
    Case {
        name: "g1_spec_helper_edited",
        state: State::KnownBypass(ACCEPTED),
    },
    Case {
        name: "g2_literal_whitespace",
        state: State::KnownBypass(ACCEPTED),
    },
    Case {
        name: "g3_added_requires",
        state: State::KnownBypass(ACCEPTED),
    },
    Case {
        name: "g4_unparseable_baseline",
        state: State::KnownBypass(ACCEPTED),
    },
    Case {
        name: "g5_contract_false_off_tests",
        state: State::KnownBypass(ACCEPTED),
    },
    Case {
        name: "g6_rejected_candidate_effects",
        state: State::KnownBypass(Outcome {
            decision: Decision::Rejected,
            host_effects: HostEffects::Left,
        }),
    },
];

#[test]
fn redteam_cases_match_their_recorded_state() {
    let mut failures = vec![];
    for case in &CASES {
        let name = case.name;
        // Run each case in a fresh copy, so any file the candidate creates is a host effect.
        let source = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/redteam")
            .join(name);
        let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
            .join("redteam")
            .join(name);
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        for file in ["base.gly", "candidate.gly"] {
            std::fs::copy(source.join(file), dir.join(file)).unwrap();
        }
        let out = Command::new(env!("CARGO_BIN_EXE_glyph"))
            .args(["fix", "base.gly", "--candidate", "candidate.gly"])
            .current_dir(&dir)
            .env_remove("ANTHROPIC_API_KEY")
            .output()
            .expect("failed to run glyph");
        let stderr = String::from_utf8_lossy(&out.stderr);
        // Exit 1 is also how `glyph fix` reports usage and I/O errors, so a rejection
        // must say so; otherwise a broken fixture would pass as `Holds`.
        let decision = match out.status.code() {
            Some(0) => Decision::Accepted,
            Some(1) if stderr.contains("rejected:") => Decision::Rejected,
            other => panic!("{name}: unexpected exit {other:?}\n{stderr}"),
        };
        let host_effects = match std::fs::read_dir(&dir).unwrap().count() {
            2 => HostEffects::None,
            _ => HostEffects::Left,
        };
        let observed = Outcome {
            decision,
            host_effects,
        };
        match &case.state {
            State::KnownBypass(today) if observed == *today => {}
            State::KnownBypass(today) => failures.push(format!(
                "{name}: behaviour changed from the recorded bypass {today:?} to {observed:?}. \
                 If this is now {SAFE:?}, mark the case `Holds`."
            )),
            State::Holds if observed == SAFE => {}
            State::Holds => failures.push(format!("{name}: expected {SAFE:?}, got {observed:?}")),
        }
    }
    assert!(failures.is_empty(), "\n\n{}", failures.join("\n\n"));
}
