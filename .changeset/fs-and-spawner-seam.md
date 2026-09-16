---
oakum: none
---

Test-only: the refusal helpers move to src/test_fixture.rs, the ssh transport probes take the prober as an argument so a probe that could not run is pinned as one, and write sets take scripted refusals so a rollback whose restore fails is driven on every platform. Nothing published changes.
