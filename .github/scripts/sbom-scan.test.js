// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

'use strict';

const assert = require('node:assert/strict');
const test = require('node:test');

const scan = require('./sbom-scan.js');
const { FakeRepo, fakeContext, fakeCore } = require('./testing.js');

const finding = (extra = {}) => ({
  id: 'RUSTSEC-2026-0001',
  aliases: ['CVE-2026-1111'],
  severity: 'high',
  summary: 'Out of bounds read in the time crate',
  component: 'time 0.1.45',
  source: 'cargo-audit',
  url: 'https://rustsec.org/advisories/RUSTSEC-2026-0001',
  location: 'userspace/Cargo.lock',
  ...extra,
});

const NOW = new Date('2026-11-15T06:17:00Z');

async function runScan(repo, document, core = fakeCore()) {
  const text = typeof document === 'string' ? document : JSON.stringify(document);
  await scan.run({
    github: repo.github,
    context: fakeContext(),
    core,
    env: { FINDINGS_FILE: 'findings.json' },
    now: NOW,
    read: (path) => {
      assert.equal(path, 'findings.json');
      return text;
    },
  });
  return core;
}

const found = (...findings) => ({ findings, not_covered: [] });

test('the scan output is checked before anything is read from it', () => {
  assert.deepEqual(scan.parseScan('{"findings": [], "not_covered": []}'), { findings: [], not_covered: [] });
  assert.throws(() => scan.parseScan('not json'), /is not JSON/);
  assert.throws(() => scan.parseScan('{"findings": []}'), /no "findings" and "not_covered" lists/);
  assert.throws(() => scan.parseScan('[]'), /no "findings" and "not_covered" lists/);
  assert.throws(() => scan.parseScan('{"findings": [{"summary": "x"}], "not_covered": []}'), /no "id"/);
  assert.throws(() => scan.parseScan('{"findings": [{"id": "A", "component": "c"}], "not_covered": []}'), /no "source"/);
});

test('the most severe rating wins, and none is the least', () => {
  assert.equal(scan.moreSevere(null, 'low'), 'low');
  assert.equal(scan.moreSevere('low', null), 'low');
  assert.equal(scan.moreSevere('high', 'critical'), 'critical');
  assert.equal(scan.moreSevere('medium', 'low'), 'medium');
  assert.equal(scan.moreSevere(null, null), null);
});

test('findings of one ID are one group, with each place and every alias', () => {
  const groups = scan.groupById([
    finding(),
    finding({ component: 'time 0.1.45', location: 'kernel/Cargo.lock', severity: null, aliases: ['GHSA-aaaa'] }),
    finding({ id: 'CVE-2026-2222', component: 'curl abcdef', source: 'osv', severity: null, summary: '' }),
  ]);
  assert.deepEqual([...groups.keys()], ['RUSTSEC-2026-0001', 'CVE-2026-2222']);
  const first = groups.get('RUSTSEC-2026-0001');
  assert.equal(first.places.length, 2);
  assert.deepEqual([...first.aliases], ['CVE-2026-1111', 'GHSA-aaaa']);
  assert.equal(first.severity, 'high');
  assert.equal(first.summary, 'Out of bounds read in the time crate');
  assert.equal(groups.get('CVE-2026-2222').severity, null);
});

test('the title starts with the ID, and is one line', () => {
  const group = (summary) => ({ id: 'GHSA-xxxx-yyyy-zzzz', summary });
  assert.equal(scan.issueTitle(group('Heap overflow')), 'GHSA-xxxx-yyyy-zzzz: Heap overflow');
  assert.equal(scan.issueTitle(group('')), 'GHSA-xxxx-yyyy-zzzz');
  assert.equal(scan.issueTitle(group('Two\nlines   and\ttabs')), 'GHSA-xxxx-yyyy-zzzz: Two lines and tabs');
  const long = scan.issueTitle(group('x'.repeat(500)));
  assert.ok(long.startsWith('GHSA-xxxx-yyyy-zzzz: xxx'));
  assert.ok(long.length < 160);
  assert.ok(long.endsWith('...'));
});

