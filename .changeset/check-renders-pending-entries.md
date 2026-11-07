---
oakum: major
---

### Changed

`check` now renders the changelog entries the pending bump files would add, as `version` renders them, and exits 1 on any entry `version` would refuse to write: a template whose entry has no heading oakum can read back, one that renders nothing (such as a cascade under a template that writes only when there are notes), and a note that leaves a code fence, HTML comment, `<details>`, `<pre>`, `<script>`, `<style>` or `<textarea>` block open. These used to pass `check --strict` and fail the version job after merge. An entry that fails only because a bump file the base does not hold yet has no commit or pull request, which the merge supplies, is rendered with stand-ins for them and passes, with a note naming it; `version` renders it if the merge's subject carries the pull request. The finding is reported under the `changelogs` look, and both commands now name the bump file whose note left a block open. `check --notes-file <path>` renders the entries with that body in place of the notes, as `version --notes-file` does. With nothing pending, `check` renders nothing.
