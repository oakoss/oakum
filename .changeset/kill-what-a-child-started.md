---
oakum: patch
---

A git, `cargo metadata` or pnpm child, or a source tool `migrate` runs, that goes past `OAKUM_REMOTE_DEADLINE` is killed together with what it started and is still waiting on, such as a signing program, a credential helper or an ssh `ProxyCommand`. The message names those programs: on stderr, and in `migrate`'s report on stdout. Before, only the child was killed and the rest kept running. The tree is stopped before it is killed, so nothing it starts in the meantime escapes. A process whose parent had already exited is not reached, and when oakum cannot list or kill what the child started, the message says it may still be running. A passphrase prompt that gpg's agent shows through pinentry belongs to the agent, not to git, so the agent decides when it closes.
