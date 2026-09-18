// Run with node --test tests/web/history-export.test.cjs; no Rust build or daemon.
const { test } = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');
const { webcrypto } = require('node:crypto');
const source = fs.readFileSync(path.join(__dirname, '../../src/web/assets/app.js'), 'utf8');

function declaration(name) {
  const start = source.search(new RegExp(`^  (?:async )?function ${name}\\(`, 'm'));
  assert.notEqual(start, -1, `production function ${name} exists`);
  const end = source.indexOf('\n  }', start) + 4;
  return source.slice(start, end);
}
function load(names, globals = {}, before = '') {
  const context = vm.createContext({ TextEncoder, Date, ...globals });
  vm.runInContext(`${before}\n${names.map(declaration).join('\n')}`, context);
  return context;
}
const { historyExportRequest: request, historyResultFacts: facts } = load(['historyExportRequest', 'historyResultFacts']);
const plain = value => JSON.parse(JSON.stringify(value));

test('four single formats, exact selector, defaults, and no unrelated task options', () => {
  assert.deepEqual(plain(request({ chat: 'wxid_example' })), { chat: 'wxid_example', limit: 500, format: 'markdown' });
  for (const format of ['markdown', 'txt', 'json', 'yaml']) {
    const value = plain(request({ chat: ' exact,name ', format }));
    assert.equal(value.chat, ' exact,name ');
    assert.equal(value.format, format);
    assert.deepEqual(Object.keys(value).sort(), ['chat', 'format', 'limit']);
  }
  assert.throws(() => request({ chat: 'id', format: 'html' }));
});

test('positive safe integer only, without a 10000 cap', () => {
  for (const limit of [1, 500, 10001, Number.MAX_SAFE_INTEGER]) assert.equal(request({ chat: 'id', limit }).limit, limit);
  for (const limit of [0, -1, 1.5, NaN, Infinity, Number.MAX_SAFE_INTEGER + 1, '']) assert.throws(() => request({ chat: 'id', limit }));
});

test('selector is UTF-8 bounded and not silently trimmed or split', () => {
  for (const chat of ['', '  ', '\tname', 'a\u007f', 'a\u0085', 'a'.repeat(257), '中'.repeat(86)]) assert.throws(() => request({ chat }));
  assert.equal(request({ chat: '中'.repeat(85) }).chat, '中'.repeat(85));
});

test('history until date includes end of day and datetime remains explicit', () => {
  const value = request({ chat: 'id', since: '2026-09-18T20:30', until: '2026-09-18' });
  assert.equal(value.since, '2026-09-18 20:30');
  assert.equal(value.until, '2026-09-18');
  assert.equal(request({ chat: 'id', until: '2026-09-18T12:34:56' }).until, '2026-09-18 12:34:56');
  assert.throws(() => request({ chat: 'id', since: '2026-09-19', until: '2026-09-18' }));
  assert.throws(() => request({ chat: 'id', since: '1720000000' }));
});

test('null query is unknown; zero messages is valid and no directory counters appear', () => {
  const base = { outcome: 'success', format: 'markdown', artifact_count: 1, query: null };
  const unknown = plain(facts(base));
  assert(unknown.some(([key, value]) => key === '查询摘要' && value.includes('尚未取得')));
  assert(!unknown.some(([key]) => key === '实际消息条数'));
  const empty = plain(facts({ ...base, query: { username: 'wxid', since_ts: 0, until_ts: null, limit: 500, messages: 0 } }));
  assert(empty.some(([key, value]) => key === '实际消息条数' && value === 0));
  assert(empty.some(([, value]) => value === '合法空结果'));
  assert(empty.some(([key, value]) => key === '开始边界（含）' && value === '1970-01-01 00:00:00 UTC'));
  assert(!empty.some(([key]) => /会话数|媒体|计划会话/.test(key)));
});

test('history kind is advertised only, and directory kind remains available', () => {
  const context = load(['descriptors'], {}, `const catalog = [{kind:'export_all'}, {kind:'export_history', advertisedOnly:true}];`);
  assert.deepEqual(plain(context.descriptors({})), [{ kind: 'export_all' }]);
  assert.equal(context.descriptors({ task_kinds: [{ kind: 'export_history', options: ['history_export'] }] })[0].kind, 'export_history');
  assert.equal(context.descriptors({ task_kinds: ['export_all'] })[0].kind, 'export_all');
});

