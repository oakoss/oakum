# Changesets

Bump files live in this directory. Each is a markdown file whose front matter
names packages and bump levels:

```markdown
---
my-package: minor
---

What a reader should do differently after this release.
```

Or run `oakum add --packages "my-package:minor" --message "…"`.

`README.md` (any case), `AGENTS.md`, `CLAUDE.md`, and `GEMINI.md` (exact names)
are not bump files. Other `.md` files here are. knope has no skip list — do not
run knope against this directory.
