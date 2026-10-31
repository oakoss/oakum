---
oakum: patch
---

### Fixed

`oakum ci pr-status` and `oakum ci post-comment` treat a pull request comment as theirs only when a bot wrote it and its body starts with oakum's marker. A bot comment that merely quoted the marker could be overwritten or deleted. When updating the plan, they now update or create it before removing older copies, so a removal that fails leaves the current plan up, and the message says the plan was posted and which removal failed rather than that nothing was posted.