test('response loss retries the original history body with the same idempotency key', async () => {
  const nodes = new Map(); const storage = new Map(); const attempts = [];
  const context = load(['pendingKey', 'savedSubmission', 'submitTask'], {
    crypto: webcrypto, location: { host: 'test.invalid' },
    sessionStorage: { getItem: key => storage.get(key), setItem: (key, value) => storage.set(key, value), removeItem: key => storage.delete(key) },
    window: { confirm: () => true },
    $: id => { if (!nodes.has(id)) nodes.set(id, { disabled: false, close() {} }); return nodes.get(id); },
    notice() {}, errorText: error => error.message, loadTasks: async () => {}, taskId: task => task.id, openDetail() {},
    request: async (url, options) => { attempts.push(plain(options)); if (attempts.length === 1) throw new Error('response lost'); return { id: 'original-task' }; },
  }, `let pendingSubmission=null,submitting=false; const model={online:true,epoch:0,accountKey:'account'};
      const availableTasks=[{kind:'export_history',enabled:true}];
      const taskSpec={kind:'export_history',options:['history_export']};
      let selectedChat='original'; const formReaders=[options=>{options.history_export={chat:selectedChat,limit:500,format:'yaml'};}];`);
  await context.submitTask({ preventDefault() {} });
  vm.runInContext("selectedChat='changed-after-loss'", context);
  await context.submitTask({ preventDefault() {} });
  assert.equal(attempts.length, 2);
  assert.match(attempts[0].idempotencyKey, /^[0-9a-f]{64}$/);
  assert.deepEqual(attempts[1], attempts[0]);
  assert.equal(attempts[1].body.options.history_export.chat, 'original');
});

test('pending history retry is retained without sending to an unsupported daemon', async () => {
  for (const availableTasks of [[], [{ kind: 'export_history', enabled: false }]]) {
    let warning = '';
    const context = load(['submitTask'], {
      availableTasks, savedSubmission: () => ({ body: { kind: 'export_history' } }),
      notice: (id, value) => { warning = value; },
      request() { assert.fail('unsupported history must not be submitted'); },
    }, `let submitting=false; const taskSpec={kind:'export_all'}; const model={online:true};`);
    await context.submitTask({ preventDefault() {} });
    assert.match(warning, /不支持或未启用/);
    assert.match(warning, /已保留/);
  }
});

test('opening history form bypasses directory flags, budget and generic fields', async () => {
  const nodes = new Map(); let historyCalls = 0;
  const context = load(['openTask'], {
    $: id => { if (!nodes.has(id)) nodes.set(id, { replaceChildren() {}, showModal() {} }); return nodes.get(id); },
    notice() {}, historyExportFields(target, selected) { historyCalls++; assert.equal(selected, 'exact-id'); },
    taskFlags() { assert.fail('directory options must not be added'); },
    exportBudgets() { assert.fail('directory budgets must not be added'); },
    field() { assert.fail('generic fields must not be added'); },
  }, `let submitting=false,taskSpec,formReaders=[],selection,formGeneration=0; const model={online:true};`);
  await context.openTask({ kind: 'export_history', name: 'History', options: ['history_export'] }, 'exact-id');
  assert.equal(historyCalls, 1);
});

test('history and directory render independently and retain registered downloads on failure', () => {
  class Element {
    constructor(text = '') { this.children = []; this.textContent = text; }
    append(...children) { this.children.push(...children); }
    replaceChildren(...children) { this.children = children; this.textContent = ''; }
    text() { return String(this.textContent) + this.children.map(child => child.text()).join('|'); }
  }
  const nodes = new Map(); const loaded = [];
  const context = load(['historyResultFacts', 'renderExportResult'], {
    $: id => { if (!nodes.has(id)) nodes.set(id, new Element()); return nodes.get(id); },
    el: (tag, className, text = '') => new Element(text),
    taskId: task => task.id, statusOf: task => task.status, loadArtifacts: id => loaded.push(id),
  }, `const artifactErrors={}; let artifactPage={generation:0}; const model={state:{capabilities:{task_artifacts_v1:true}}};`);
  const common = { version: 1, finalized: false, outcome: 'partial', artifact_count: 1, artifacts_complete: false, diagnostics: [] };
  context.renderExportResult({ id: 'history', kind: 'export_history', status: 'failed', result: { ...common, scope: 'chat_history', format: 'txt', query: null } });
  const history = nodes.get('task-result').text();
  assert(history.includes('尚未取得可信查询结果'));
  assert(!history.includes('计划会话') && !history.includes('媒体问题'));
  assert.equal(nodes.get('export-result-title').textContent, '单会话历史导出结果');
  context.renderExportResult({ id: 'directory', kind: 'export_all', status: 'cancelled', result: { ...common, scope: 'chat_directory', dry_run: false, planned_chats: 2, exported_chats: 1, failed_chats: 1, messages: 5, media_issues: 1 } });
  const directory = nodes.get('task-result').text();
  assert(directory.includes('计划会话') && directory.includes('媒体问题'));
  assert.deepEqual(loaded, ['history', 'directory']);
  context.renderExportResult({ id: 'wrong-scope', kind: 'export_history', status: 'succeeded', result: { ...common, scope: 'chat_directory' } });
  assert.equal(nodes.get('task-result').textContent, '暂无聊天导出结果');
  assert.deepEqual(loaded, ['history', 'directory']);
});
