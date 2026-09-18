// node --test tests/web/voice-export.test.cjs; synthetic data, no daemon or media.
const { test } = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');
const { webcrypto } = require('node:crypto');
const source = fs.readFileSync(path.join(__dirname, '../../src/web/assets/app.js'), 'utf8');
function declaration(name) {
  const start = source.search(new RegExp(`^  (?:async )?function ${name}\\(`, 'm'));
  assert.notEqual(start, -1);
  return source.slice(start, source.indexOf('\n  }', start) + 4);
}
function load(names, globals = {}, before = '') {
  const context = vm.createContext({ TextEncoder, Date, ...globals });
  vm.runInContext(`${before}\n${names.map(declaration).join('\n')}`, context);
  return context;
}
const plain = value => JSON.parse(JSON.stringify(value));
const { voiceExportRequest: request, voiceResultFacts: facts } = load(['voiceExportRequest', 'voiceResultFacts']);
const result = overrides => ({
  version: 1, scope: 'raw_voices', finalized: false, outcome: null,
  selection: { request: { chat: null, since: null, until: null, limit: null, offset: 0, overwrite: false }, since_ts: null, until_ts: null, target_username: null },
  selected_rows: null, exported: 0, associated: 0, unproven: 0, incomplete_items: 0, artifact_count: 0, artifacts_complete: false, diagnostics: [], ...overrides,
});

test('all-account and exact single selection, no directory or model path options', () => {
  assert.deepEqual(plain(request({ scope: 'all', chat: 'ignored' })), { offset: 0, overwrite: false });
  assert.deepEqual(plain(request({ scope: 'single', chat: ' exact,name ' })), { chat: ' exact,name ', offset: 0, overwrite: false });
  for (const chat of ['', ' \t ']) assert.throws(() => request({ chat }));
  assert.throws(() => request({ scope: 'other', chat: 'id' }));
  assert.throws(() => request({ scope: 'all', overwrite: true }));
});

test('None is unlimited and zero is explicitly empty, no business limit added', () => {
  assert(!('limit' in request({ scope: 'all', limit: 0 })));
  for (const count of [0, 1, 10001, Number.MAX_SAFE_INTEGER]) {
    const value = request({ scope: 'all', limited: true, limit: count, offset: count });
    assert.equal(value.limit, count); assert.equal(value.offset, count);
  }
  for (const value of ['', ' ', null, false, -1, 1.5, Infinity, NaN, Number.MAX_SAFE_INTEGER + 1]) {
    assert.throws(() => request({ scope: 'all', limited: true, limit: value }));
    assert.throws(() => request({ scope: 'all', offset: value }));
  }
});

test('date-only until includes last second, explicit times retained, reversed and Unix rejected', () => {
  assert.equal(request({ scope: 'all', since: '2026-09-18T23:59:59', until: '2026-09-18' }).until, '2026-09-18');
  assert.equal(request({ scope: 'all', until: '2026-09-18T12:34:56' }).until, '2026-09-18 12:34:56');
  assert.throws(() => request({ scope: 'all', since: '2026-09-19', until: '2026-09-18' }));
  assert.throws(() => request({ scope: 'all', until: '1720000000' }));
});

test('unknown selection is not zero and an unfinished report is not success', () => {
  const value = Object.fromEntries(facts(result()));
  for (const key of ['选中语音', '已导出语音', '已关联语音', '关联未证实', '不完整条目']) assert.equal(value[key], '未确定');
  assert.equal(value['原始语音结果'], '尚未确定');
  assert.equal(Object.fromEntries(facts(result({ outcome: 'success' })))['原始语音结果'], '尚未确定');
  assert.equal(value['请求条数上限'], '不限');
  assert(!('选择结果' in value));
  const empty = Object.fromEntries(facts(result({ selected_rows: 0, finalized: true, outcome: 'success', artifact_count: 1 })));
  assert.equal(empty['选中语音'], 0); assert.equal(empty['选择结果'], '空选择（已核验）');
});