test('the body names the ID, the places, the severity and what to do', () => {
  const [group] = scan.groupById([finding(), finding({ location: 'kernel/Cargo.lock' })]).values();
  const body = scan.issueBody(group);
  assert.ok(body.startsWith('<!-- sbom-scan:RUSTSEC-2026-0001 -->'));
  assert.match(body, /\(also CVE-2026-1111\)/);
  assert.match(body, /Severity, from the advisory: \*\*high\*\*/);
  assert.match(body, /\| time 0\.1\.45 \| userspace\/Cargo\.lock \| cargo-audit \| <https:\/\/rustsec\.org\/advisories\/RUSTSEC-2026-0001> \|/);
  assert.match(body, /\| time 0\.1\.45 \| kernel\/Cargo\.lock \|/);
  assert.match(body, /fix-available/);
  assert.match(body, /backport-previous/);
  assert.match(body, /not planned/);
});

test('an advisory with no severity says so and asks for triage', () => {
  const [group] = scan.groupById([finding({ severity: null })]).values();
  assert.match(scan.issueBody(group), /gives no severity, so this issue is labelled `needs-triage`/);
  assert.deepEqual(scan.labelsFor(group), ['security', 'sbom-scan', 'needs-triage']);
  const [rated] = scan.groupById([finding({ severity: 'critical' })]).values();
  assert.deepEqual(scan.labelsFor(rated), ['security', 'sbom-scan', 'severity:critical']);
});

