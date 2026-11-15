// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

// A repository of GitHub, in memory, for the tests of the bots: the parts
// of the REST API they call, and a `core` that records what it is told.
// It keeps the same shapes the API answers with (`data`, `status` on an
// error), so that a bot that works against it asks the real API for what
// it needs.

'use strict';

class ApiError extends Error {
  constructor(status, message, headers = {}) {
    super(message);
    this.status = status;
    this.response = { status, headers };
  }
}

/** A `core` of actions/github-script that records its output. */
function fakeCore() {
  const core = {
    infos: [],
    warnings: [],
    errors: [],
    failures: [],
    summaryText: '',
    info: (message) => core.infos.push(message),
    warning: (message) => core.warnings.push(message),
    error: (message) => core.errors.push(message),
    setFailed: (message) => core.failures.push(message),
    summary: {
      addRaw(text) {
        core.summaryText += text;
        return core.summary;
      },
      async write() {},
    },
  };
  return core;
}

/** The `context` of a workflow run in the repository `owner/repo`. */
function fakeContext(payload = {}, owner = 'dukovo-os', repo = 'dukovo') {
  return { repo: { owner, repo }, payload };
}

/**
 * The repository: files, branches, labels, issues and pull requests,
 * and the calls that change it, in `calls`. `github` is what a script
 * receives.
 */
class FakeRepo {
  constructor() {
    this.defaultBranch = 'main';
    this.files = {};
    this.branches = { main: sha('main-1') };
    this.labels = [];
    this.issues = [];
    this.checkRuns = {};
    this.calls = [];
    this.pinned = [];
    this.dispatches = [];
    this.failOn = {};
    this.github = this.makeGithub();
  }

  /**
   * Makes `method` fail with `status` the next time a script calls it.
   * `headers` are those of the response, as Octokit keeps them on an error.
   */
  failWith(method, status, message = 'failed', headers = {}) {
    this.failOn[method] = new ApiError(status, message, headers);
  }

  addIssue({ title, body = '', labels = [], state = 'open', stateReason = null, events = [], createdAt, pullRequest = false, ...extra }) {
    const number = this.issues.length + 1;
    const issue = {
      number,
      node_id: `I_${number}`,
      title,
      body,
      state,
      state_reason: stateReason,
      labels: labels.map((name) => ({ name })),
      created_at: createdAt ?? '2026-01-01T00:00:00Z',
      html_url: `https://github.com/dukovo-os/dukovo/issues/${number}`,
      comments: [],
      events,
      ...extra,
    };
    if (pullRequest) {
      issue.pull_request = {};
      issue.html_url = `https://github.com/dukovo-os/dukovo/pull/${number}`;
    }
    this.issues.push(issue);
    return issue;
  }

  /**
   * A pull request, which is an issue too, as on GitHub. `merged` and
   * `mergeCommit` say how it ended; `base` is the branch it was for.
   */
  addPull({ title, body = '', labels = [], base = 'main', merged = false, mergeCommit = null, state }) {
    return this.addIssue({
      title,
      body,
      labels,
      pullRequest: true,
      state: state ?? (merged ? 'closed' : 'open'),
      merged,
      merge_commit_sha: mergeCommit,
      base: { ref: base },
      head: { ref: `topic-${this.issues.length + 1}`, user: 'dukovo-os' },
    });
  }

  issue(number) {
    return this.issues.find((issue) => issue.number === number);
  }

  labelNamesOf(number) {
    return this.issue(number).labels.map((label) => label.name);
  }

