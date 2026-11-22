// Fails when the release version is not the same in every place it is written:
// the workspace (Cargo.toml), the Tauri app (tauri.conf.json) and the two
// workspace packages in Cargo.lock. A mismatch breaks `cargo --locked` builds
// and the release workflow's own version check.
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');

const root = path.resolve(__dirname, '..');
const cargo = fs.readFileSync(path.join(root, 'Cargo.toml'), 'utf8');
const workspace = cargo.match(/\[workspace\.package\]\s*version\s*=\s*"([^"]+)"/);
assert(workspace, 'Cargo.toml has a [workspace.package] version');
const tauri = JSON.parse(fs.readFileSync(path.join(root, 'crates/desktop/tauri.conf.json'), 'utf8'));
const lock = fs.readFileSync(path.join(root, 'Cargo.lock'), 'utf8');

const expected = workspace[1];
assert.equal(tauri.version, expected, 'tauri.conf.json version matches Cargo.toml');
for (const name of ['ghostchat-desktop', 'ghostchat-server']) {
  const entry = lock.match(new RegExp(`name = "${name}"\\r?\\nversion = "([^"]+)"`));
  assert(entry, `Cargo.lock has ${name}`);
  assert.equal(entry[1], expected, `Cargo.lock ${name} version matches Cargo.toml`);
}
console.log(`version sync tests passed (${expected})`);