test('text from an advisory cannot break the body or notify anyone', () => {
  const [group] = scan.groupById([finding({ summary: 'Bad | table @maintainers\nnext line', component: 'a | b' })]).values();
  const body = scan.issueBody(group);
  assert.ok(!body.includes('@maintainers'));
  assert.match(body, /> Bad \\\| table @&#8203;maintainers next line/);
  assert.match(body, /\| a \\\| b \|/);
});

test('an issue is for the ID of its marker, or of its title when the marker is gone', () => {
  assert.equal(scan.idOfIssue({ title: 'edited by hand', body: 'x\n<!-- sbom-scan:CVE-2026-5 -->\ny' }), 'CVE-2026-5');
  assert.equal(scan.idOfIssue({ title: 'CVE-2026-5: a summary', body: 'no marker' }), 'CVE-2026-5');
  assert.equal(scan.idOfIssue({ title: 'GHSA-aaaa-bbbb-cccc', body: null }), 'GHSA-aaaa-bbbb-cccc');
  assert.equal(scan.idOfIssue({ title: '', body: '' }), null);
});

test('an open issue is preferred to a closed one for the same ID', () => {
  const issues = [
    { number: 1, title: 'CVE-1: a', body: '', state: 'closed' },
    { number: 2, title: 'CVE-1: a', body: '', state: 'open' },
    { number: 3, title: 'CVE-2: b', body: '', state: 'closed' },
    { number: 4, title: 'CVE-2: b', body: '', state: 'closed' },
  ];
  const byId = scan.issuesById(issues);
  assert.equal(byId.get('CVE-1').number, 2);
  assert.equal(byId.get('CVE-2').number, 3);
});

test('a finding opens one issue labelled security, sbom-scan and its severity', async () => {
  const repo = new FakeRepo();
  const core = await runScan(repo, found(finding(), finding({ location: 'kernel/Cargo.lock' })));
  assert.equal(repo.issues.length, 1);
  const [issue] = repo.issues;
  assert.ok(issue.title.startsWith('RUSTSEC-2026-0001: '));
  assert.deepEqual(repo.labelNamesOf(issue.number), ['security', 'sbom-scan', 'severity:high']);
  assert.match(issue.body, /kernel\/Cargo\.lock/);
  assert.match(issue.body, /userspace\/Cargo\.lock/);
  for (const name of ['security', 'sbom-scan', 'severity:critical', 'severity:high', 'severity:medium', 'severity:low', 'needs-triage']) {
    assert.ok(repo.labels.includes(name), `the label ${name} is made`);
  }
  assert.match(core.summaryText, /RUSTSEC-2026-0001: #1 opened/);
});

test('an advisory with no severity gets needs-triage and no severity label', async () => {
  const repo = new FakeRepo();
  await runScan(repo, found(finding({ id: 'CVE-2026-2222', severity: null, source: 'osv', component: 'curl abc' })));
  assert.deepEqual(repo.labelNamesOf(1), ['security', 'sbom-scan', 'needs-triage']);
});

test('a second scan with the same findings changes nothing', async () => {
  const repo = new FakeRepo();
  const document = found(finding(), finding({ id: 'CVE-2026-2222', severity: null, component: 'curl abc', source: 'osv' }));
  await runScan(repo, document);
  repo.calls.length = 0;
  const core = await runScan(repo, document);
  assert.deepEqual(repo.mutations(), []);
  assert.equal(repo.issues.length, 2);
  assert.match(core.summaryText, /No issue changed/);
});

test('an issue whose ID matches again is reopened, with a comment, and never doubled', async () => {
  const repo = new FakeRepo();
  await runScan(repo, found(finding()));
  await runScan(repo, found());
  assert.equal(repo.issue(1).state, 'closed');
  await runScan(repo, found(finding()));
  assert.equal(repo.issues.length, 1);
  assert.equal(repo.issue(1).state, 'open');
  assert.match(repo.issue(1).comments.at(-1).body, /matches RUSTSEC-2026-0001 again/);
});

test('an issue closed as not planned stays closed while the finding matches', async () => {
  const repo = new FakeRepo();
  await runScan(repo, found(finding()));
  repo.issue(1).state = 'closed';
  repo.issue(1).state_reason = 'not_planned';
  repo.calls.length = 0;
  const core = await runScan(repo, found(finding()));
  assert.equal(repo.issue(1).state, 'closed');
  assert.deepEqual(repo.mutations(), []);
  assert.equal(repo.issues.length, 1);
  assert.match(core.summaryText, /left closed as not planned/);
});

test('a closed issue that a person closed as completed is reopened too', async () => {
  const repo = new FakeRepo();
  await runScan(repo, found(finding()));
  repo.issue(1).state = 'closed';
  repo.issue(1).state_reason = 'completed';
  await runScan(repo, found(finding()));
  assert.equal(repo.issue(1).state, 'open');
});

test('an open issue whose ID no longer matches is closed with a comment that says so', async () => {
  const repo = new FakeRepo();
  await runScan(repo, found(finding(), finding({ id: 'CVE-2026-2222', component: 'curl abc', source: 'osv' })));
  const core = await runScan(repo, found(finding()));
  assert.equal(repo.issue(1).state, 'open');
  assert.equal(repo.issue(2).state, 'closed');
  assert.equal(repo.issue(2).state_reason, 'completed');
  assert.match(repo.issue(2).comments[0].body, /does not match CVE-2026-2222 any more/);
  assert.match(core.summaryText, /CVE-2026-2222: #2 closed/);
  // A closed issue is not closed again, and says nothing more.
  repo.calls.length = 0;
  await runScan(repo, found(finding()));
  assert.deepEqual(repo.mutations(), []);
  assert.equal(repo.issue(2).comments.length, 1);
});

test('the issue of an ID is found by its marker when its title was edited', async () => {
  const repo = new FakeRepo();
  await runScan(repo, found(finding()));
  repo.issue(1).title = 'time crate problem, looking at it';
  await runScan(repo, found(finding()));
  assert.equal(repo.issues.length, 1);
  // The title the bot writes is its own: a changed title is brought back.
  assert.ok(repo.issue(1).title.startsWith('RUSTSEC-2026-0001: '));
});

test('a severity a person set is kept, and one the advisory gains is added', async () => {
  const repo = new FakeRepo();
  await runScan(repo, found(finding({ severity: 'high' })));
  repo.issue(1).labels = repo.issue(1).labels.filter((label) => label.name !== 'severity:high');
  repo.issue(1).labels.push({ name: 'severity:low' });
  await runScan(repo, found(finding({ severity: 'high' })));
  assert.deepEqual(repo.labelNamesOf(1).sort(), ['sbom-scan', 'security', 'severity:low']);

  const second = new FakeRepo();
  await runScan(second, found(finding({ severity: null })));
  assert.ok(second.labelNamesOf(1).includes('needs-triage'));
  await runScan(second, found(finding({ severity: 'critical' })));
  assert.deepEqual(second.labelNamesOf(1).sort(), ['sbom-scan', 'security', 'severity:critical']);
});

test('a reopened issue keeps the severity it had and is labelled when it had none', async () => {
  const repo = new FakeRepo();
  repo.addIssue({
    title: 'CVE-2026-2222: old',
    body: '<!-- sbom-scan:CVE-2026-2222 -->',
    labels: ['sbom-scan', 'severity:low'],
    state: 'closed',
    stateReason: 'completed',
  });
  await runScan(repo, found(finding({ id: 'CVE-2026-2222', severity: 'critical', source: 'osv', component: 'curl abc' })));
  assert.deepEqual(repo.labelNamesOf(1).sort(), ['sbom-scan', 'security', 'severity:low']);
  assert.equal(repo.issue(1).state, 'open');

  const bare = new FakeRepo();
  bare.addIssue({ title: 'CVE-9: old', body: '<!-- sbom-scan:CVE-9 -->', labels: ['sbom-scan'], state: 'closed', stateReason: 'completed' });
  await runScan(bare, found(finding({ id: 'CVE-9', severity: null, source: 'osv', component: 'x y' })));
  assert.deepEqual(bare.labelNamesOf(1).sort(), ['needs-triage', 'sbom-scan', 'security']);
});

test('issues that the scan did not make are not touched', async () => {
  const repo = new FakeRepo();
  repo.addIssue({ title: 'RUSTSEC-2026-0001: a person opened this', labels: ['security'] });
  repo.addIssue({ title: 'A pull request', labels: ['sbom-scan'], pullRequest: true });
  await runScan(repo, found());
  assert.deepEqual(repo.mutations().filter((name) => name !== 'issues.createLabel'), []);
});

test('the ports the scan cannot ask about are named in the summary, each with its reason', async () => {
  const repo = new FakeRepo();
  const core = await runScan(repo, {
    findings: [],
    not_covered: [
      { port: 'zip', version: '3.0', url: 'https://example.org/zip30.tar.gz', reason: 'Info-ZIP keeps no public repository, and 3.0 is its last release' },
      { port: 'mandoc', version: '1.14.6', url: 'https://example.org/mandoc.tar.gz', reason: 'its source is in CVS' },
    ],
  });
  assert.match(
    core.summaryText,
    /Not covered.*zip 3\.0 \(Info-ZIP keeps no public repository, and 3\.0 is its last release\); mandoc 1\.14\.6 \(its source is in CVS\)\./,
  );
});

test('a port with no reason is named alone, and a scan that leaves nothing out says so', async () => {
  const bare = await runScan(new FakeRepo(), {
    findings: [],
    not_covered: [{ port: 'zip', version: '3.0', url: 'https://example.org/zip30.tar.gz' }],
  });
  assert.match(bare.summaryText, /Not covered.*: zip 3\.0\./);
  const whole = await runScan(new FakeRepo(), found());
  assert.match(whole.summaryText, /Every port with a source is covered\./);
  assert.doesNotMatch(whole.summaryText, /Not covered/);
});

test('a reason from a recipe cannot break the summary or mention anybody', async () => {
  const core = await runScan(new FakeRepo(), {
    findings: [],
    not_covered: [{ port: 'x', version: '1', url: 'u', reason: 'kept by @someone |\nin two lines' }],
  });
  assert.match(core.summaryText, /x 1 \(kept by @&#8203;someone \\\| in two lines\)/);
});

test('output that does not parse changes nothing and fails the run', async () => {
  const repo = new FakeRepo();
  repo.addIssue({ title: 'CVE-1: a', body: '<!-- sbom-scan:CVE-1 -->', labels: ['sbom-scan'] });
  await assert.rejects(runScan(repo, ''), /is not JSON/);
  assert.deepEqual(repo.mutations(), []);
  assert.equal(repo.issue(1).state, 'open');
});
