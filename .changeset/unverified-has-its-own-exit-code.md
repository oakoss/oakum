---
oakum: major
---

### Changed

An outcome oakum could not verify now exits `2`. Every failure exited `1`, which put "this repository failed the check" and "the check could not be run" behind the same number — the collapse the three-outcome rule exists to prevent, surviving in the one channel a caller reads without parsing prose. The three outcomes are now `0` ok, `2` unverified, `1` error, decided in one place and applied by every command that can report one. Every exit stays non-zero, so `set -e`, `if ! oakum check`, and a GitHub Actions step all behave as before; a script comparing the code against `1` exactly does not. See [ADR-0034](https://github.com/oakoss/oakum/blob/main/docs/decisions/0034-exit-two-for-unverified.md).

`migrate` is where this was costing the most. A cutover that wrote every file correctly and merely could not run the source tool to compare against reported `error: unverified: migrated files were kept` and exited `1` — indistinguishable from a migration that failed, on the path the install docs produce.
