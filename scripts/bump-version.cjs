#!/usr/bin/env node
// Sets the GhostChat version everywhere it is written, so a release cannot
// ship with the workspace, the Tauri app and the lockfile out of step.
//
// Usage: node scripts/bump-version.cjs 5.0.3
//
// Writes: Cargo.toml ([workspace.package] version), crates/desktop/tauri.conf.json
// and the two workspace packages in Cargo.lock. Then run scripts/test-version-sync.cjs.
const fs = require('node:fs');
const path = require('node:path');

const root = path.resolve(__dirname, '..');
const version = process.argv[2];
if (!/^\d+\.\d+\.\d+$/.test(version || '')) {
  console.error('Usage: node scripts/bump-version.cjs MAJOR.MINOR.PATCH');
  process.exit(2);
}

const cargoPath = path.join(root, 'Cargo.toml');
const tauriPath = path.join(root, 'crates/desktop/tauri.conf.json');
const lockPath = path.join(root, 'Cargo.lock');

const cargo = fs.readFileSync(cargoPath, 'utf8');
const nextCargo = cargo.replace(/(\[workspace\.package\]\s*version\s*=\s*")[^"]+(")/, `$1${version}$2`);
if (nextCargo === cargo && !cargo.includes(`version = "${version}"`)) throw new Error('Cargo.toml has no [workspace.package] version.');
fs.writeFileSync(cargoPath, nextCargo);

// Replace only the top-level version line, so the rest of the file keeps its formatting.
const tauri = fs.readFileSync(tauriPath, 'utf8');
const nextTauri = tauri.replace(/^(\s{2}"version":\s*")[^"]+(")/m, `$1${version}$2`);
if (nextTauri === tauri && !tauri.includes(`"version": "${version}"`)) throw new Error('tauri.conf.json has no version line.');
fs.writeFileSync(tauriPath, nextTauri);

const names = ['ghostchat-desktop', 'ghostchat-server'];
let lock = fs.readFileSync(lockPath, 'utf8');
for (const name of names) {
  const pattern = new RegExp(`(name = "${name}"\\r?\\nversion = ")[^"]+(")`);
  if (!pattern.test(lock)) throw new Error(`Cargo.lock has no entry for ${name}.`);
  lock = lock.replace(pattern, `$1${version}$2`);
}
fs.writeFileSync(lockPath, lock);

console.log(`GhostChat version set to ${version} in Cargo.toml, tauri.conf.json and Cargo.lock.`);
