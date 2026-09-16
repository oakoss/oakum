---
oakum: patch
---

### Fixed

`oakum ci pr-status` reads `.changeset/_config.toml` once per run. It read it twice, once to choose the channel and once more inside the state builder to compose the plan, so a config edited between the two reads could choose a channel with one config and build the plan with another. `status`, `ci pr-status`, and `version` share one plan pipeline; `version` carried its own copy of it.