test('partial and unproven remain qualified; unresolved identity is never inferred', () => {
  const partial = result({ finalized: true, outcome: 'partial', selected_rows: 2, exported: 1, unproven: 1, incomplete_items: 1, artifact_count: 3 });
  assert.equal(Object.fromEntries(facts(partial))['原始语音结果'], '部分完成');
  const unproven = Object.fromEntries(facts({ ...partial, outcome: 'success' }));
  assert.equal(unproven['原始语音结果'], '已导出，关联未证实');
  const single = result(); single.selection.request.chat = 'same name'; single.selection.request.limit = 0;
  const value = Object.fromEntries(facts(single));
  assert.equal(value['实际会话 username'], '尚未解析'); assert.equal(value['请求条数上限'], 0);
});

class Element {
  constructor(text = '') { this.children = []; this.textContent = text; this.value = ''; this.checked = false; this.events = {}; }
  append(...children) { this.children.push(...children); }
  replaceChildren(...children) { this.children = children; this.textContent = ''; }
  setAttribute(key, value) { this[key] = value; }
  removeAttribute(key) { delete this[key]; }
  add(option) { if (!this.children.length) this.value = option.value; this.children.push(option); }
  addEventListener(event, fn) { this.events[event] = fn; }
  text() { return String(this.textContent) + this.children.map(child => child.text()).join('|'); }
}

test('real form readers emit only nested voice selection and preserve None/0 switching', () => {
  const elements = [];
  const context = load(['voiceExportRequest', 'voiceExportFields'], {
    el: (tag, css, text = '') => { const element = new Element(text); element.tag = tag; elements.push(element); return element; },
    Option: class extends Element { constructor(label, value) { super(label); this.value = value; } },
    username: item => item.username, displayName: item => item.name,
  }, `const model={selected:null,sessions:[{username:'one',name:'same'},{username:'two',name:'same'}],contacts:[]}; const formReaders=[];`);
  const root = new Element(); context.voiceExportFields(root);
  const find = name => elements.find(element => element.name === name);
  const body = () => plain(vm.runInContext('(() => {const options={}; formReaders[0](options); return options;})()', context));
  assert.equal(find('voice_scope').value, 'single'); assert.throws(body);
  find('voice_chat').value = ' one '; assert.equal(body().voice_export.chat, ' one ');
  find('voice_scope').value = 'all'; find('voice_scope').events.change();
  assert(find('voice_chat').disabled); assert(!find('voice_chat').required);
  assert.deepEqual(body(), { voice_export: { offset: 0, overwrite: false } });
  find('voice_limited').checked = true; find('voice_limited').events.change(); assert.throws(body);
  find('voice_limit').value = '0'; find('voice_limit').events.input();
  assert.equal(body().voice_export.limit, 0); assert(root.text().includes('0（选空）'));
  find('voice_limited').checked = false; find('voice_limited').events.change(); assert(!('limit' in body().voice_export));
  assert(!elements.some(element => ['audio', 'video'].includes(element.tag)));
});

test('opening voices bypasses directory options and budgets', async () => {
  let calls = 0;
  const context = load(['openTask'], {
    $: () => ({ replaceChildren() {}, showModal() {} }), notice() {},
    voiceExportFields(target, selected) { calls++; assert.equal(selected, 'exact'); },
    taskFlags() { assert.fail('directory flags'); }, exportBudgets() { assert.fail('media budgets'); }, field() { assert.fail('generic field'); },
  }, `let submitting=false,taskSpec,formReaders=[],selection,formGeneration=0; const model={online:true};`);
  await context.openTask({ kind: 'export_voices', name: 'Voices', options: ['voice_export'] }, 'exact'); assert.equal(calls, 1);
});

