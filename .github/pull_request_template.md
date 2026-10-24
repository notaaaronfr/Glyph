## Summary

<!-- Briefly explain what this PR does and why. -->

Closes #

## Changes

<!-- One bullet per change. -->

## Testing

<!-- Explain how you verified the changes. If a check didn't run, such as the QEMU kernel tests, say so. -->

- [ ] `cargo fmt --all --check`
- [ ] `cargo clippy --release --workspace --all-targets --locked -- -D warnings`
- [ ] `cargo test --release --workspace --locked`
- [ ] `scripts/tcb.sh`

## Checklist

- [ ] Interpreter and native backends behave identically
- [ ] Added or updated test cases in `tests/cases/`
- [ ] Updated documentation (README / language reference) if behaviour changed
