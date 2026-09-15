---
oakum: patch
---

### Changed

The workflow `init` and `migrate` print surfaces a `pr-status` run that could not post its report. That step carries `continue-on-error`, so a GitHub outage does not fail a contributor's check; the same setting also let a report that never posted pass for one that did, and left a green check over a log nobody opens. A step now follows it that, when the step failed, emits a warning into the run summary and names the check above as what still decides. It runs whether or not the check itself passed, since a failed check is exactly when the report matters. A post that fell back to the job summary, the designed path for a fork with no token that can comment, is not a failure and does not warn. oakum's own workflow gains the same step.
