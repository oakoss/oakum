---
oakum: patch
---

Oakum notices sooner that a git child has finished: the wait polled every 5 ms, so each child cost up to a poll's worth of idle time after git had already exited, and a command can run many of them. It now polls every 1 ms. Measured on macOS, `oakum check` in this repository dropped from 185 ms to 153 ms (fastest of 15 alternating runs). Output is unchanged.
