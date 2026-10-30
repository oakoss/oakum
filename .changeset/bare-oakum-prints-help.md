---
oakum: major
---

### Changed

Running `oakum` with no command prints its help on stderr and exits 2, the usage-error code, where it printed `oakum` and exited 0. The help lists commands in the order a new user meets them (`init`, `add`, `generate`, `status`, `check`, then `version`, `release` and `ci`) and ends with the typical flow and a link to the guides.
