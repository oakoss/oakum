---
oakum: patch
---

Errors, warnings, prompts, and the summaries `oakum status` and `oakum version` print now escape bytes that could rewrite the terminal, such as an escape sequence or a bidi override in a bump file's name or a pnpm package name. So does the plan `oakum ci pr-status` prints to stdout when `GITHUB_STEP_SUMMARY` is unset; the job summary file keeps the text as rendered. Before, only some messages did: `oakum status` printed a bump file's name raw when refusing it, and the summaries printed such names raw. Error text quoted from pnpm, cargo, git or the old release tool prints without its colour codes. Combining accents print as written rather than as `\u{301}`. Output meant for scripts is unchanged: `--json`, and the paths, URLs and tags that commands print as their result.
