---
oakum: patch
---

An empty pnpm catalog entry (`''`, `' '`, or an empty block scalar such as `|`) now reads as matching any version, as pnpm installs it. A bump of the package it names leaves the entry empty instead of failing `version` with `inherited pin ... is not a single-operator range`, and an empty block scalar no longer fails discovery as an unresolved catalog entry.
