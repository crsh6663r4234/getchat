# Flaky test ledger

The daily flaky-fix pass reads this file. Append one line per flaky event under **Open**.
The fix pass moves an entry to **Fixed** with its PR, or to **Won't fix** with a reason.

Line format:
`- <date> | <case or test file> | <tier: paid/free/evaluator> | PR #<n> @ <head12> | failed <run id> | rerun passed <run id or "measured N/10"> | <symptom>`

Sources: the gate runs' flake-records artifacts (every case the in-run rerun turned
green) and the free suites' retried cases. Never a case that failed twice (that is a
real failure), never a harness, infra or incomplete red.

A fix changes the cause: a forced probe or a regression that is red on BASE and green
with the fix. Weakened assertions, raised thresholds, added trials or samples, changed
case kinds, skipped cases and longer timeouts are not fixes.

## Open
> Last fix pass: <thread> (<date>); see reports/<thread>.md. New entries go at the top,
> worst (most repeated) first.

## Fixed
- <date> | <case> | fixed by #<n> (<one line: the cause>)

## Won't fix
- <date> | <case> | <reason, and who decided>
