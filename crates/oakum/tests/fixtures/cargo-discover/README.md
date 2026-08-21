# Cargo discovery fixtures

Scratch Cargo packages for `discover::cargo` tests. Each is a real tree
`cargo metadata --format-version 1 --no-deps` can read.

Lone packages under this tree sit inside oakum's git workspace, so they carry an
empty `[workspace]` table (the same opt-out cargo's exit-101 message names) so
they are not absorbed as undeclared members of the oakum workspace.
