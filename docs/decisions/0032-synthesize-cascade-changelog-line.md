# Write a synthesized Changed line for a cascaded bump

- Status: accepted
- Date: 2026-08-25
- Deciders: Jace Babin
- Amends: [ADR-0010](0010-derive-cascade-from-declared-ranges.md)

## Context and Problem Statement

[ADR-0010](0010-derive-cascade-from-declared-ranges.md) derives the cascade trigger from the declared range and rejected a per-file `cascade:` block as the routine graph. It left the attribution problem that block solves open: a package that versions only because a dependency did gets a version heading and nothing else. How should that bump read in the changelog and in pull-request comments?

The leftover `bumpAs` question came with it: when the cascade fires, how large a bump does the dependent get?

## Decision Drivers

- Keep a Changelog wants a notable-change sentence per version; heading-only looks unfinished
- Copying the trigger's feature and fix notes attributes the wrong work ([bumpy PR #60](https://github.com/dmno-dev/bumpy/pull/60))
- [ADR-0031](0031-write-generated-markdown-genre-intersection.md) pins the builtin to Added / Changed / Fixed
- [ADR-0015](0015-layer-the-pr-status-channels.md) already puts the cascade explanation on the job summary
- The plan already stores `ChangeSource::Cascade { trigger }`; templates already receive `source` and `trigger`

## Considered Options

- **A.** Heading only / no reason line
- **B.** Synthesize a dependency line under Changed
- **C.** Copy the trigger's notes onto the dependent

## Decision Outcome

Chosen option: **B**, because a cascade-only version with no body looks unfinished, and every surveyed tool that treats attribution as a product problem names the trigger and its new version without claiming the trigger's work.

The builtin changelog writes one Changed bullet for a `ChangeSource::Cascade` package:

```text
Updated {trigger} to {version}
```

`{trigger}` is the planned trigger's package name. `{version}` is that trigger's new version. The line is attribution, not a bump-file note, so it goes under Changed even though the cascade itself is patch. It does not go under Fixed, and it does not open a `### Dependencies` section.

The line is synthesized. It is not copied from the trigger's changelog or bump files.

**Templates remain the escape hatch.** `source` and `trigger` are already in the section context. A template may emit a Dependencies section or different wording. The builtin does not.

**Comments keep the existing format.** `status` already prints `cascade from {name} ({ecosystem})` in the Source column. [ADR-0015](0015-layer-the-pr-status-channels.md) already assigned the cascade explanation to the summary (detail) and a short plan to the comment (verdict). Answering attribution does not require a new comment shape.

**Intent packages keep their own notes.** A `ChangeSource::Intent` package keeps its bump-file notes and does not get this line. Compose keeps Intent when both intent and cascade apply, so a mixed package is Intent today. Whether that package also gets the line is unconsidered until the changelog writer can read the cascade boost; it is not required to ship B.

**No new graph mechanism.** Do not add a `release` verb or a per-file `cascade:` block to get a changelog sentence. The plan already has the fields.

### `bumpAs` stays patch unless configured

Dependents stay `CascadeAs::Patch`. That is the shipped default and the surveyed default for an ordinary runtime edge.

A later `bumpAs` / peer-`match` key is **unconsidered as a default, not rejected as a key**. When it is added, it is a config that must be set; Patch remains the default. [ADR-0004](0004-derive-facts-configure-preference.md) allows that key because bump size is preference, not a restatement of the graph.

This answers the open `bumpAs` paragraph in [ADR-0010](0010-derive-cascade-from-declared-ranges.md). It does not add the key.

### Consequences

- Good, because a cascade-only changelog states why the version exists
- Good, because the shape matches those tools (name the trigger, do not copy notes). The exact sentence is oakum's; changesets and release-please use a list or a Dependencies section
- Good, because the builtin stays inside [ADR-0031](0031-write-generated-markdown-genre-intersection.md)
- Bad, because the tool authors a sentence the human did not write. That is the point of B, and a template can replace it
- Neutral, because a GitHub release body that pastes the changelog slice gets the line without a second code path
- Neutral, because a mixed intent-and-cascade package still reads as intent-only until a later call

### Confirmation

This is the acceptance test, not current output. `version` today writes a heading only for a cascade-only package. After the builtin implements B, that package gets the heading, a Changed section, and the one bullet. An intent-only package is unchanged. `--notes-file` and a configured template are unchanged.

Revisit the sentence if a real repo wants the changesets list form. Revisit `bumpAs` when a peer graph needs `match`; the key is opt-in, not a new default.

## Pros and Cons of the Options

### A. Heading only

- Good, because it invents nothing
- Bad, because the version exists and the file does not say why — the skip-`chore` failure in [release-plz #2799](https://github.com/release-plz/release-plz/issues/2799)
- Rejected: no surveyed tool chooses this shape on purpose

### B. Synthesize a dependency line

- Good, because the dependent changelog names the cause and the new version
- Good, because Keep a Changelog's own `1.1.1` puts "Upgrade dependencies" under Changed
- Bad, because the sentence is machine-authored

### C. Copy the trigger's notes

- Good, because the dependent file is never empty
- Bad, because it claims the trigger's work as this package's
- Rejected: bumpy shipped this and [undid it](https://github.com/dmno-dev/bumpy/pull/60)

## More Information

- [cascade-attribution.md](../research/cascade-attribution.md) (2026-08-25) — peer survey this record cites
- [ADR-0010](0010-derive-cascade-from-declared-ranges.md) — trigger from the declared range; attribution and `bumpAs` were the leftovers
- [ADR-0015](0015-layer-the-pr-status-channels.md) — summary carries cascade detail
- [ADR-0031](0031-write-generated-markdown-genre-intersection.md) — Changed, not a new heading
- `okm-qrx`
