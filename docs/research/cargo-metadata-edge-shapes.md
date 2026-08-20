# What `cargo metadata` reports for one intra-workspace edge

- Date: 2026-08-19
- Author: Jace Babin
- Scope: Which fields distinguish two Cargo dependency entries that point at the same package, and which manifest forms survive into the metadata oakum plans over.

## Question

The planner keys a dependency on the manifest line it came from, because `version`
rewrites that line. Which entries does `cargo metadata --no-deps` report separately,
what distinguishes them, and which manifest constructs are erased before oakum sees
them?

## Sources

Constructed scratch workspaces exercised against `cargo 1.97.1 (c980f4866 2026-06-30)`.
Every command below was run and its output recorded.

## Findings

### One package under one key appears twice when the target tables differ

A member declaring `core-pkg` in both `[target.'cfg(unix)'.dependencies]` and
`[target.'cfg(windows)'.dependencies]`, with different requirements:

```text
name=core-pkg kind=None target=cfg(unix)    req=^0.1.0 rename=None
name=core-pkg kind=None target=cfg(windows) req==0.1.0 rename=None
```

Identical in name, kind, and key; distinguished only by `target`. Both are legal and
each is a separate line to rewrite, so a uniqueness rule keyed on name and kind alone
refuses a valid manifest.

### A rename is a second entry onto the same package

`a = { path = "../a" }` beside `a_renamed = { package = "a", path = "../a", version = "0.1.0" }`
reports two entries for package `a`, one with `rename=None` and one with
`rename='a_renamed'`. The key `version` must edit is the rename where present.

That manifest does not build, though, and the metadata gives no hint of it:

```text
error: the crate `b v0.1.0` depends on crate `a v0.1.0` multiple times with different names
```

Cargo accepts the pair only while the aliased entry is `optional = true` and its feature
is off, which `cargo check` then compiles clean. The restriction is not per-section —
moving the alias to `[dev-dependencies]` fails the same way. So for Cargo this shape is
legal only with an inert alias; npm aliases (`"alias": "npm:real@^1.0.0"`) are ordinary
separate keys. The uniqueness rule has to permit it either way, and the target-table case
above forces the same conclusion independently.

### A crate may hold itself as a dev-dependency

`selfdep = { path = "." }` under `[dev-dependencies]` of `selfdep`:

```text
name=selfdep kind=dev req=*
```

`cargo check` succeeds. The one-node cycle this creates is legal in Cargo and cascades
nowhere, so a blanket refusal of every self-edge would reject a manifest Cargo accepts.
**ADR-0008 does not carve this out** — its cycle rule is unqualified by edge kind and its
worked example is itself a development cycle. `Workspace::new` refuses no self-edge and
performs no acyclicity check of any kind, so this shape is accepted today;
`okm-9ja` owns reconciling the ADR with what Cargo permits.

### Dependency-level `workspace = true` is resolved before oakum sees it

A member using `a = { workspace = true }` in `[dependencies]`, `[build-dependencies]`,
and `[target.'cfg(windows)'.dependencies]`, plus an aliased optional form:

```text
{'name':'a','req':'^1.5.0','kind':None,'target':None,'rename':None,'optional':False}
{'name':'a','req':'=1.5.0','kind':None,'target':None,'rename':'a_alias','optional':True}
{'name':'a','req':'^1.5.0','kind':'build','target':None,'rename':None,'optional':False}
{'name':'a','req':'^1.5.0','kind':None,'target':'cfg(windows)','rename':None,'optional':False}
```

No marker of inheritance survives. This is distinct from `version.workspace = true`,
which [workspace discovery](workspace-discovery.md) records for the *package* version.

### Cargo optional dependencies carry no section of their own

An `optional = true` entry reports `kind: None` — that is, `[dependencies]` — with an
`optional` flag beside it. There is no separate optional section to write back to.

## Conclusions

`(section, key, target)` identifies a manifest line; name and kind do not. Inheritance
is invisible, so the planner never sees a `workspace = true` edge as anything but a
plain range.

## Implications / actions

- Discovery must carry the rename and the target predicate onto each edge, or `version`
  rewrites the wrong entry.
- A Cargo optional dependency is the normal kind, not a kind of its own.

## Open questions

- Where the line lives for an inherited edge: the range is in the root's
  `[workspace.dependencies]`, not the member's manifest, and nothing in the metadata
  says so. `version` needs an answer before it rewrites one — see `okm-6re`.
