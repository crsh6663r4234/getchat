# Negative control for test/launch-env-echo-ratchet.test.ts

Every fenced command below that would print the environment must be reported;
the allowed lines must not. Prose mentions such as "never `export $(...)`" are
not commands and are not scanned.

```bash
export $(sed -n 's/^GSTACK_LAUNCH: //p' install.log)
export "$(cat launch.env)"
env
env | grep -i key
printenv
printenv ANTHROPIC_API_KEY
set
export
export -p
declare -x
typeset -x
declare -p
cd /repo && env
typeset -p
export `cat launch.env`
export $LAUNCH
export ${LAUNCH}
cd /repo && set
set 2>&1 | tee out.txt
env -0
/usr/bin/env
echo "a #b" && env
```

Allowed:

```bash
set -euo pipefail
set -u
set -x
env FOO=1 ./tool --flag
/usr/bin/env bun run build
export FOO=bar
export GSTACK_SESSION_KIND=spawned
export PATH
export PATH="$DIR:$PATH"
export "$varname"
. "$GSTACK_LAUNCH_FILE"
set | grep '^SCOPE_'
set | grep -E '^SCOPE_'
declare -x FOO=1
declare -p FOO
echo "$HOME"
# env
```

```json
{"env": "not a command"}
```
