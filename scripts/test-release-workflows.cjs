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

// GitHub skips a `paths:`-filtered push trigger when the matching file is not
// in the first 300 changed files: the 4.0.62 merge (9532aaf) changed 3795
// files and release.yml never started. CI's release-guard job dispatches the
// release when the version changed and no release run exists for the commit.
const ciPath = path.join(root, '.github/workflows/ci.yml');
const ciText = fs.readFileSync(ciPath, 'utf8');

test('CI has a release guard that only runs on main pushes after the tests', () => {
  const ci = parseYaml(ciPath);
  if (!ci) { console.log('  (PyYAML missing: YAML parse skipped)'); return; }
  assert.deepEqual(ci.permissions, { contents: 'read' }, 'the workflow default stays read-only');
  const guard = ci.jobs['release-guard'];
  assert.ok(guard, 'ci.yml has a release-guard job');
  assert.equal(guard.needs, 'check', 'the guard waits for the test job');
  assert.equal(guard.if, "github.event_name == 'push' && github.ref == 'refs/heads/main'");
  assert.deepEqual(guard.permissions, { actions: 'write', contents: 'read' });
  for (const [name, job] of Object.entries(ci.jobs)) {
    if (name !== 'release-guard') assert.ok(!job.permissions?.actions, `${name} gets no actions permission`);
  }
  const checkout = guard.steps.find(step => /actions\/checkout@/.test(step.uses || ''));
  assert.equal(checkout?.with?.['fetch-depth'], 2, 'the previous commit is fetched to compare versions');
  const run = guard.steps.map(step => step.run || '').join('\n');
  assert.match(run, /gh workflow run release\.yml --ref main/);
  assert.match(run, /actions\/workflows\/release\.yml\/runs\?head_sha=/);
});

test('the release guard dispatches once, only for a version change', () => {
  const ci = parseYaml(ciPath);
  if (!ci) { console.log('  (PyYAML missing: guard run skipped)'); return; }
  const guard = ci.jobs['release-guard'];
  const step = guard.steps.find(item => /gh workflow run/.test(item.run || ''));
  const os = require('node:os');
  const work = fs.mkdtempSync(path.join(os.tmpdir(), 'release-guard-'));
  const bin = path.join(work, 'bin');
  fs.mkdirSync(bin);
  const log = path.join(work, 'gh.log');
  fs.writeFileSync(path.join(bin, 'gh'), [
    '#!/bin/sh',
    `echo "$*" >> "${log}"`,
    'case "$*" in',
    '  *head_sha=*) echo "$FAKE_RUN_COUNT" ;;',
    '  *git/ref/heads/main*) echo "$FAKE_MAIN_SHA" ;;',
    'esac',
  ].join('\n'), { mode: 0o755 });
  const git = (...args) => {
    const result = spawnSync('git', args, { cwd: work, encoding: 'utf8' });
    assert.equal(result.status, 0, result.stderr);
    return result.stdout.trim();
  };
  const commit = version => {
    fs.writeFileSync(path.join(work, 'Cargo.toml'), `[workspace]\nmembers = []\n\n[workspace.package]\nversion = "${version}"\n\n[dependencies]\nversion = "9.9.9"\n`);
    git('add', 'Cargo.toml');
    git('-c', 'user.name=t', '-c', 'user.email=t@example.invalid', 'commit', '-qm', version);
    return git('rev-parse', 'HEAD');
  };
  git('init', '-q');
  commit('4.0.63');
  const bumped = commit('4.0.64');
  const runGuard = ({ runs, mainSha = bumped }) => {
    fs.rmSync(log, { force: true });
    const env = { ...process.env, PATH: `${bin}:${process.env.PATH}`, GH_TOKEN: 'x', GITHUB_REPOSITORY: 'o/r', HEAD_SHA: git('rev-parse', 'HEAD'), FAKE_RUN_COUNT: String(runs), FAKE_MAIN_SHA: mainSha };
    const result = spawnSync('bash', ['-e', '-o', 'pipefail', '-c', step.run], { cwd: work, env, encoding: 'utf8' });
    assert.equal(result.status, 0, `${result.stdout}\n${result.stderr}`);
    return fs.existsSync(log) ? fs.readFileSync(log, 'utf8').split('\n').filter(line => /^workflow run/.test(line)) : [];
  };
  assert.deepEqual(runGuard({ runs: 0 }), ['workflow run release.yml --ref main'], 'a version bump with no release run is dispatched');
  assert.deepEqual(runGuard({ runs: 1 }), [], 'an existing release run for the commit is not doubled');
  assert.deepEqual(runGuard({ runs: 0, mainSha: 'f'.repeat(40) }), [], 'main moved on: the newer commit handles its own release');
  fs.writeFileSync(path.join(work, 'notes.txt'), 'x');
  git('add', 'notes.txt');
  git('-c', 'user.name=t', '-c', 'user.email=t@example.invalid', 'commit', '-qm', 'notes');
  assert.deepEqual(runGuard({ runs: 0, mainSha: git('rev-parse', 'HEAD') }), [], 'no version change, no release');
  fs.rmSync(work, { recursive: true, force: true });
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
