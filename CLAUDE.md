# Working model for this repository

The owner has adopted the superpowers workflow (skills in `.claude/skills/`). Follow it for every non-trivial change:

1. **brainstorming**: understand what the owner wants, lay out approaches, get the design approved. Specs go in `docs/superpowers/specs/YYYY-MM-DD-<topic>-design.md`.
2. **writing-plans**: turn the approved design into a task plan in `docs/superpowers/plans/YYYY-MM-DD-<topic>.md`.
3. **subagent-driven-development**: execute the plan with a fresh implementer subagent per task and a reviewer after each task, then a whole-branch review. This is the owner's chosen execution method; do not ask again.
4. **test-driven-development**, **systematic-debugging** and **verification-before-completion** apply inside every task: failing test first, root cause before fix, evidence before claiming done.
5. **finishing-a-development-branch** when the work is complete.

Quick, fully specified tasks (a one-line fix, a typo) can be done directly.

The owner writes in Turkish; reply in Turkish.

## Checks that must stay green

```bash
cargo fmt --all -- --check
cargo test --locked -p ghostchat-server
for f in scripts/test-*.cjs; do node "$f"; done
for f in crates/desktop/ui/*.js; do node --check "$f"; done
git diff --check
```

The desktop crate (`ghostchat-desktop`) targets Windows; on Linux it needs the GTK/WebKit dev packages to build.
