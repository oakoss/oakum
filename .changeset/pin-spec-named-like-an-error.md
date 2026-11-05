---
oakum: patch
---

### Fixed

A workflow that pins oakum to a spec spelled `unversioned`, `unparseable-yaml` or `with-not-object`, such as `npm i -g @oakoss/oakum@unparseable-yaml`, was reported as the wrong problem by every command that reads the pin (`check`, `add`, `version`, `generate`, `upgrade`, `init`, `migrate`): not valid YAML, an install without a version, or a `with` that is not a mapping. It now says the spec is not an exact version. The outcome was already unverified; only the message changes.
