---
oakum: patch
---

Recovery advice after a `version` that failed and rolled back is right in more cases. The consume marker is now marked as rolled back before the rollback's first step, so a run stopped partway through it is advised to keep none of its writes instead of being read as one whose writes landed. That advice points at `git status` and `git diff` rather than the failed run's own message, which may be gone, and says to keep edits made before the run. A marker that could not be marked says why, a marker overwritten in place keeps none of its old listing, and an empty marker beside a set-aside bump file is no longer read as a consume that landed, whose advice would remove bump files never applied. The advice for set-aside bump files of unknown state says to remove one the kept writes already applied.
