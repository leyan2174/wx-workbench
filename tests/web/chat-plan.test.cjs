// Run: node --test tests/web/chat-plan.test.cjs
const { test } = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');
const source = fs.readFileSync(path.join(__dirname, '../../src/web/assets/app.js'), 'utf8');
function declaration(name) {
  const start = source.search(new RegExp(`^  (?:async )?function ${name}\\(`, 'm'));
  assert.notEqual(start, -1);
  const firstLine = source.slice(start, source.indexOf('\n', start));
  if (firstLine.endsWith(' }')) return firstLine;
  return source.slice(start, source.indexOf('\n  }', start) + 4);
}
function load(names, globals = {}, before = '') {
  const context = vm.createContext({ TextEncoder, Date, ...globals });
  vm.runInContext(before + '\n' + names.map(declaration).join('\n'), context);
  return context;
}
const api = load(['planRequest', 'validPlanRef', 'planSelected', 'planPreviewCount', 'setPlanChange', 'planSizeStatus', 'planResultFacts']);
const plain = value => JSON.parse(JSON.stringify(value));
const ref = { task_id: 'a'.repeat(64), artifact_id: 'b'.repeat(64), sha256: 'c'.repeat(64) };

test('explicit empty selection remains [], all is null, no paths or old options', () => {
  assert.deepEqual(plain(api.planRequest({ scope: 'selected' }, ['estimate'])), { users: [], exclude_users: [], size_mode: 'estimate', threads: 1 });
  assert.equal(api.planRequest({ scope: 'all', users: 'ignored' }, ['estimate']).users, null);
  const value = plain(api.planRequest({ scope: 'selected', users: 'same-a\nsame-b\nsame-a', exclude: 'same-b', start: '2026-09-18', end: '2026-09-18', path: 'must-not-exist' }, ['estimate']));
  assert.deepEqual(value.users, ['same-a', 'same-b']);
  assert.equal(value.end, '2026-09-18');
  assert.equal(value.start, '2026-09-18');
  assert(!('path' in value) && !('include_images' in value));
});

test('scan is opt-in per Web capability; threads are bounded 1..6', () => {
  assert.throws(() => api.planRequest({ scope: 'all', size_mode: 'scan' }, ['estimate']));
  assert.equal(api.planRequest({ scope: 'all', size_mode: 'scan', threads: 6 }, ['estimate', 'scan']).threads, 6);
  for (const threads of [0, 7, 1.5]) assert.throws(() => api.planRequest({ scope: 'all', threads }, ['estimate']));
});

test('blacklist only excludes 0; whitelist only includes 1', () => {
  for (const [flag, black, white] of [['', true, false], ['0', false, false], ['1', true, true]]) {
    assert.equal(api.planSelected(flag, 'blacklist'), black);
    assert.equal(api.planSelected(flag, 'whitelist'), white);
  }
});

test('same names remain independent username identities across pages and modes', () => {
  const changes = new Map();
  const first = { username: 'same-a', chat_name: 'Same name', export: '' };
  const second = { username: 'same-b', chat_name: 'Same name', export: '' };
  api.setPlanChange(changes, first, '0');
  api.setPlanChange(changes, second, '1');
  assert.equal(changes.size, 2);
  assert.equal(api.planPreviewCount(2, changes, 'blacklist'), 1);
  assert.equal(api.planPreviewCount(0, changes, 'whitelist'), 1);
  assert.deepEqual([...changes.keys()], ['same-a', 'same-b']);
  api.setPlanChange(changes, first, '');
  assert.equal(changes.size, 1);
  assert.throws(() => api.setPlanChange(changes, first, 'approved'));
});

test('plan refs are opaque identities, with distinct generated and apply summaries', () => {
  assert(api.validPlanRef(ref));
  assert(!api.validPlanRef({ ...ref, artifact_id: 'C:/private/plan.csv' }));
  const generated = plain(api.planResultFacts({ scope: 'chat_plan', outcome: 'partial', source_kind: 'runtime_snapshot', row_count: 2, partial_rows: 1, start_ts: null, end_ts: 0, published_plan_ref: ref, parent_ref: null, artifact_count: 1 }));
  assert(generated.some(([key, value]) => key === '版本 SHA-256' && value === ref.sha256));
  assert(!generated.some(([key]) => key === '媒体问题'));
  const applied = plain(api.planResultFacts({ scope: 'chat_plan_apply', outcome: 'success', finalized: true, plan_ref: ref, plan_mode: 'whitelist', dry_run: true, selected_count: 0, published_count: 0, failed_count: 0, messages: 0, artifact_count: 0 }));
  assert(applied.some(([key, value]) => key === '已选择会话' && value === 0));
  assert(applied.some(([key, value]) => key === '执行意图' && value === '仅预演选择'));
});

test('unfinished zero selection is unknown while published partial results remain visible', () => {
  const result = { scope: 'chat_plan_apply', outcome: 'failure', finalized: false, plan_ref: ref, plan_mode: 'blacklist', dry_run: false, selected_count: 0, published_count: 0, failed_count: 0, messages: 0, artifact_count: 0 };
  const pending = Object.fromEntries(plain(api.planResultFacts(result)));
  assert.equal(pending['已选择会话'], '尚未确认');
  assert.equal(pending['已记录失败会话'], 0);
  assert(!Object.hasOwn(pending, '失败会话'));
  const empty = Object.fromEntries(plain(api.planResultFacts({ ...result, finalized: true })));
  assert.equal(empty['已选择会话'], 0);
  assert.equal(empty['失败会话'], 0);
  const partial = Object.fromEntries(plain(api.planResultFacts({ ...result, selected_count: 3, published_count: 1, messages: 7, artifact_count: 1 })));
  assert.equal(partial['已选择会话'], 3);
  assert.equal(partial['已发布文档'], 1);
  assert.equal(partial['已发布消息'], 7);
  assert.equal(partial['已登记产物'], 1);
});

test('missing-source size status is visible in Chinese', () => {
  assert.equal(api.planSizeStatus('ok'), '完整');
  assert.equal(api.planSizeStatus('partial:message_db_missing,scan_limited'), '部分统计：消息库缺失、扫描数量受限');
});

test('unsaved edits and zero selected disable apply without inventing all', () => {
  const nodes = new Map();
  const get = id => { if (!nodes.has(id)) nodes.set(id, {}); return nodes.get(id); };
  const context = load(['planSelected', 'planPreviewCount', 'updatePlanActions'], { $: get }, `const availableTasks=[{kind:'chat_plan_apply'},{kind:'chat_plan_review'}]; const planReview={busy:false,page:{selected_count:0,total:2},mode:'whitelist',changes:new Map()};`);
  context.updatePlanActions(); assert.equal(get('plan-apply').disabled, true);
  vm.runInContext("planReview.changes.set('same-a',{original:'',export:'1'})", context);
  context.updatePlanActions(); assert.equal(get('plan-apply').disabled, true); assert.equal(get('plan-save-review').disabled, false);
  vm.runInContext("planReview.changes.clear();planReview.page.selected_count=1", context);
  context.updatePlanActions(); assert.equal(get('plan-apply').disabled, false);
});
