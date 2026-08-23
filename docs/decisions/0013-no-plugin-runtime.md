# Compile in the adapters; extend at the process boundary

- Status: accepted
- Date: 2026-08-18
- Deciders: Jace Babin

## Context and Problem Statement

Oakum needs per-ecosystem behavior — how cargo describes a workspace differs from how pnpm does — and users will eventually want to do something the tool does not do. Should that extensibility be a plugin system?

## Decision Drivers

- The north star is "something that always works, set and forget"
- Rust has no stable ABI, so a dynamically loaded plugin is a version-coupling problem forever
- [ADR-0006](0006-no-command-execution-in-templates.md) already rejected execution inside templates

## Considered Options

- A plugin runtime — dynamic libraries, or an embedded scripting language
- Compiled-in adapters plus extension at the process boundary
- No extensibility at all

## Decision Outcome

Chosen option: **compiled-in adapters, extended at the process boundary**.

Every package-manager adapter ships in the binary. Extension is a hook command with a documented JSON contract: oakum runs a program the user names, hands it JSON on stdin, and reads JSON back. The boundary is a process, which means a crash is an exit code rather than a segfault, and version skew is a schema mismatch rather than undefined behavior.

**No plugin runtime.** A plugin boundary is exactly where "always works" dies: the tool updates, the plugin does not, and the failure lands on the user at release time. Rust's lack of a stable ABI makes the dynamic-library form worse than in most ecosystems, and an embedded scripting language reintroduces the arbitrary execution that [ADR-0006](0006-no-command-execution-in-templates.md) rejected.

**A CLI, not a bundled GitHub Action.** Composability comes from every subcommand emitting JSON on stdout, which a workflow can append to `GITHUB_OUTPUT` (or any other sink). A bundled action would be a second interface to maintain and a second thing to version, and it would obscure the tool-version pin that [ADR-0007](0007-pin-the-tool-version-in-config.md) depends on.

### Consequences

- Good, because there is no ABI to keep stable and no sandbox to get wrong
- Good, because a hook can be written in any language, including a shell script
- Bad, because adding an ecosystem requires a release of oakum rather than a third-party package
- Neutral, because the JSON contract becomes a public interface the moment anything consumes it — which is one of [ADR-0002](0002-single-crate-until-io.md)'s crate-split triggers

### Confirmation

Revisit if a third party actually wants to add an ecosystem adapter. Until someone does, a plugin system is speculative generality with a permanent maintenance cost.

## Pros and Cons of the Options

### A plugin runtime

- Good, because ecosystems could be added without touching oakum
- Bad, because Rust has no stable ABI; a plugin compiled against 0.3 is undefined behavior under 0.4
- Bad, because the failure surfaces during a release, which is the worst moment available

### No extensibility

- Good, because it is the smallest thing that works
- Bad, because the repositories being targeted already need custom publish commands, and refusing that pushes users back to shell wrappers around the tool
