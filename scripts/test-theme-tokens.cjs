// Both themes must define the tokens the interface reads. A theme that leaves
// one out falls back to the other theme's colour or to nothing, so this fails
// before that reaches a screen.
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');

const ui = path.resolve(__dirname, '../crates/desktop/ui');
const read = name => fs.readFileSync(path.join(ui, name), 'utf8');

// Tokens defined on :root in the signage layer (the default theme).
const signage = read('wayfinding.css');
const rootBlock = signage.match(/:root\s*\{([\s\S]*?)\n\}/);
assert(rootBlock, 'wayfinding.css defines the signage tokens on :root');
const signageTokens = new Set([...rootBlock[1].matchAll(/(--[\w-]+)\s*:/g)].map(m => m[1]));

// Tokens defined by the plain theme.
const plain = read('theme-plain.css');
const plainBlock = plain.match(/body\[data-theme="plain"\]\s*\{([\s\S]*?)\n\}/);
assert(plainBlock, 'theme-plain.css defines the plain tokens');
const plainTokens = new Set([...plainBlock[1].matchAll(/(--[\w-]+)\s*:/g)].map(m => m[1]));

// The tokens every theme must provide.
const required = ['--bg', '--surface', '--line-subtle', '--text', '--muted', '--here', '--accent', '--accent-ink', '--font-ui', '--radius-control'];
for (const token of required) {
  assert(signageTokens.has(token), `signage theme defines ${token}`);
  assert(plainTokens.has(token), `plain theme defines ${token}`);
}

// The plain theme has no route colours and no purple gradients.
for (const token of ['--route-1', '--route-2', '--route-3', '--route-4', '--route-5']) {
  assert(!plainTokens.has(token), `plain theme does not redefine ${token}`);
}
assert(!/linear-gradient\(\s*135deg\s*,\s*#?a[0-9a-f]/i.test(plain), 'plain theme has no purple gradient');

console.log('theme token tests passed');