test('response loss retains exact voice body and key even after host write capability disappears', async () => {
  const storage = new Map(), attempts = [];
  const context = load(['pendingKey', 'savedSubmission', 'submitTask'], {
    crypto: webcrypto, location: { host: 'test.invalid' },
    sessionStorage: { getItem: key => storage.get(key), setItem: (key, value) => storage.set(key, value), removeItem: key => storage.delete(key) },
    window: { confirm: () => true }, $: () => ({ close() {} }), notice() {}, errorText: error => error.message,
    loadTasks: async () => {}, taskId: task => task.id, openDetail() {},
    request: async (url, options) => { attempts.push(plain(options)); if (attempts.length === 1) throw new Error('response lost'); return { id: 'voice-task' }; },
  }, `let pendingSubmission=null,submitting=false; const model={online:true,epoch:0,accountKey:'account'};
      const availableTasks=[{kind:'export_voices',enabled:true}]; const taskSpec={kind:'export_voices',options:['voice_export']};
      let limit=0; const formReaders=[options=>{options.voice_export={limit,offset:0,overwrite:false};}];`);
  await context.submitTask({ preventDefault() {} }); vm.runInContext('limit=10001; availableTasks.length=0', context);
  await context.submitTask({ preventDefault() {} });
  assert.equal(attempts.length, 2); assert.deepEqual(attempts[1], attempts[0]); assert.equal(attempts[1].body.options.voice_export.limit, 0);
});

test('new voice submission is not sent without the advertised host capability', async () => {
  const context = load(['submitTask'], {
    savedSubmission: () => null, notice() {},
    request() { assert.fail('new submission must not be sent'); },
  }, `let submitting=false; const taskSpec={kind:'export_voices'}; const model={online:true}; const availableTasks=[];`);
  await context.submitTask({ preventDefault() {} });
});

test('host write refusal does not revoke valid artifact-read authentication', async () => {
  let authenticationFailures = 0;
  const context = load(['request'], {
    AbortController, DOMException, setTimeout, clearTimeout,
    authenticateFailure() { authenticationFailures++; },
    fetch: async () => ({ ok: false, status: 403, json: async () => ({ code: 'media_write_not_authorized', error: 'media_write_not_authorized' }) }),
  }, `const token='synthetic'; const model={epoch:0};`);
  await assert.rejects(context.request('/api/tasks', { method: 'POST', body: {} }), error => error.status === 403 && error.code === 'media_write_not_authorized');
  assert.equal(authenticationFailures, 0);
});

test('failed/cancelled voice results keep registered downloads, no success inference or player', () => {
  const nodes = new Map(), loaded = [], tags = [];
  const context = load(['voiceResultFacts', 'renderExportResult'], {
    $: id => { if (!nodes.has(id)) nodes.set(id, new Element()); return nodes.get(id); },
    el: (tag, css, text = '') => { tags.push(tag); return new Element(text); },
    taskId: task => task.id, statusOf: task => task.status, loadArtifacts: id => loaded.push(id),
  }, `const artifactErrors={}; let artifactPage={generation:0}; const model={state:{capabilities:{task_artifacts_v1:true}}};`);
  for (const status of ['failed', 'cancelled', 'interrupted']) {
    context.renderExportResult({ id: status, kind: 'export_voices', status, result: result({ selected_rows: 2, exported: 1, unproven: 1, artifact_count: 2, outcome: 'partial' }) });
    const text = nodes.get('task-result').text(); assert(text.includes('关联未证实')); assert(!text.includes('成功')); assert(!text.includes('计划会话'));
  }
  assert.deepEqual(loaded, ['failed', 'cancelled', 'interrupted']);
  context.renderExportResult({ id: 'running', kind: 'export_voices', status: 'running', result: result() });
  assert.equal(loaded.length, 3); assert(!tags.includes('audio'));
  context.renderExportResult({ id: 'pending-result', kind: 'export_voices', status: 'running', result: null });
  assert.equal(nodes.get('task-result').textContent, '尚未收到原始语音结果');
  assert.equal(nodes.get('task-artifacts').children.length, 0);
  assert.equal(loaded.length, 3);
  context.renderExportResult({ id: 'wrong', kind: 'export_voices', status: 'succeeded', result: { ...result(), scope: 'chat_history' } });
  assert.equal(nodes.get('task-result').textContent, '尚未收到原始语音结果');
  assert.equal(loaded.length, 3);
});
