# <CREW_NAME> notes (append-only, newest at the bottom)

The controller's working log. One line per event with a UTC timestamp. Never edited
after the fact; a correction is a new line. The digest is derived from here, never the
other way round.

Shapes:
- `<YYYY-MM-DD HH:MMZ> READY #<n> <sha12> from <thread> (<--screen|--no-screen>; flaky: <none|cases>)`
- `<ts> TRAIN <nonce> built: <order>; gate <run id>`
- `<ts> TRAIN <nonce> verdict GREEN (FLAKY <k>: <cases>) | RED (<cases>) | INCOMPLETE (<why>)`
- `<ts> SLOT <i> called: #<n> v<X> base <sha12>` · `<ts> MERGED #<n> as v<X> (<sha12>)`
- `<ts> FREEZE <why>; backstop <run id>` · `<ts> FREEZE cleared by #<n>`
- `<ts> OWNER "<exact words>" on <what was asked> (via <who>)`
- `<ts> LANE <thread> started: <scope>, owns <files>` · `<ts> LANE <thread> done: reports/<thread>.md`
- `<ts> INCIDENT <what>; <what was done>; <lesson or rule it produced>`

---
