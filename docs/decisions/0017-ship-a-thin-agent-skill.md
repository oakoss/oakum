# Ship an agent skill that teaches orchestration, not derivation

- Status: accepted
- Date: 2026-08-18
- Deciders: Jace Babin

## Context and Problem Statement

Change files are written at the moment a change lands, which is increasingly by an agent. bumpy ships a skill for this and it is a reasonable thing to copy. But a skill is prose the model follows, and prose that duplicates what the binary computes will disagree with the binary. What should the skill actually teach?

## Decision Drivers

- `generate` already derives packages and levels from commits, deterministically and reproducibly
- The failure this project exists to prevent is a silent miss; a skill that guesses at coverage reintroduces one
- Everything the skill teaches is text that has to stay in sync with the CLI

## Considered Options

- No skill; document the CLI and let agents read `--help`
- Copy bumpy's skill, which teaches the model to examine changes and pick packages and levels
- A thin skill that teaches orchestration only

## Decision Outcome

Chosen option: **a thin skill**. *"We should have AI skills but we can keep it thin."*

The split is that **the CLI owns everything derivable and the skill owns judgment.** Running `generate`, reading what it produced, and verifying with `check --explain` is orchestration. Deciding whether a summary is written for a human reading release notes in six months, and whether a bump level matches the user-visible impact, is judgment — and it is the only part a model is genuinely better placed to do than the binary.

The anti-pattern is bumpy's skill, which teaches the model to examine git changes and identify which packages are affected. `generate` already does that from commits, and a model re-deriving it in prose is slower, non-reproducible, and wrong more often than the tool is.

### Consequences

- Good, because the skill stays short, which is what keeps it accurate as the CLI changes
- Good, because two other orchestration flows fall naturally into it: `init`, paste what it prints, then `check` ([ADR-0007](0007-pin-the-tool-version-in-config.md)); and accepting a Renovate version bump by running `upgrade` and pushing it into the same pull request
- Bad, because a thin skill is worth less to someone whose repository is not already set up — the value is concentrated in the judgment step, which assumes the mechanics work
- Neutral, because it is not on the v0 critical path. [ADR-0012](0012-scope-v0-to-version-math-and-the-github-layer.md) scopes v0 to version math and the GitHub layer, and nothing there depends on this

### Confirmation

Revisit if `check --explain` output turns out to need interpretation the skill has to teach. That would mean the explanation is not doing its job, and the fix belongs in the output rather than in prose about the output.

## More Information

**Distribution is unsettled.** bumpy reaches agents four ways — `npx skills add`, `gh skill install`, a Claude Code plugin, and bundled inside the published npm package at a version-pinned path. The last one is the interesting one, and oakum cannot copy it *for every install path*: cargo-dist's npm installer could carry a skill — [idea 0002](../ideas/0002-agent-skill.md) records that auto-includes are verified to reach npm, with `include` inferred and one config line from confirmation — but a binary arriving by cargo-binstall or Homebrew has no `node_modules` for a skill to live in. So npm can be a channel; it cannot be the only one. That is the same asymmetry that ruled out bumpy's `$schema` mechanism in [ADR-0007](0007-pin-the-tool-version-in-config.md), and version-pinning the skill to the binary that ships it still needs its own answer.

- [idea 0002](../ideas/0002-agent-skill.md) — the exploratory note this promotes
