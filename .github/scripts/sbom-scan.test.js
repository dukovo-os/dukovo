// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

'use strict';

const assert = require('node:assert/strict');
const test = require('node:test');

const scan = require('./sbom-scan.js');
const { ApiError, FakeRepo, fakeContext, fakeCore } = require('./testing.js');

const finding = (extra = {}) => ({
  id: 'RUSTSEC-2026-0001',
  aliases: ['CVE-2026-1111'],
  severity: 'high',
  summary: 'Out of bounds read in the time crate',
  component: 'time 0.1.45',
  product: 'pkg:cargo/time@0.1.45',
  source: 'cargo-audit',
  url: 'https://rustsec.org/advisories/RUSTSEC-2026-0001',
  location: 'userspace/Cargo.lock',
  ...extra,
});

/** An entry of the scan's `not_affected` list: a statement that leaves a finding out. */
const stated = (extra = {}) => ({
  id: 'CVE-2023-49990',
  product: 'pkg:generic/espeak-ng@1.52.0',
  component: 'espeak-ng 4870e0f',
  status: 'fixed',
  justification: null,
  statement: 'Fixed by 58f1e0b6 (#1846), which 1.52.0 holds; the advisory names 1.51 and older.',
  ...extra,
});

const NOW = new Date('2026-11-15T06:17:00Z');

const SECONDARY = 'You have exceeded a secondary rate limit and have been temporarily blocked from content creation.';

/** Runs the scan against `repo`; a pause is not waited for, but kept in `sleeps` when it is given. */
async function runScan(repo, document, core = fakeCore(), sleeps = []) {
  const text = typeof document === 'string' ? document : JSON.stringify(document);
  await scan.run({
    github: repo.github,
    context: fakeContext(),
    core,
    env: { FINDINGS_FILE: 'findings.json' },
    now: NOW,
    sleep: async (ms) => sleeps.push(ms),
    read: (path) => {
      assert.equal(path, 'findings.json');
      return text;
    },
  });
  return core;
}

const found = (...findings) => ({ findings, not_covered: [] });

/** A scan with these findings, and the statements that leave other findings out. */
const withStatements = (findings, ...notAffected) => ({ findings, not_affected: notAffected, unused_statements: [], not_covered: [] });

test('the scan output is checked before anything is read from it', () => {
  assert.deepEqual(scan.parseScan('{"findings": [], "not_covered": []}'), {
    findings: [],
    not_covered: [],
    not_affected: [],
    unused_statements: [],
  });
  assert.throws(() => scan.parseScan('not json'), /is not JSON/);
  assert.throws(() => scan.parseScan('{"findings": []}'), /no "findings" and "not_covered" lists/);
  assert.throws(() => scan.parseScan('[]'), /no "findings" and "not_covered" lists/);
  assert.throws(() => scan.parseScan('{"findings": [{"summary": "x"}], "not_covered": []}'), /no "id"/);
  assert.throws(() => scan.parseScan('{"findings": [{"id": "A", "component": "c"}], "not_covered": []}'), /no "source"/);
});

test('the lists of statements are read when the scan has them, and are empty when it has not', () => {
  const document = {
    findings: [],
    not_covered: [],
    not_affected: [stated()],
    unused_statements: [{ id: 'CVE-2026-1', product: 'pkg:generic/x@1' }],
  };
  assert.deepEqual(scan.parseScan(JSON.stringify(document)), document);
  assert.deepEqual(scan.parseScan('{"findings": [], "not_covered": [], "not_affected": []}').unused_statements, []);
  assert.throws(() => scan.parseScan('{"findings": [], "not_covered": [], "not_affected": {}}'), /"not_affected" is not a list/);
  assert.throws(() => scan.parseScan('{"findings": [], "not_covered": [], "unused_statements": "x"}'), /"unused_statements" is not a list/);
  assert.throws(() => scan.parseScan('{"findings": [], "not_covered": [], "not_affected": [{"product": "p", "status": "fixed"}]}'), /"not_affected" has no "id"/);
  assert.throws(() => scan.parseScan('{"findings": [], "not_covered": [], "not_affected": [{"id": "A", "status": "fixed"}]}'), /"not_affected" has no "product"/);
  assert.throws(() => scan.parseScan('{"findings": [], "not_covered": [], "not_affected": [{"id": "A", "product": "p"}]}'), /"not_affected" has no "status"/);
  assert.throws(() => scan.parseScan('{"findings": [], "not_covered": [], "unused_statements": [{"id": "A"}]}'), /"unused_statements" has no "product"/);
});

