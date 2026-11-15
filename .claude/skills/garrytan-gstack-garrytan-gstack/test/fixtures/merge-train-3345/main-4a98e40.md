# Changelog

## [1.91.181.0] - 2026-10-23

**/cso can run its contained runtime checks.** Before this release the runtime catalog qualified nothing, so /cso always fell back to static assessment. It now qualifies ten pinned images: Node 24.4.0, Bun 1.3.10, Python 3.13.4 with uv 0.8.0, Rails on Ruby 3.4.4, and PostgreSQL 17.2, each for linux/amd64 and linux/arm64. With Docker installed, comprehensive /cso boots the target app inside the matching image and tests findings and repairs against it. The images passed the private evaluator's release qualification (run-5 statements, re-delivered under Garry's approval). The ingress then checked them in qualify run 39674065531 at 514fbfd.

### Added
- **`lib/cso/runtime-catalog.json`** is now catalog `cso-v3-39674065531-133cb360842d`, which replaces `cso-v3-build-reviewed-2026-09-10`. Promote run 39841502986 generated it from qualification run 39674065531's authenticated statements and attested it. GitHub Actions committed the attested bytes (`sha256:078e0bf6…`). Each runtime's image is pinned to a public `ghcr.io/garrytan/gstack/cso-staging/…@sha256` digest, and each entry records its source commit (133cb36), SBOM and provenance digests, and requalification triggers (`preparationSha256` `c721b585…`).

### For contributors
- **The /cso free tests no longer assume the committed runtime catalog is empty.** Six tests pinned `runtimes: []` and broke as soon as a promotion landed. Tests that need an unqualified catalog now build one with `unqualifiedRuntimeCatalog()`. The replay tests expect whichever fail-closed rejection the committed catalog produces, so the suite passes with the catalog both before and after a promotion.

## [1.91.180.0] - 2026-10-22

**The /cso docs say what the qualified catalogs now allow.** AGENTS.md, the README and `docs/cso-release-qualification.md` still said that no qualified runtime or scanner image existed, so /cso ran static assessment only. The scanner catalog has been qualified since #3042. The runtime catalog becomes qualified with the runtime promotion PR that ships alongside this one.

### Changed
- **AGENTS.md**, **README.md** and **`docs/cso-release-qualification.md`**: the /cso entries name the ten qualified runtime images and the twelve scanner images, and say when /cso still falls back to static assessment: without Docker, or where no profile matches.

## [1.91.179.0] - 2026-10-22

**docsync-fault-interface fits its budgets under load again.** On 2026-10-18 it timed out under load on a Ubicloud full run. Its nested 36-case suite spent most of its time starting processes: each fault actor spawn loaded about 36 ms of imports (an empty bun takes 5.5 ms), inspect repeated Git reads, and the whole nested run sat under one 120 s outer test. With real Git on 4 cores, inner cases now take a 0.70 s median (max 1.25 s) unloaded, against 1.07 s before. Under 8 busy loops the max is 3.44 s, inside bun's default 5 s; before, cases reached 4.45 s. The worst outer test went from about 100 s of its 120 s cap to 33 s. No timeout, budget, retry, assertion or case changed.

### Fixed
- **`test/helpers/docsync-fault-actor.ts`**: imports only light helper modules, hashes with `Bun.CryptoHasher` instead of loading `node:crypto`, and loads the redaction engine only on the snapshot-failure path. inspect makes 8 Git calls instead of 11, and the snapshot base check reuses the `--base` it just passed. Each actor spawn now costs 15–19 ms.
- **`test/helpers/docsync-repo.ts`, `test/helpers/docsync-interface.ts`** (new): the fixture's Git and file helpers and `docsNativeInterface` moved out of the fixture and observer modules, so the actor stops loading the eval store and the QA observer. `docsync-fixture.ts` and `docsync-observer.ts` re-export them, so no importer changed. The fixture writes its Git identity with one append to `.git/config` instead of three `git config` calls.
- **`bin/gstack-docs-candidate`**: snapshot lists `ls-files` once instead of twice and hashes with `Bun.CryptoHasher`. A snapshot spawn went from 68 ms to 46 ms, and the record it writes is byte-identical.
- **`test/docsync-fault-interface.test.ts`**: the nested callback run is split into three outer tests by test-name pattern (10, 12 and 14 cases). Each part asserts its exact pass count, its exact filtered-out count and `0 fail`, so all 36 cases still run and the patterns are disjoint.

## [1.91.178.0] - 2026-10-22

**A /qa run no longer gets a hint that another report root could change its verdict.** Train gate 39646086155 failed qa-functional-webhook-fix once, and gate 39166935531 failed qa-functional-cli-fix the same way on 2026-10-18. Both times the agent ran `gstack-qa-evidence materialize`, read a fail verdict that ended "this verdict is final for this report root", and its very next call wrote a stray placeholder file in the fixture's parent directory (`qa-reports-placeholder-unused`, `qa-Q3vLqZ-placeholder`) before writing `report.md`. Each agent then reported the stray file itself. The out-of-fixture write detector was right both times and is unchanged.

### Fixed
- **`lib/qa-evidence.ts`**: materialize's `next` instruction now ends "this verdict is final. Next, write the Markdown report beside evidence.json." It names the next write and where it goes, instead of qualifying the verdict by report root.
- **`test/helpers/qa-functional-eval.ts`**: the functional fixture's declared interface says that every Write/Edit targets an owned path inside the working directory, and that a write anywhere else, including the parent directory, fails acceptance. That was already the acceptance rule; the prompt now states it.

### Gate integrity
- No detector, threshold, trial count or case kind changed. `test/qa-evidence.test.ts` pins the new `next` wording and checks that it no longer mentions a report root.

## [1.91.177.0] - 2026-10-22

**The /cso runtime promotion workflow can finish.** Promote run 39691485727 failed after its `cso-runtime-release` approval. Its "Generate a fail-closed catalog candidate" step calls `gh api …/compare` to prove the staged commit is an ancestor of the qualification head, but the step had no `GH_TOKEN`. On a hosted runner `gh` then exits 4 ("set the GH_TOKEN environment variable"). The run stopped before it signed, pushed or opened anything.

### Fixed
- **`.github/workflows/cso-runtime-promote.yml`**: the candidate step gets `GH_TOKEN: ${{ github.token }}`, like every other step there that calls `gh`.
- **`test/workflow-gh-token.test.ts`** (new): fails when any step in any workflow invokes `gh` with no `GH_TOKEN` or `GITHUB_TOKEN` in its step, job or workflow env. actionlint does not check this. This step was the only one in the repository missing a token.
