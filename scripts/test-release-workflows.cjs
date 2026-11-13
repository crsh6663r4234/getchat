// Checks the two release workflows: the stable one (release.yml) publishes
// only from main, and the beta one (release-beta.yml) publishes only from the
// beta branch, as prereleases, never under a stable v<version> tag and never
// as the latest release (docs/UPDATES.md, beta channel).
const fs = require('node:fs');
const path = require('node:path');
const assert = require('node:assert/strict');
const { spawnSync } = require('node:child_process');

const root = path.join(__dirname, '..');
const stablePath = path.join(root, '.github/workflows/release.yml');
const betaPath = path.join(root, '.github/workflows/release-beta.yml');
const stableText = fs.readFileSync(stablePath, 'utf8');
const betaText = fs.readFileSync(betaPath, 'utf8');

// YAML is parsed with Python's PyYAML when it is installed (CI runners and
// the dev container have it); the text checks below run either way.
function parseYaml(file) {
  const python = spawnSync('python3', ['-I', '-c', 'import json,sys,yaml; print(json.dumps(yaml.safe_load(open(sys.argv[1], encoding="utf-8"))))', file], { encoding: 'utf8' });
  if (python.error || python.status !== 0) {
    if (/No module named 'yaml'|ENOENT/.test(`${python.stderr} ${python.error?.code}`)) return null;
    throw new Error(`${path.basename(file)} is not valid YAML: ${python.stderr}`);
  }
  return JSON.parse(python.stdout);
}

const tests = [];
const test = (name, fn) => tests.push({ name, fn });

// PyYAML reads the bare key `on` as true.
const triggers = workflow => workflow.on ?? workflow[true];

test('both workflows parse and trigger on their own branch only', () => {
  const stable = parseYaml(stablePath);
  const beta = parseYaml(betaPath);
  if (!stable || !beta) { console.log('  (PyYAML missing: YAML parse skipped)'); return; }
  assert.deepEqual(triggers(stable).push.branches, ['main'], 'the stable release still runs for main only');
  assert.deepEqual(triggers(beta).push.branches, ['beta'], 'the beta release runs for the beta branch only');
  assert.deepEqual(Object.keys(stable.jobs), ['publish-windows']);
  assert.deepEqual(Object.keys(beta.jobs), ['publish-windows-beta']);
  const runs = workflow => Object.values(workflow.jobs).flatMap(job => job.steps.map(step => step.run || '')).join('\n');
  assert.ok(!/\bbeta\b/i.test(runs(stable)), 'the stable job has no beta logic');
  assert.match(runs(beta), /--prerelease/);
});

test('the stable job keeps its main-only guard and stable tag', () => {
  assert.match(stableText, /if \(\$env:GITHUB_REF_NAME -ne "main"\)/);
  assert.match(stableText, /\$releaseTag = "v\$\(\$config\.version\)"\n/);
  assert.match(stableText, /gh release create "\$releaseTag" .*--latest\n/);
  assert.ok(!/prerelease/i.test(stableText), 'stable releases are never prereleases');
});

test('the beta job publishes only from the beta branch under a -beta.<n> tag', () => {
  assert.match(betaText, /if \(\$env:GITHUB_REF_NAME -ne "beta"\)/);
  assert.match(betaText, /\$releaseTag = "v\$\(\$config\.version\)-beta\.\$env:GITHUB_RUN_NUMBER"/);
  const guards = betaText.match(/\$releaseTag -notmatch '\^v\\d\+\\\.\\d\+\\\.\\d\+-beta\\\.\\d\+\$'/g) || [];
  assert.equal(guards.length, 2, 'the tag is checked when computed and again before publishing');
  assert.match(betaText, /\$releaseTag -eq "v\$\(\$config\.version\)"/, 'the plain stable tag is refused');
});

test('every beta release command is a prerelease that is never latest', () => {
  const creates = betaText.split('\n').filter(line => /gh release create/.test(line));
  assert.equal(creates.length, 2, 'the build prerelease and the beta feed release');
  for (const line of creates) {
    assert.match(line, /--prerelease/, line);
    assert.match(line, /--latest=false/, line);
    assert.match(line, /--target "\$env:GITHUB_SHA"/, `${line} must tag the built commit, not main`);
  }
  assert.match(creates[0], /gh release create "\$releaseTag"/);
  assert.match(creates[1], /gh release create "beta"/);
  assert.ok(!/--latest(?!=false)/.test(betaText), 'no beta release is marked latest');
  assert.ok(!/gh release (edit|delete)/.test(betaText), 'the beta job edits or deletes no release');
});

test('the beta job uploads only to its prerelease and the beta feed tag', () => {
  const uploads = betaText.split('\n').filter(line => /gh release upload/.test(line));
  assert.equal(uploads.length, 2);
  assert.match(uploads[0], /^\s*gh release upload "\$releaseTag" /);
  assert.match(uploads[1], /^\s*gh release upload "beta" "\$latestPath#latest\.json" --clobber$/);
  assert.ok(!/releases\/download\/v\$\(\$config\.version\)\//.test(betaText), 'no URL into a stable tag');
});

test('beta and stable feeds are distinct and the beta feed names its tag', () => {
  assert.match(betaText, /release_tag = \$releaseTag/);
  assert.match(betaText, /\$latestPath = "target\/release\/latest-beta\.json"/);
  assert.match(stableText, /\$latestPath = "target\/release\/latest\.json"/);
  assert.ok(!/release_tag/.test(stableText), 'the stable feed is unchanged');
  assert.ok(!/version = .*beta/.test(betaText), 'the beta workflow does not change the version string');
});

(async () => {
  let failed = 0;
  for (const { name, fn } of tests) {
    try { await fn(); console.log(`ok - ${name}`); }
    catch (error) { failed += 1; console.error(`not ok - ${name}\n  ${error.message}`); }
  }
  if (failed) { console.error(`${failed} of ${tests.length} release workflow tests failed`); process.exit(1); }
  console.log(`${tests.length} release workflow tests passed`);
})();