test('the statements of one ID are kept together', () => {
  const byId = scan.statementsById([stated(), stated({ id: 'CVE-2026-9' }), stated({ product: 'pkg:generic/other@2' })]);
  assert.deepEqual([...byId.keys()], ['CVE-2023-49990', 'CVE-2026-9']);
  assert.deepEqual(
    byId.get('CVE-2023-49990').map((entry) => entry.product),
    ['pkg:generic/espeak-ng@1.52.0', 'pkg:generic/other@2'],
  );
  assert.equal(scan.statementsById([]).size, 0);
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
  assert.match(body, /\| Component \| Product \| Where \| Found by \| Advisory \|/);
  assert.match(
    body,
    /\| time 0\.1\.45 \| pkg:cargo\/time@0\.1\.45 \| userspace\/Cargo\.lock \| cargo-audit \| <https:\/\/rustsec\.org\/advisories\/RUSTSEC-2026-0001> \|/,
  );
  assert.match(body, /\| time 0\.1\.45 \| pkg:cargo\/time@0\.1\.45 \| kernel\/Cargo\.lock \|/);
  assert.match(body, /fix-available/);
  assert.match(body, /backport-previous/);
  assert.match(body, /not planned/);
  assert.match(body, /does not affect Dukovo is stated in `security\/vex\.json`, with this ID and the product above/);
});

