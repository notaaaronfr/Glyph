# Security policy

Glyph is young, experimental software: don't rely on it for security-critical systems yet.

That said, Glyph makes promises about what code can do (effects, contracts), so a way to **break those promises** is a security bug. Examples include a program performing an effect it didn't declare, or a native executable behaving differently from the interpreter in a way that bypasses a check.

## Reporting a vulnerability

Please report it privately, via GitHub's **"Report a vulnerability"** button on the repository's Security tab, rather than in a public issue. Include a minimal `.gly` program that demonstrates the problem.

We will acknowledge your report, keep you updated, and credit you in the fix unless you prefer to stay anonymous.