  makeGithub() {
    const repo = this;
    const log = (name, params) => {
      repo.calls.push({ name, params });
      if (repo.failOn[name]) {
        const error = repo.failOn[name];
        delete repo.failOn[name];
        throw error;
      }
    };
    const respond = (data) => Promise.resolve({ data, status: 200 });
    const listIssues = (params) => {
      const wanted = params.labels ? params.labels.split(',') : [];
      const state = params.state ?? 'open';
      return repo.issues.filter(
        (issue) =>
          (state === 'all' || issue.state === state) &&
          wanted.every((name) => issue.labels.some((label) => label.name === name)),
      );
    };
    const rest = {
      repos: {
        get: async (params) => {
          log('repos.get', params);
          return respond({ default_branch: repo.defaultBranch });
        },
        getContent: async (params) => {
          log('repos.getContent', params);
          const text = repo.files[params.path];
          if (text === undefined) {
            throw new ApiError(404, 'Not Found');
          }
          return respond({ content: Buffer.from(text).toString('base64'), encoding: 'base64' });
        },
        getBranch: async (params) => {
          log('repos.getBranch', params);
          const head = repo.branches[params.branch];
          if (!head) {
            throw new ApiError(404, 'Branch not found');
          }
          return respond({ name: params.branch, commit: { sha: head } });
        },
      },
      git: {
        listMatchingRefs: async (params) => {
          log('git.listMatchingRefs', params);
          return respond(
            Object.entries(repo.branches)
              .filter(([name]) => `heads/${name}`.startsWith(params.ref))
              .map(([name, head]) => ({ ref: `refs/heads/${name}`, object: { sha: head } })),
          );
        },
        createRef: async (params) => {
          log('git.createRef', params);
          const name = params.ref.replace(/^refs\/heads\//, '');
          if (repo.branches[name]) {
            throw new ApiError(422, 'Reference already exists');
          }
          repo.branches[name] = params.sha;
          return respond({ ref: params.ref });
        },
      },
      issues: {
        listLabelsForRepo: async (params) => {
          log('issues.listLabelsForRepo', params);
          return respond(repo.labels.map((name) => ({ name })));
        },
        createLabel: async (params) => {
          log('issues.createLabel', params);
          if (repo.labels.some((name) => name.toLowerCase() === params.name.toLowerCase())) {
            throw new ApiError(422, 'already_exists');
          }
          repo.labels.push(params.name);
          repo.labelDetails = { ...repo.labelDetails, [params.name]: params };
          return respond({ name: params.name });
        },
        listForRepo: async (params) => {
          log('issues.listForRepo', params);
          return respond(listIssues(params));
        },
        get: async (params) => {
          log('issues.get', params);
          return respond(repo.issue(params.issue_number));
        },
        create: async (params) => {
          log('issues.create', params);
          const issue = repo.addIssue({ title: params.title, body: params.body, labels: params.labels ?? [] });
          return respond(issue);
        },
        update: async (params) => {
          log('issues.update', params);
          const issue = repo.issue(params.issue_number);
          for (const key of ['title', 'body', 'state', 'state_reason']) {
            if (params[key] !== undefined) {
              issue[key] = params[key];
            }
          }
          return respond(issue);
        },
        createComment: async (params) => {
          log('issues.createComment', params);
          const issue = repo.issue(params.issue_number);
          issue.comments.push({ body: params.body });
          return respond({ body: params.body });
        },
        listComments: async (params) => {
          log('issues.listComments', params);
          return respond(repo.issue(params.issue_number).comments);
        },
        addLabels: async (params) => {
          log('issues.addLabels', params);
          const issue = repo.issue(params.issue_number);
          for (const name of params.labels) {
            if (!issue.labels.some((label) => label.name === name)) {
              issue.labels.push({ name });
            }
          }
          return respond(issue.labels);
        },
        removeLabel: async (params) => {
          log('issues.removeLabel', params);
          const issue = repo.issue(params.issue_number);
          if (!issue.labels.some((label) => label.name === params.name)) {
            throw new ApiError(404, 'Label does not exist');
          }
          issue.labels = issue.labels.filter((label) => label.name !== params.name);
          return respond(issue.labels);
        },
        listEvents: async (params) => {
          log('issues.listEvents', params);
          return respond(repo.issue(params.issue_number).events);
        },
      },
      pulls: {
        get: async (params) => {
          log('pulls.get', params);
          const pull = repo.issue(params.pull_number);
          if (!pull?.pull_request) {
            throw new ApiError(404, 'Not Found');
          }
          return respond(pull);
        },
        list: async (params) => {
          log('pulls.list', params);
          const state = params.state ?? 'open';
          return respond(
            repo.issues.filter(
              (pull) =>
                pull.pull_request &&
                (state === 'all' || pull.state === state) &&
                (!params.head || `${pull.head.user}:${pull.head.ref}` === params.head),
            ),
          );
        },
        create: async (params) => {
          log('pulls.create', params);
          const pull = repo.addPull({ title: params.title, body: params.body, base: params.base });
          pull.head = { ref: params.head, user: params.owner };
          return respond(pull);
        },
      },
      checks: {
        listForRef: async (params) => {
          log('checks.listForRef', params);
          return respond({ check_runs: repo.checkRuns[params.ref] ?? [] });
        },
      },
      actions: {
        createWorkflowDispatch: async (params) => {
          log('actions.createWorkflowDispatch', params);
          repo.dispatches.push(params);
          return { status: 204 };
        },
      },
    };
    return {
      rest,
      paginate: async (method, params) => {
        const { data } = await method(params);
        return data;
      },
      graphql: async (query, variables) => {
        log('graphql', { query, variables });
        if (/pinIssue/.test(query)) {
          repo.pinned.push(variables.id);
        }
        return {};
      },
    };
  }

  /** The names of the calls that changed something, in order. */
  mutations() {
    const reads = /^(repos\.get|repos\.getContent|repos\.getBranch|git\.listMatchingRefs|issues\.list|issues\.get|pulls\.get|pulls\.list|checks\.listForRef)/;
    return this.calls.filter((call) => !reads.test(call.name)).map((call) => call.name);
  }
}

/** A 40-character commit ID made from `seed`, the same for the same seed. */
function sha(seed) {
  let hex = '';
  let state = 0;
  for (const char of `${seed}`) {
    state = (state * 31 + char.charCodeAt(0)) >>> 0;
  }
  while (hex.length < 40) {
    state = (Math.imul(state, 1664525) + 1013904223) >>> 0;
    hex += state.toString(16).padStart(8, '0');
  }
  return hex.slice(0, 40);
}

module.exports = { ApiError, FakeRepo, fakeContext, fakeCore, sha };