test('a finding of an older scan with no product has an empty cell for it', () => {
  const [group] = scan.groupById([finding({ product: undefined })]).values();
  assert.match(scan.issueBody(group), /\| time 0\.1\.45 \|  \| userspace\/Cargo\.lock \|/);
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
  assert.match(core.summaryText, /CVE-2026-2222: #2 closed, no longer matches/);
  // A closed issue is not closed again, and says nothing more.
  repo.calls.length = 0;
  await runScan(repo, found(finding()));
  assert.deepEqual(repo.mutations(), []);
  assert.equal(repo.issue(2).comments.length, 1);
});

test('the comment that closes an issue says every reason the ID can have gone', async () => {
  const repo = new FakeRepo();
  await runScan(repo, found(finding()));
  await runScan(repo, found());
  assert.equal(
    repo.issue(1).comments[0].body,
    'The scan of 2026-11-15 does not match RUSTSEC-2026-0001 any more: the part that had it has moved to a version without it, the advisory was withdrawn, or the scan no longer asks about the part (the run\'s summary lists those and why). Closing this issue. It is opened again if the scan matches RUSTSEC-2026-0001 again.',
  );
});

test('an open issue of an ID that a statement leaves out is closed as completed, with the statement quoted', async () => {
  const repo = new FakeRepo();
  await runScan(repo, found(finding({ id: 'CVE-2023-49990', severity: 'high', source: 'osv', component: 'espeak-ng 4870e0f' })));
  assert.equal(repo.issue(1).state, 'open');
  const core = await runScan(repo, withStatements([], stated()));
  assert.equal(repo.issue(1).state, 'closed');
  assert.equal(repo.issue(1).state_reason, 'completed');
  assert.equal(repo.issue(1).comments.length, 1);
  assert.equal(
    repo.issue(1).comments[0].body,
    'The scan of 2026-11-15 matches CVE-2023-49990 only where `security/vex.json` states that Dukovo is not affected: pkg:generic/espeak-ng@1.52.0, fixed. Fixed by 58f1e0b6 (#1846), which 1.52.0 holds; the advisory names 1.51 and older. Closing this issue. It is opened again if the statement is withdrawn or the scan matches CVE-2023-49990 elsewhere.',
  );
  assert.match(core.summaryText, /- CVE-2023-49990: #1 closed, stated not affected/);
  // Closed, it is left alone while the statement stands.
  repo.calls.length = 0;
  await runScan(repo, withStatements([], stated()));
  assert.deepEqual(repo.mutations(), []);
  assert.equal(repo.issue(1).comments.length, 1);
});

test('an issue closed by a statement is opened again when the statement is withdrawn', async () => {
  const repo = new FakeRepo();
  const espeak = finding({ id: 'CVE-2023-49990', source: 'osv', component: 'espeak-ng 4870e0f', product: 'pkg:generic/espeak-ng@1.52.0' });
  await runScan(repo, found(espeak));
  await runScan(repo, withStatements([], stated()));
  await runScan(repo, found(espeak));
  assert.equal(repo.issues.length, 1);
  assert.equal(repo.issue(1).state, 'open');
  assert.match(repo.issue(1).comments.at(-1).body, /matches CVE-2023-49990 again/);
});

test('a statement says why a finding that is not affected is, and an ID with a finding elsewhere stays open', async () => {
  const repo = new FakeRepo();
  const other = finding({ id: 'CVE-2023-49990', source: 'osv', component: 'espeak-ng 4870e0f', product: 'pkg:generic/espeak-ng@1.52.0' });
  await runScan(repo, found(other, finding({ id: 'CVE-2026-2222', source: 'osv', component: 'curl abc' })));
  repo.calls.length = 0;
  // CVE-2023-49990 still matches in another product, so it is not closed.
  const core = await runScan(
    repo,
    withStatements([{ ...other, product: 'pkg:generic/espeak-ng-data@1.52.0' }], stated(), stated({ id: 'CVE-2026-2222', product: 'pkg:generic/curl@8' })),
  );
  assert.equal(repo.issue(1).state, 'open');
  assert.equal(repo.issue(2).state, 'closed');
  assert.match(repo.issue(2).comments[0].body, /^The scan of 2026-11-15 matches CVE-2026-2222 only where/);
  assert.equal(repo.issue(1).comments.length, 0);
  assert.doesNotMatch(core.summaryText, /CVE-2023-49990: #1 closed/);
});

test('the words of a statement name its status and its justification', async () => {
  const closedWith = async (entry) => {
    const repo = new FakeRepo();
    await runScan(repo, found(finding({ id: entry.id })));
    await runScan(repo, withStatements([], entry));
    return repo.issue(1).comments[0].body;
  };
  const cases = [
    ['component_not_present', 'the component is not present'],
    ['vulnerable_code_not_present', 'the vulnerable code is not present'],
    ['vulnerable_code_not_in_execute_path', 'the vulnerable code is not in the execute path'],
    ['vulnerable_code_cannot_be_controlled_by_adversary', 'the vulnerable code cannot be controlled by an adversary'],
    ['inline_mitigations_already_exist', 'mitigations already exist'],
  ];
  for (const [justification, words] of cases) {
    const body = await closedWith(stated({ id: 'CVE-1', status: 'not_affected', justification, statement: 'Only the data is used.' }));
    assert.ok(body.includes(`: pkg:generic/espeak-ng@1.52.0, not affected, ${words}. Only the data is used. Closing this issue.`), body);
  }
  const plain = await closedWith(stated({ id: 'CVE-1', statement: 'Fixed in 1.52.0' }));
  assert.ok(plain.includes(': pkg:generic/espeak-ng@1.52.0, fixed. Fixed in 1.52.0. Closing this issue.'), plain);
  const bare = await closedWith(stated({ id: 'CVE-1', statement: null }));
  assert.ok(bare.includes(': pkg:generic/espeak-ng@1.52.0, fixed. Closing this issue.'), bare);
});

test('an ID that several statements leave out has each product named', async () => {
  const repo = new FakeRepo();
  await runScan(repo, found(finding({ id: 'CVE-1' })));
  await runScan(
    repo,
    withStatements(
      [],
      stated({ id: 'CVE-1', product: 'pkg:generic/a@1', status: 'not_affected', justification: 'component_not_present', statement: 'A is not built.' }),
      stated({ id: 'CVE-1', product: 'pkg:generic/b@2', statement: 'B holds the fix' }),
    ),
  );
  assert.equal(repo.issue(1).state, 'closed');
  assert.equal(
    repo.issue(1).comments[0].body,
    [
      'The scan of 2026-11-15 matches CVE-1 only where `security/vex.json` states that Dukovo is not affected:',
      '',
      '- pkg:generic/a@1, not affected, the component is not present. A is not built.',
      '- pkg:generic/b@2, fixed. B holds the fix.',
      '',
      'Closing this issue. It is opened again if a statement is withdrawn or the scan matches CVE-1 elsewhere.',
    ].join('\n'),
  );
});

test('text of a statement cannot break the comment or mention anybody', async () => {
  const repo = new FakeRepo();
  await runScan(repo, found(finding({ id: 'CVE-1' })));
  await runScan(repo, withStatements([], stated({ id: 'CVE-1', product: 'pkg:generic/a|b@1', statement: 'Thanks @someone\nfor the fix' })));
  const body = repo.issue(1).comments[0].body;
  assert.ok(!body.includes('@someone'));
  assert.match(body, /pkg:generic\/a\\\|b@1, fixed\. Thanks @&#8203;someone for the fix\. Closing/);
});

test('an issue is left open when the scan lists a statement for an ID that has no issue', async () => {
  const repo = new FakeRepo();
  const core = await runScan(repo, withStatements([], stated()));
  assert.equal(repo.issues.length, 0);
  assert.deepEqual(repo.mutations().filter((name) => name !== 'issues.createLabel'), []);
  assert.match(core.summaryText, /No issue changed/);
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

test('the ports the scan does not ask about are named in the summary, each with its reason', async () => {
  const repo = new FakeRepo();
  const core = await runScan(repo, {
    findings: [],
    not_covered: [
      { port: 'locales', version: '2.42', url: 'https://example.org/glibc.tar.xz', reason: 'only data is taken: the locale tables of glibc, no glibc code' },
      { port: 'zip', version: '3.0', url: 'https://example.org/zip30.tar.gz', reason: 'no git repository to ask by: Info-ZIP keeps none, and 3.0 is its last release' },
      { port: 'mandoc', version: '1.14.6', url: 'https://example.org/mandoc.tar.gz', reason: 'the recipe has no [upstream]' },
    ],
  });
  assert.match(
    core.summaryText,
    /Not asked about: locales 2\.42 \(only data is taken: the locale tables of glibc, no glibc code\); zip 3\.0 \(no git repository to ask by: Info-ZIP keeps none, and 3\.0 is its last release\); mandoc 1\.14\.6 \(the recipe has no \[upstream\]\)\./,
  );
  assert.doesNotMatch(core.summaryText, /Not covered/);
  assert.doesNotMatch(core.summaryText, /public git repository/);
});

test('a port with no reason is named alone, and a scan that leaves nothing out says so', async () => {
  const bare = await runScan(new FakeRepo(), {
    findings: [],
    not_covered: [{ port: 'zip', version: '3.0', url: 'https://example.org/zip30.tar.gz' }],
  });
  assert.match(bare.summaryText, /Not asked about: zip 3\.0\./);
  const whole = await runScan(new FakeRepo(), found());
  assert.match(whole.summaryText, /Every port with a source is covered\./);
  assert.doesNotMatch(whole.summaryText, /Not asked about/);
  assert.doesNotMatch(whole.summaryText, /security\/vex\.json/);
});

test('a reason from a recipe cannot break the summary or mention anybody', async () => {
  const core = await runScan(new FakeRepo(), {
    findings: [],
    not_covered: [{ port: 'x', version: '1', url: 'u', reason: 'kept by @someone |\nin two lines' }],
  });
  assert.match(core.summaryText, /x 1 \(kept by @&#8203;someone \\\| in two lines\)/);
});

test('the summary counts the findings that are stated not affected, each with its product and status', async () => {
  const core = await runScan(
    new FakeRepo(),
    withStatements(
      [finding()],
      stated(),
      stated({ id: 'CVE-2024-1', product: 'pkg:generic/glibc@2.42', status: 'not_affected', justification: 'component_not_present' }),
    ),
  );
  assert.match(core.summaryText, /1 findings of 1 advisories\./);
  assert.match(
    core.summaryText,
    /Stated not affected in `security\/vex\.json`, so left out of the findings \(2\):\n- CVE-2023-49990: pkg:generic\/espeak-ng@1\.52\.0, fixed\n- CVE-2024-1: pkg:generic\/glibc@2\.42, not affected\n/,
  );
});

test('the summary names the statements that match nothing, so that a person removes or fixes them', async () => {
  const core = await runScan(new FakeRepo(), {
    findings: [],
    not_affected: [],
    unused_statements: [
      { id: 'CVE-2026-1', product: 'pkg:generic/x@1' },
      { id: 'GHSA-aaaa-bbbb-cccc', product: 'pkg:cargo/y@2.0.0' },
    ],
    not_covered: [],
  });
  assert.match(
    core.summaryText,
    /Statements in `security\/vex\.json` that match no finding, to remove or fix: CVE-2026-1 \(pkg:generic\/x@1\); GHSA-aaaa-bbbb-cccc \(pkg:cargo\/y@2\.0\.0\)\./,
  );
  assert.doesNotMatch(core.summaryText, /Stated not affected/);
});

test('every write of the run is paced, a second apart', async () => {
  const repo = new FakeRepo();
  const sleeps = [];
  await runScan(repo, found(finding(), finding({ id: 'CVE-2026-2222', severity: null, source: 'osv', component: 'curl abc' })), fakeCore(), sleeps);
  assert.equal(repo.issues.length, 2);
  const writes = repo.mutations().length;
  assert.ok(writes > 2);
  assert.deepEqual(sleeps, new Array(writes - 1).fill(1000));
});

test('the updates, the comments and the labels of a run are paced too, and its reads are not', async () => {
  const repo = new FakeRepo();
  const [a, b, c, d] = ['CVE-1', 'CVE-2', 'CVE-3', 'CVE-4'].map((id) => finding({ id, severity: 'high', source: 'osv', component: `x ${id}` }));
  await runScan(repo, found(a, { ...b, severity: null }, c, d));
  await runScan(repo, found(a, { ...b, severity: null }, c));
  repo.issue(1).title = 'edited by hand';
  repo.calls.length = 0;
  const sleeps = [];
  await runScan(repo, found(a, b, d), fakeCore(), sleeps);
  const names = new Set(repo.mutations());
  for (const name of ['issues.update', 'issues.createComment', 'issues.addLabels', 'issues.removeLabel']) {
    assert.ok(names.has(name), `${name} is called`);
  }
  assert.equal(repo.issue(3).state, 'closed');
  assert.equal(repo.issue(4).state, 'open');
  assert.deepEqual(sleeps, new Array(repo.mutations().length - 1).fill(1000));
});

test('a secondary rate limit in the middle of a run is waited out, and the run goes on to its end', async () => {
  const repo = new FakeRepo();
  const findings = ['CVE-1', 'CVE-2', 'CVE-3'].map((id) => finding({ id, source: 'osv', component: `x ${id}` }));
  await runScan(repo, found(findings[0]));
  repo.calls.length = 0;
  repo.failWith('issues.create', 403, SECONDARY, { 'retry-after': '45' });
  const sleeps = [];
  const core = await runScan(repo, found(...findings), fakeCore(), sleeps);
  assert.equal(repo.issues.length, 3);
  assert.equal(repo.calls.filter((call) => call.name === 'issues.create').length, 3);
  assert.deepEqual(sleeps, [45000, 1000]);
  assert.deepEqual(core.failures, []);
  assert.match(core.summaryText, /CVE-2: #2 opened/);
  assert.match(core.summaryText, /CVE-3: #3 opened/);
});

test('a limit that does not lift fails the run after the tries, with the issues made so far kept', async () => {
  const repo = new FakeRepo();
  const findings = ['CVE-1', 'CVE-2'].map((id) => finding({ id, source: 'osv', component: `x ${id}` }));
  const limit = new ApiError(403, SECONDARY);
  const create = repo.github.rest.issues.create;
  let attempts = 0;
  repo.github.rest.issues.create = async (params) => {
    if (params.title.startsWith('CVE-2')) {
      attempts += 1;
      throw limit;
    }
    return create(params);
  };
  const sleeps = [];
  await assert.rejects(runScan(repo, found(...findings), fakeCore(), sleeps), (error) => error === limit);
  assert.equal(attempts, 3);
  assert.deepEqual(sleeps.slice(-2), [60000, 60000]);
  assert.equal(repo.issues.length, 1);
});

test('output that does not parse changes nothing and fails the run', async () => {
  const repo = new FakeRepo();
  repo.addIssue({ title: 'CVE-1: a', body: '<!-- sbom-scan:CVE-1 -->', labels: ['sbom-scan'] });
  await assert.rejects(runScan(repo, ''), /is not JSON/);
  assert.deepEqual(repo.mutations(), []);
  assert.equal(repo.issue(1).state, 'open');
});
