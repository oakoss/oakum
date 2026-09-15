---
oakum: patch
---

### Fixed

`oakum check` prints its refusals as blocks, the deciding line first. Each refusal's summary comes first with its detail indented beneath it, and every refusal that did not decide the exit code follows as `also …` in the same shape, so a reader meets the verdict before what is subordinate to it, and evidence sits under the line it supports; two looks that fail identically say it once, keeping what each found. Before, each look printed its detail as it ran and its summary at the end, so the management guidance could sit five lines from its summary with three unrelated lines between, and `also` lines printed before the line they were also-to. `tag-drift` prints the same block for the same state, and with a stale install pin beside real drift it now exits 1 for the drift and names the pin as `also`, as `check` does.

Among refusals of one class, the order `check` announces decides which one carries the exit code, and the install pin is its own look after the tags. With git wholly unusable, the first line a reader meets says git could not run, not `install pin is 9.9.9`. Before, the pin was verified inside the tag look and before git was touched, so a stale pin decided for a repository whose git did not work. A base ref git cannot resolve is quoted on the scope line by its first line only; the coverage look still carries the whole failure.
