/* 本地静态工作台。图标路径来自 Lucide，许可见 LICENSE-lucide.txt。 */
/*
ISC License
Copyright (c) 2026 Lucide Icons and Contributors
Permission to use, copy, modify, and/or distribute this software for any
purpose with or without fee is hereby granted, provided that the above
copyright notice and this permission notice appear in all copies.
THE SOFTWARE IS PROVIDED "AS IS" AND THE AUTHOR DISCLAIMS ALL WARRANTIES
WITH REGARD TO THIS SOFTWARE INCLUDING ALL IMPLIED WARRANTIES OF
MERCHANTABILITY AND FITNESS. IN NO EVENT SHALL THE AUTHOR BE LIABLE FOR
ANY SPECIAL, DIRECT, INDIRECT, OR CONSEQUENTIAL DAMAGES OR ANY DAMAGES
WHATSOEVER RESULTING FROM LOSS OF USE, DATA OR PROFITS, WHETHER IN AN
ACTION OF CONTRACT, NEGLIGENCE OR OTHER TORTIOUS ACTION, ARISING OUT OF
OR IN CONNECTION WITH THE USE OR PERFORMANCE OF THIS SOFTWARE.

The MIT License (MIT), Feather-derived icons: chevron-left, chevron-right,
download, search, square, x. Copyright (c) 2013-present Cole Bemis
Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:
The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.
THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
*/
(() => {
  'use strict';
  const $ = (id) => document.getElementById(id);
  const NS = 'http://www.w3.org/2000/svg';
  const ICONS = {
    'message-square': '<path d="M22 17a2 2 0 0 1-2 2H6.828a2 2 0 0 0-1.414.586l-2.202 2.202A.71.71 0 0 1 2 21.286V5a2 2 0 0 1 2-2h16a2 2 0 0 1 2 2z"/>',
    users: '<path d="M16 21v-2a4 4 0 0 0-4-4H6a4 4 0 0 0-4 4v2"/><path d="M16 3.128a4 4 0 0 1 0 7.744"/><path d="M22 21v-2a4 4 0 0 0-3-3.87"/><circle cx="9" cy="7" r="4"/>',
    settings: '<path d="M9.671 4.136a2.34 2.34 0 0 1 4.659 0 2.34 2.34 0 0 0 3.319 1.915 2.34 2.34 0 0 1 2.33 4.033 2.34 2.34 0 0 0 0 3.831 2.34 2.34 0 0 1-2.33 4.033 2.34 2.34 0 0 0-3.319 1.915 2.34 2.34 0 0 1-4.659 0 2.34 2.34 0 0 0-3.32-1.915 2.34 2.34 0 0 1-2.33-4.033 2.34 2.34 0 0 0 0-3.831A2.34 2.34 0 0 1 6.35 6.051a2.34 2.34 0 0 0 3.319-1.915"/><circle cx="12" cy="12" r="3"/>',
    'refresh-cw': '<path d="M3 12a9 9 0 0 1 9-9 9.75 9.75 0 0 1 6.74 2.74L21 8"/><path d="M21 3v5h-5"/><path d="M21 12a9 9 0 0 1-9 9 9.75 9.75 0 0 1-6.74-2.74L3 16"/><path d="M8 16H3v5"/>',
    search: '<path d="m21 21-4.34-4.34"/><circle cx="11" cy="11" r="8"/>',
    image: '<rect width="18" height="18" x="3" y="3" rx="2" ry="2"/><circle cx="9" cy="9" r="2"/><path d="m21 15-3.086-3.086a2 2 0 0 0-2.828 0L6 21"/>',
    x: '<path d="M18 6 6 18"/><path d="m6 6 12 12"/>',
    'chevron-left': '<path d="m15 18-6-6 6-6"/>',
    'chevron-right': '<path d="m9 18 6-6-6-6"/>',
    download: '<path d="M12 15V3"/><path d="M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4"/><path d="m7 10 5 5 5-5"/>',
    play: '<path d="M5 5a2 2 0 0 1 3.008-1.728l11.997 6.998a2 2 0 0 1 .003 3.458l-12 7A2 2 0 0 1 5 19z"/>',
    square: '<rect width="18" height="18" x="3" y="3" rx="2"/>',
    'list-checks': '<path d="M13 5h8"/><path d="M13 12h8"/><path d="M13 19h8"/><path d="m3 17 2 2 4-4"/><path d="m3 7 2 2 4-4"/>'
  };
  function icon(name) {
    const svg = document.createElementNS(NS, 'svg');
    svg.setAttribute('viewBox', '0 0 24 24'); svg.setAttribute('class', 'icon'); svg.setAttribute('aria-hidden', 'true');
    // 只插入内置许可图标，所有 API 文本均使用 textContent。
    svg.innerHTML = ICONS[name] || ICONS['list-checks'];
    return svg;
  }
  function el(tag, className, text) {
    const node = document.createElement(tag);
    if (className) node.className = className;
    if (text !== undefined) node.textContent = String(text);
    return node;
  }
  function button(text, handler, className = '') {
    const node = el('button', className, text); node.type = 'button'; node.addEventListener('click', handler); return node;
  }
  function notice(id, text = '') { $(id).textContent = text; $(id).hidden = !text; }
  function empty(node, text) { node.replaceChildren(el('p', 'empty', text)); }
  const storage = { get(key) { try { return sessionStorage.getItem(key); } catch { return null; } }, set(key, value) { try { value ? sessionStorage.setItem(key, value) : sessionStorage.removeItem(key); } catch { /* 禁用存储时仅用内存。 */ } } };
  const fragment = new URLSearchParams(location.hash.slice(1));
  let token = fragment.get('token') || fragment.get('wx_token') || storage.get('wx.web.token') || '';
  if (location.hash) {
    history.replaceState(null, '', location.pathname + location.search);
  }
  storage.set('wx.web.token', token);
  const model = { state: {}, contacts: [], sessions: [], tags: [], tagCache: new Map(), tagMembership: new Map(), tagBusy: false, tagError: '', tagSeq: 0, tasks: [], messages: [], selected: null, source: 'wechat', view: 'sessions', offset: 0, total: null, hasMore: false, detailId: null, detail: null, online: false, live: true, epoch: 0, historySeq: 0, tasksSeq: 0, detailSeq: 0, directoryError: {}, connectedOnce: false };
  let historyController, directoryController, streamController, streamTimer, pollTimer, refreshTimer, streamGeneration = 0, refreshPending = false, refreshQueued = false, taskEventsPending = false, dataEventsPending = false;
  let imageListController, imageController, imageUrl, imageOffset = 0, imageGeneration = 0;
  let capabilitiesDirty = false, directoriesDirty = false, directoryRevision = 0, directorySeq = 0;
  let notificationRuntime, notificationSettings = { enabled: false, sound_enabled: true, rules: [] }, notificationStreamReady = false, soundContext;
  const notificationWatermarks = new Map(), openNotifications = new Set();
  const inlineImages = new Map(), inlineQueue = [];
  let inlineActive = 0, inlineGeneration = 0, inlineBytes = 0;
  let inlineRetryAt = 0, inlinePumpTimer;
  const monitorWarning = el('p', $('history-error').className); monitorWarning.id = 'monitor-warning'; monitorWarning.hidden = true;
  monitorWarning.setAttribute('role', 'status'); $('history-error').after(monitorWarning);
  const activeStates = new Set(['queued', 'pending', 'running', 'cancelling', 'cancel_requested']);
  const labels = { queued: '排队中', pending: '等待中', running: '运行中', cancelling: '取消中', cancel_requested: '取消中', completed: '已完成', succeeded: '已完成', success: '已完成', failed: '失败', cancelled: '已取消', canceled: '已取消', interrupted: '已中断' };
  const catalog = [
    { kind: 'wechat_keys', name: '数据库密钥', group: '个人微信', icon: 'search' },
    { kind: 'wechat_decrypt', name: '微信解密', group: '个人微信', icon: 'play' },
    { kind: 'image_key', name: '图片密钥', group: '个人微信', icon: 'search' },
    { kind: 'export_all', name: '导出聊天', group: '个人微信', icon: 'download', export: true },
    { kind: 'export_history', name: '导出单会话历史', group: '个人微信', icon: 'download', advertisedOnly: true },
    { kind: 'chat_plan', name: '生成导出计划', group: '个人微信', icon: 'list-checks', advertisedOnly: true },
    { kind: 'chat_plan_review', name: '保存计划修订', group: '个人微信', icon: 'list-checks', advertisedOnly: true, contextOnly: true },
    { kind: 'chat_plan_apply', name: '应用导出计划', group: '个人微信', icon: 'play', advertisedOnly: true, contextOnly: true },
    { kind: 'decode_images', name: '批量解密图片', group: '个人微信', icon: 'play' },
    { kind: 'sns_decrypt', name: '朋友圈解密与导出', group: '朋友圈', icon: 'download', users: true },
  ];
  let availableTasks = catalog.filter(spec => !spec.advertisedOnly), taskSpec = null, formReaders = [], selection = new Set(), submitting = false, formGeneration = 0;
  function unwrap(value) { return value && typeof value === 'object' && !Array.isArray(value) && value.data !== undefined ? value.data : value; }
  function rows(value, key) { const data = unwrap(value); if (Array.isArray(data)) return data; if (data && Array.isArray(data[key])) return data[key]; if (data && Array.isArray(data.items)) return data.items; throw new Error('接口返回的列表格式不正确'); }
  function statusOf(task) { return String(task.status || task.state || 'unknown').toLowerCase(); }
  function taskId(task) { return String(task.id ?? task.task_id ?? ''); }
  function taskName(task) { return task.name || availableTasks.find((x) => x.kind === task.kind)?.name || task.kind || '任务'; }
  function username(item) { return String(item.username ?? item.user_name ?? ''); }
  function displayName(item) { return item.remark || item.display_name || item.display || item.name || item.nickname || username(item); }
  function redact(value) { const text = typeof value === 'string' ? value : JSON.stringify(value, null, 2); return token ? String(text ?? '').split(token).join('[令牌已隐藏]') : String(text ?? ''); }
  function errorText(error) {
    const planErrors = { plan_ref_unavailable: '计划引用已不可用，请重新选择计划', plan_ref_changed: '计划内容已变更，已停止操作', plan_selection_invalid: '计划选择无效或会话已变化，请重新审阅', plan_scan_not_authorized: '本次 Web 启动未允许计划扫描', invalid_page: '计划分页参数无效' };
    return planErrors[error.code] || redact(error.message || error);
  }
  function timestamp(value, short = false) {
    if (!value) return '';
    const numeric = Number(value), date = new Date(Number.isFinite(numeric) ? numeric * (numeric < 1e12 ? 1000 : 1) : value);
    if (!Number.isFinite(date.getTime())) return String(value);
    return short ? date.toLocaleDateString('zh-CN', { month: '2-digit', day: '2-digit' }) : date.toLocaleString('zh-CN', { hour12: false });
  }
  function notificationKey() { return typeof model.state.runtime_id === 'string' ? `wx.web.notify.${model.state.runtime_id}` : null; }
  function saveNotifications() {
    const key = notificationKey(); if (!key) return;
    try { localStorage.setItem(key, JSON.stringify(notificationSettings)); }
    catch { notice('notify-status', '通知设置暂未保存，仅在本页生效。'); }
  }
  function notificationPermission() {
    return 'Notification' in window ? Notification.permission : 'unsupported';
  }
  function updateNotificationPermission() {
    if (!$('notify-status')) return;
    const labels = { granted: '桌面通知已授权', denied: '桌面通知被浏览器阻止', default: '桌面通知尚未授权', unsupported: '当前浏览器不支持桌面通知' };
    notice('notify-status', labels[notificationPermission()]);
    $('notify-permission').disabled = !['default', 'granted'].includes(notificationPermission());
  }
  function loadNotificationSettings() {
    if (!$('notify-enabled') || notificationRuntime === notificationKey()) return;
    notificationRuntime = notificationKey();
    notificationSettings = { enabled: false, sound_enabled: true, rules: [] };
    try {
      const raw = JSON.parse(localStorage.getItem(notificationRuntime) || 'null');
      if (raw && typeof raw === 'object') notificationSettings = { enabled: raw.enabled === true, sound_enabled: raw.sound_enabled !== false,
        rules: (Array.isArray(raw.rules) ? raw.rules : []).slice(0, 50).map((rule) => ({ group_name: typeof rule?.group_name === 'string' ? rule.group_name.slice(0, 160) : '', sender_name: typeof rule?.sender_name === 'string' ? rule.sender_name.slice(0, 160) : '', notify_on_any: rule?.notify_on_any === true })) };
    } catch { /* 无效存储只重置当前账号偏好。 */ }
    $('notify-enabled').checked = notificationSettings.enabled; $('notify-sound').checked = notificationSettings.sound_enabled;
    renderNotificationRules(); updateNotificationPermission();
  }
  async function prepareSound() {
    try {
      const Audio = window.AudioContext || window.webkitAudioContext;
      if (!Audio) { notice('notify-status', '当前浏览器不支持声音提醒。'); return; }
      if (!soundContext || soundContext.state === 'closed') soundContext = new Audio();
      await soundContext.resume();
    } catch { notice('notify-status', '声音暂不可用，请重新点击试听。'); }
  }
  function playNotificationSound() {
    if (soundContext?.state !== 'running') return;
    try {
      const oscillator = soundContext.createOscillator(), gain = soundContext.createGain();
      oscillator.connect(gain); gain.connect(soundContext.destination);
      oscillator.frequency.value = 880; gain.gain.value = 0.15;
      oscillator.onended = () => { oscillator.disconnect(); gain.disconnect(); };
      oscillator.start(); oscillator.stop(soundContext.currentTime + 0.15);
    } catch { /* 音频设备失效不影响消息刷新。 */ }
  }
  function closeNotifications() {
    for (const notification of openNotifications) notification.close(); openNotifications.clear();
    if (soundContext) { soundContext.close().catch(() => {}); soundContext = undefined; }
  }
  function renderNotificationRules() {
    const target = $('notify-rules'); target.replaceChildren();
    notificationSettings.rules.forEach((rule, index) => {
      const entry = el('fieldset'), heading = el('legend', '', `规则 ${index + 1}`);
      entry.append(heading);
      for (const [key, label] of [['group_name', '会话名称包含'], ['sender_name', '发送人包含（可选）']]) {
        const field = el('label', 'field', label), input = el('input'); input.type = 'text'; input.maxLength = 160; input.value = rule[key]; input.setAttribute('aria-label', `规则 ${index + 1} ${label}`);
        input.addEventListener('input', () => { rule[key] = input.value; saveNotifications(); }); field.append(input); entry.append(field);
      }
      const active = el('label', 'check'), checkbox = el('input'); checkbox.type = 'checkbox'; checkbox.checked = rule.notify_on_any;
      checkbox.addEventListener('change', () => { rule.notify_on_any = checkbox.checked; saveNotifications(); }); active.append(checkbox, el('span', '', '匹配时通知'));
      const remove = button('', () => { notificationSettings.rules.splice(index, 1); saveNotifications(); renderNotificationRules(); }, 'icon-button'); remove.title = `删除规则 ${index + 1}`; remove.setAttribute('aria-label', remove.title); remove.append(icon('x'));
      const actions = el('div', 'choice-actions'); actions.append(active, remove); entry.append(actions); target.append(entry);
    });
    $('notify-add-rule').disabled = notificationSettings.rules.length >= 50;
  }
  function initNotificationSettings() {
    const section = el('section'); section.append(el('h3', '', '消息通知'));
    for (const [id, text, key] of [['notify-enabled', '启用规则通知', 'enabled'], ['notify-sound', '声音提醒', 'sound_enabled']]) {
      const label = el('label', 'check'), input = el('input'); input.type = 'checkbox'; input.id = id;
      input.addEventListener('change', () => { notificationSettings[key] = input.checked; saveNotifications(); if (notificationSettings.enabled && notificationSettings.sound_enabled) prepareSound(); });
      label.append(input, el('span', '', text)); section.append(label);
    }
    const actions = el('div', 'choice-actions');
    const permission = button('授权桌面通知', async () => {
      if (!('Notification' in window)) return;
      try { await Notification.requestPermission(); updateNotificationPermission(); }
      catch { notice('notify-status', '浏览器未允许通知授权。'); }
    }); permission.id = 'notify-permission';
    const sound = button('', async () => { await prepareSound(); playNotificationSound(); }, 'icon-button'); sound.id = 'notify-test-sound'; sound.title = '试听提示音'; sound.setAttribute('aria-label', sound.title); sound.append(icon('play'));
    actions.append(permission, sound); section.append(actions);
    const status = el('p', 'muted'); status.id = 'notify-status'; status.setAttribute('role', 'status'); section.append(status);
    const rules = el('div'); rules.id = 'notify-rules'; section.append(rules);
    const add = button('添加规则', () => { notificationSettings.rules.push({ group_name: '', sender_name: '', notify_on_any: true }); saveNotifications(); renderNotificationRules(); }); add.id = 'notify-add-rule'; section.append(add);
    $('settings-dialog').querySelector('.dialog-body').append(section);
    loadNotificationSettings();
  }
  function notifyLiveMessage(message) {
    const delivery = message?.web_delivery, runtime = model.state.runtime_id;
    if (!notificationStreamReady || !delivery || delivery.runtime_id !== runtime || delivery.committed !== true || delivery.replay !== false || !Number.isSafeInteger(delivery.sequence) || delivery.sequence <= 0) return;
    if (delivery.sequence <= (notificationWatermarks.get(runtime) || 0)) return;
    notificationWatermarks.set(runtime, delivery.sequence);
    if (!model.online || !model.live || model.source !== 'wechat' || !notificationSettings.enabled) return;
    const chat = String(message.chat || message.chat_name || username(message)), sender = String(message.sender_name || message.sender || '');
    const matches = notificationSettings.rules.some((rule) => rule.notify_on_any && rule.group_name.trim() && chat.toLocaleLowerCase().includes(rule.group_name.toLocaleLowerCase()) && (!rule.sender_name || sender.toLocaleLowerCase().includes(rule.sender_name.toLocaleLowerCase())));
    if (!matches) return;
    if (notificationPermission() === 'granted') {
      try {
        const epoch = model.epoch, notification = new Notification(`${chat}${sender ? ` - ${sender}` : ''}`.slice(0, 160), { body: messageText(message).slice(0, 100), tag: `${runtime}:${delivery.sequence}` });
        openNotifications.add(notification); notification.onclose = () => openNotifications.delete(notification);
        notification.onclick = () => { notification.close(); if (epoch !== model.epoch || runtime !== model.state.runtime_id || model.source !== 'wechat') return; window.focus(); const person = model.sessions.find((item) => username(item) === username(message)); if (person) selectPerson(person); };
      } catch { /* 系统通知失败不影响消息记录。 */ }
    }
    if (notificationSettings.sound_enabled) playNotificationSound();
  }
  function connected(text, state = '') { $('connection').textContent = text; $('connection').className = `status ${state}`; }
  function authenticateFailure() {
    closeImages(); closeNotifications(); resetInlineImages();
    model.online = false; stopEvents(); connected('认证失效', 'failed');
    notice('global-error', '认证失败，请在设置中重新输入访问令牌。');
    $('auth-state').textContent = '令牌无效或已过期'; renderTools(); updateImageButton();
  }
  async function request(path, { method = 'GET', body, signal, stream = false, image = false, idempotencyKey } = {}) {
    // 同源认证与写操作保护均由本次启动令牌头承担，不写入 URL。
    const headers = { 'X-WX-Token': token, Accept: stream ? 'text/event-stream' : image ? 'image/*' : 'application/json' };
    if (idempotencyKey) headers['Idempotency-Key'] = idempotencyKey;
    if (body !== undefined) headers['Content-Type'] = 'application/json';
    const epoch = model.epoch;
    const controller = new AbortController(); let timedOut = false;
    const abort = () => controller.abort();
    if (signal?.aborted) abort(); else signal?.addEventListener('abort', abort, { once: true });
    const timer = setTimeout(() => { timedOut = true; controller.abort(); }, 30000);
    try {
    const response = await fetch(path, { method, headers, body: body === undefined ? undefined : JSON.stringify(body), signal: controller.signal, cache: 'no-store', credentials: 'omit', redirect: 'error' });
    if (epoch !== model.epoch) throw new DOMException('连接已更换', 'AbortError');
    if (!response.ok) {
      let message = `请求失败（HTTP ${response.status}）`, code;
      try { const data = await response.json(); code = data.error?.code || data.code; message = data.error?.message || data.error || data.message || message; } catch { /* 非 JSON 错误保留状态码。 */ }
      if (response.status === 401 || (response.status === 403 && code !== 'plan_scan_not_authorized')) authenticateFailure();
      const error = new Error(typeof message === 'string' ? message : JSON.stringify(message)); error.status = response.status; error.code = code; throw error;
    }
    // SSE 在收到响应头后取消首包超时，持续流的生命周期仍由调用者控制。
    if (stream) { clearTimeout(timer); return response; }
    if (image) {
      if (!/^image\/(jpeg|png|gif|webp|bmp)(;|$)/i.test(response.headers.get('content-type') || '')) throw new Error('图片格式不支持预览');
      const blob = await response.blob();
      if (!blob.size || blob.size > 16 * 1024 * 1024) throw new Error('图片大小超出预览限额');
      if (epoch !== model.epoch || controller.signal.aborted) throw new DOMException('图片请求已取消', 'AbortError');
      return blob;
    }
    if (response.status === 204) return null;
    const result = await response.json();
    if (epoch !== model.epoch) throw new DOMException('连接已更换', 'AbortError');
    if (result?.ok === false) throw new Error(result.error?.message || result.error || '操作失败');
    return unwrap(result);
    } catch (error) {
      if (timedOut) throw new Error('请求超时，请检查本地服务状态');
      throw error;
    } finally {
      clearTimeout(timer);
      // SSE 保留上游取消监听，直到调用方结束这一轮连接。
      if (!stream) signal?.removeEventListener('abort', abort);
    }
  }
  function updateImageButton() {
    $('preview-images').disabled = !model.online || !model.selected || model.source !== 'wechat' || model.state.image_preview?.enabled !== true;
  }
  function releaseImage() {
    imageController?.abort(); imageController = null;
    if (imageUrl) URL.revokeObjectURL(imageUrl);
    imageUrl = null; $('image-preview').replaceChildren();
  }
  function resetImages() {
    ++imageGeneration; imageListController?.abort(); imageListController = null; releaseImage();
    $('image-list').replaceChildren(); notice('images-error');
  }
  function closeImages() {
    if ($('images-dialog').open) $('images-dialog').close();
    resetImages();
  }
  async function showImage(row, node, generation) {
    releaseImage(); notice('images-error');
    const id = row.attachment_id;
    // 只接受后端附件身份对应的同源路由，不接受图片 URL 或任意文件路径。
    if (typeof id !== 'string' || id.length > 2048 || row.preview_url !== `/api/images/${id}`) {
      notice('images-error', '此图片缺少唯一资源绑定'); return;
    }
    const controller = new AbortController(); imageController = controller;
    for (const button of $('image-list').querySelectorAll('button')) button.setAttribute('aria-pressed', String(button === node));
    empty($('image-preview'), '正在读取图片…');
    try {
      const blob = await request(`/api/images/${encodeURIComponent(id)}`, { image: true, signal: controller.signal });
      if (controller.signal.aborted || generation !== imageGeneration || !$('images-dialog').open) return;
      imageUrl = URL.createObjectURL(blob);
      const picture = el('img'); picture.alt = '当前会话的缓存图片'; picture.src = imageUrl;
      picture.addEventListener('error', () => {
        if (controller !== imageController) return;
        releaseImage(); notice('images-error', '浏览器无法显示这张缓存图片');
      }, { once: true });
      $('image-preview').replaceChildren(picture);
    } catch (error) {
      if (error.name !== 'AbortError' && generation === imageGeneration && controller === imageController) {
        empty($('image-preview'), '图片不可用'); notice('images-error', errorText(error));
      }
    }
  }
  async function loadImages() {
    if (!$('images-dialog').open || !model.selected || model.source !== 'wechat') return;
    resetImages(); const generation = imageGeneration, chat = username(model.selected);
    const controller = new AbortController(); imageListController = controller;
    $('images-previous').disabled = true; $('images-next').disabled = true;
    $('images-page').textContent = String(imageOffset / 20 + 1);
    empty($('image-list'), '正在加载图片…'); empty($('image-preview'), '未选择图片');
    try {
      const data = await request(`/api/images?${new URLSearchParams({ source: 'wechat', chat, limit: '20', offset: String(imageOffset) })}`, { signal: controller.signal });
      if (generation !== imageGeneration || controller.signal.aborted) return;
      if (data?.username !== chat) throw new Error('图片列表会话身份不一致');
      const entries = rows(data, 'attachments'); $('image-list').replaceChildren();
      if (!entries.length) empty($('image-list'), '暂无图片');
      entries.forEach((row, index) => {
        const node = button('', () => showImage(row, node, generation), 'image-entry');
        node.append(icon('image'), el('span', '', timestamp(row.create_time || row.timestamp) || `图片 ${imageOffset + index + 1}`));
        node.setAttribute('aria-pressed', 'false');
        node.disabled = row.resource_status !== 'found' || !row.preview_url;
        if (node.disabled) node.title = '图片资源未找到或身份不唯一';
        $('image-list').append(node);
      });
      $('images-next').disabled = typeof data.has_more === 'boolean' ? !data.has_more : entries.length < 20;
    } catch (error) {
      if (error.name !== 'AbortError' && generation === imageGeneration) {
        empty($('image-list'), '图片列表加载失败'); notice('images-error', errorText(error));
      }
    } finally {
      if (generation === imageGeneration) $('images-previous').disabled = imageOffset === 0;
    }
  }
  function tagNames(item, source = model.source) {
    const tags = item.tags || item.labels || [];
    const direct = (Array.isArray(tags) ? tags : []).map((tag) => typeof tag === 'object' ? tag.name || tag.label || String(tag.id) : String(tag));
    const linked = source === 'wechat' ? model.tagMembership.get(username(item)) || [] : [];
    return [...new Set([...direct, ...linked])];
  }
  function personType(item) {
    const id = username(item), type = String(item.chat_type || item.type || '');
    return id.endsWith('@chatroom') || ['group', '群', '群聊'].includes(type) ? 'group' : id.startsWith('gh_') || ['official_account', 'public', '公众号'].includes(type) ? 'public' : 'direct';
  }
  function mergedPeople() {
    if (model.view === 'contacts') return model.contacts;
    const contacts = new Map(model.contacts.map((item) => [username(item), item]));
    return model.sessions.map((item) => ({ ...contacts.get(username(item)), ...item, tags: item.tags || contacts.get(username(item))?.tags }));
  }
  function renderTags() {
    const selected = $('tag-filter').value;
    model.tagMembership.clear();
    const tagsWithMembers = new Map(model.tags.map((tag) => [tag.name || tag.label || String(tag.id), tag]));
    model.tagCache.forEach((tag, name) => tagsWithMembers.set(name, tag));
    tagsWithMembers.forEach((tag) => {
      const name = tag.name || tag.label || String(tag.id);
      (tag.members || tag.usernames || []).forEach((member) => {
        const id = typeof member === 'string' ? member : username(member);
        if (!model.tagMembership.has(id)) model.tagMembership.set(id, []);
        model.tagMembership.get(id).push(name);
      });
    });
    const names = new Set([...model.contacts, ...model.sessions].flatMap((item) => tagNames(item)));
    $('tag-filter').replaceChildren(new Option('全部标签', ''));
    [...names].sort((a, b) => a.localeCompare(b, 'zh-CN')).forEach((name) => $('tag-filter').add(new Option(name, name)));
    model.tags.forEach((tag) => {
      const name = tag.name || tag.label || String(tag.id); if (names.has(name)) return;
      const option = new Option(`${name}${tag.member_count !== undefined ? ` (${tag.member_count})` : ''}`, name);
      $('tag-filter').add(option);
    });
    if ([...$('tag-filter').options].some((option) => option.value === selected)) $('tag-filter').value = selected;
  }
  async function loadTag(name) {
    if (!name || model.tagCache.has(name)) return;
    const epoch = model.epoch;
    const data = await request(`/api/tag-members?${new URLSearchParams({ name })}`);
    if (epoch !== model.epoch) throw new DOMException('账号已切换', 'AbortError');
    if (!data || !Array.isArray(data.members)) throw new Error('标签接口未返回成员列表');
    model.tagCache.set(name, data); renderTags();
  }
  async function chooseTag() {
    const name = $('tag-filter').value, seq = ++model.tagSeq, epoch = model.epoch;
    model.tagError = ''; model.tagBusy = Boolean(name && !model.tagCache.has(name)); renderDirectory();
    try { if (model.source === 'wechat' && name) await loadTag(name); }
    catch (error) { if (seq === model.tagSeq && epoch === model.epoch) model.tagError = `标签成员加载失败：${errorText(error)}`; }
    finally { if (seq === model.tagSeq && epoch === model.epoch) { model.tagBusy = false; renderDirectory(); } }
  }
  function renderDirectory() {
    const target = $('directory-list'), query = $('directory-search').value.trim().toLocaleLowerCase(), type = $('type-filter').value, tag = $('tag-filter').value;
    if (model.tagBusy) { $('directory-count').textContent = '…'; empty(target, '正在加载标签成员…'); return; }
    if (model.tagError) { $('directory-count').textContent = ''; empty(target, model.tagError); target.append(button('重试标签', chooseTag)); return; }
    const people = mergedPeople().filter((item) => username(item) && (!query || [displayName(item), username(item), item.nickname, item.remark].join(' ').toLocaleLowerCase().includes(query)) && (!type || personType(item) === type) && (!tag || tagNames(item).includes(tag)));
    $('directory-count').textContent = String(people.length);
    $('directory-limit').hidden = model[model.view].length < 2000;
    target.replaceChildren();
    if (model.directoryError[model.view]) { empty(target, model.directoryError[model.view]); target.append(button('重新加载', refreshAll)); return; }
    if (!people.length) { empty(target, query || type || tag ? '没有符合筛选条件的记录' : model.online ? '暂无记录' : '尚未连接本地服务'); return; }
    const fragment = document.createDocumentFragment();
    for (const item of people) {
      const node = button('', () => selectPerson(item), `person ${personType(item)}`);
      node.setAttribute('aria-current', String(username(item) === username(model.selected || {}) || (item.conversations || []).some((chat) => username(chat) === username(model.selected || {}))));
      const avatar = el('span', 'avatar'); avatar.append(icon(personType(item) === 'group' ? 'users' : 'message-square'));
      const body = el('span', 'person-body'), top = el('span', 'person-top');
      top.append(el('span', 'person-name', displayName(item)), el('time', '', timestamp(item.last_ts || item.last_timestamp, true)));
      body.append(top, el('span', 'preview', model.view === 'contacts' ? username(item) : item.summary || item.last_message || username(item)));
      const tags = el('span', 'tags'); tagNames(item).forEach((name) => tags.append(el('span', 'tag', name))); body.append(tags);
      node.append(avatar, body); fragment.append(node);
    }
    target.append(fragment);
  }
  function selectPerson(item) {
    closeImages();
    if (username(model.selected || {}) !== username(item)) resetInlineImages();
    model.selected = item; model.offset = 0; model.messages = []; model.total = null;
    $('chat-title').textContent = displayName(item); $('chat-subtitle').textContent = [username(item), ...tagNames(item)].join(' · ');
    $('export-chat').disabled = !model.online || !availableTasks.some((x) => x.kind === 'export_all' && x.enabled !== false); $('refresh-history').disabled = false;
    document.body.classList.add('chat-open'); $('sidebar').classList.remove('open'); $('sidebar-toggle').setAttribute('aria-expanded', 'false');
    updateImageButton();
    renderDirectory(); loadHistory();
  }
  function clearSelected() {
    closeImages(); resetInlineImages(); historyController?.abort(); ++model.historySeq;
    model.selected = null; model.offset = 0; model.messages = []; model.total = null; model.hasMore = false;
    $('chat-title').textContent = '消息记录'; $('chat-subtitle').textContent = '未选择会话';
    $('export-chat').disabled = true; $('refresh-history').disabled = true;
    $('messages').setAttribute('aria-busy', 'false'); notice('history-error');
    updateImageButton(); renderMessages(); renderDirectory();
  }
  function messageType(item) {
    const type = String(item.type ?? item.type_name ?? item.msg_type ?? item.local_type ?? 'text').toLowerCase();
    return ({ '1': 'text', '3': 'image', '34': 'voice', '43': 'video', '49': 'file', '文字': 'text', '文本': 'text', '图片': 'image', '语音': 'voice', '视频': 'video', '文件': 'file', audio: 'voice' })[type] || (['text', 'image', 'voice', 'video', 'file'].includes(type) ? type : 'other');
  }
  function messageText(item) { const content = item.display_content || item.content || item.text || item.message || ''; return typeof content === 'string' ? content : JSON.stringify(content, null, 2); }
  function externalUrl(value) {
    if (typeof value !== 'string' || value.length > 8192) return null;
    try { const url = new URL(value); return ['http:', 'https:'].includes(url.protocol) && !url.username && !url.password ? url.href : null; } catch { return null; }
  }
  function linkOrText(title, url) {
    const href = externalUrl(url), node = el(href ? 'a' : 'span', '', title);
    if (href) { node.href = href; node.target = '_blank'; node.rel = 'noopener noreferrer'; }
    return node;
  }
  function renderRich(rich) {
    if (!rich || typeof rich !== 'object' || Array.isArray(rich)) return null;
    const text = key => typeof rich[key] === 'string' ? rich[key] : '';
    const target = el('div', 'message-content'), title = text('title');
    const detail = value => { if (value) target.append(el('p', 'muted', value)); };
    switch (rich.type) {
      case 'link': case 'miniapp':
        if (!title && !text('des') && !externalUrl(rich.url)) return null;
        target.append(linkOrText(title || text('des') || rich.url, rich.url)); detail(text('des') === title ? '' : text('des')); detail(text('source')); break;
      case 'file':
        if (!title) return null;
        target.append(el('strong', '', title)); detail([text('file_ext'), Number.isSafeInteger(rich.file_size) && rich.file_size >= 0 ? `${rich.file_size.toLocaleString('zh-CN')} 字节` : ''].filter(Boolean).join(' · ')); break;
      case 'quote':
        if (!title && !text('ref_name') && !text('ref_content')) return null;
        if (title) target.append(el('p', '', title));
        target.append(el('blockquote', '', [text('ref_name'), text('ref_content')].filter(Boolean).join('：'))); break;
      case 'chatlog': {
        const items = (Array.isArray(rich.items) ? rich.items : []).slice(0, 20).filter(item => item && (typeof item.name === 'string' || typeof item.text === 'string'));
        if (!title && !text('des') && !items.length) return null;
        if (title) target.append(el('strong', '', title)); detail(text('des'));
        const list = el('ul'); for (const item of items) list.append(el('li', '', [typeof item.name === 'string' ? item.name : '', typeof item.text === 'string' ? item.text : ''].filter(Boolean).join('：'))); target.append(list); break;
      }
      case 'transfer':
        if (!title && !text('fee_desc') && !text('pay_memo')) return null;
        target.append(el('strong', '', title || text('fee_desc'))); detail([text('direction'), text('fee_desc') === title ? '' : text('fee_desc')].filter(Boolean).join(' · ')); detail(text('pay_memo')); break;
      case 'voice': case 'video':
        if (!Number.isFinite(rich.duration) || rich.duration < 0) return null;
        target.append(el('span', '', `${rich.type === 'voice' ? '语音' : '视频'} · ${Math.round(rich.duration * 10) / 10} 秒`)); break;
      case 'channels':
        if (!title) return null;
        target.append(el('span', '', title)); break;
      case 'emoji': {
        const url = externalUrl(rich.emoji_url);
        if (!url) return null;
        // 表情地址仅供用户主动打开；不自动向 CDN 发送请求。
        target.append(linkOrText('查看表情', url)); break;
      }
      default: return null;
    }
    return target;
  }
  function inlineBinding(message) {
    const image = message.image, chat = username(model.selected || {}), source = message.source;
    const timestamp = message.create_time ?? message.timestamp, localId = message.local_id;
    if (model.source !== 'wechat' || !chat || !image || image.source !== source || typeof source !== 'string' || source.length > 64 || !/^message\/message_\d+\.db$/.test(source)) return null;
    if (!Number.isSafeInteger(localId) || localId <= 0 || !Number.isSafeInteger(timestamp) || timestamp <= 0 || (message.username !== undefined && message.username !== chat)) return null;
    if (typeof image.attachment_id !== 'string' || image.attachment_id.length > 4096 || !/^[A-Za-z0-9_-]+$/.test(image.attachment_id)) return null;
    try {
      const url = new URL(image.decode_url, location.origin);
      if (url.origin !== location.origin || url.pathname !== `/api/images/${image.attachment_id}/decode` || url.hash || [...url.searchParams.keys()].length !== 1 || url.searchParams.get('source') !== source) return null;
      const encoded = image.attachment_id.replace(/-/g, '+').replace(/_/g, '/');
      const identity = JSON.parse(new TextDecoder().decode(Uint8Array.from(atob(encoded), character => character.charCodeAt(0))));
      if (identity.v !== 1 || identity.kind !== 'image' || identity.chat !== chat || identity.local_id !== localId || identity.create_time !== timestamp || (identity.db !== undefined && identity.db !== null)) return null;
      return { key: JSON.stringify([chat, source, localId, timestamp, image.attachment_id]), url: url.pathname + url.search, chat };
    } catch { return null; }
  }
  function validInline(entry) { return inlineImages.get(entry.key) === entry && entry.epoch === model.epoch && entry.generation === inlineGeneration && model.source === 'wechat' && entry.chat === username(model.selected || {}); }
  function releaseInline(entry) {
    entry.controller?.abort(); clearTimeout(entry.retryTimer);
    if (entry.blobUrl) { URL.revokeObjectURL(entry.blobUrl); inlineBytes -= entry.bytes; entry.blobUrl = null; entry.bytes = 0; }
  }
  function resetInlineImages() {
    ++inlineGeneration; inlineQueue.length = 0;
    clearTimeout(inlinePumpTimer); inlineRetryAt = 0;
    for (const entry of inlineImages.values()) releaseInline(entry); inlineImages.clear(); inlineBytes = 0;
    if ($('inline-image-dialog')?.open) $('inline-image-dialog').close();
  }
  function initInlinePreview() {
    const dialog = el('dialog', 'wide-dialog inline-preview'); dialog.id = 'inline-image-dialog'; dialog.setAttribute('aria-labelledby', 'inline-image-title');
    const header = el('header', 'dialog-head'), title = el('h2', '', '聊天图片'); title.id = 'inline-image-title';
    const close = button('', () => dialog.close(), 'icon-button'); close.title = '关闭图片'; close.setAttribute('aria-label', close.title); close.append(icon('x')); header.append(title, close);
    const body = el('div', 'dialog-body'); body.id = 'inline-image-body'; dialog.append(header, body); document.body.append(dialog);
    dialog.addEventListener('close', () => body.replaceChildren());
    dialog.addEventListener('click', event => { const rect = dialog.getBoundingClientRect(); if (event.target === dialog && (event.clientX < rect.left || event.clientX > rect.right || event.clientY < rect.top || event.clientY > rect.bottom)) dialog.close(); });
  }
  function paintInline(entry) {
    for (const node of entry.nodes) {
      node.replaceChildren();
      if (entry.blobUrl) {
        const image = el('img'); image.src = entry.blobUrl; image.alt = '聊天图片';
        const open = button('', () => {
          if (!validInline(entry) || !entry.blobUrl) return;
          const preview = el('img'); preview.src = entry.blobUrl; preview.alt = '聊天图片'; $('inline-image-body').replaceChildren(preview); $('inline-image-dialog').showModal();
        }, 'inline-image-open'); open.title = '查看图片'; open.setAttribute('aria-label', open.title); open.append(image); node.append(open);
        image.onerror = () => { if (!validInline(entry)) return; releaseInline(entry); entry.status = 'error'; entry.error = '图片数据无法显示'; paintInline(entry); };
      } else {
        node.append(el('p', 'muted', entry.status === 'error' ? entry.error : '正在解码图片…'));
        if (entry.status === 'error') node.append(button('重试图片', () => { if (!validInline(entry)) return; entry.attempts = 0; entry.status = 'pending'; paintInline(entry); inlineQueue.push(entry); pumpInline(); }));
      }
    }
  }
  async function decodeInline(entry) {
    entry.controller = new AbortController(); entry.attempts++;
    try {
      const blob = await request(entry.url, { method: 'POST', image: true, signal: entry.controller.signal });
      if (!validInline(entry)) return;
      if (inlineBytes + blob.size > 64 * 1024 * 1024) throw new Error('本页图片达到内存上限，请缩小每页条数后重试');
      entry.blobUrl = URL.createObjectURL(blob); entry.bytes = blob.size; inlineBytes += blob.size; entry.status = 'ready'; paintInline(entry);
    } catch (error) {
      if (!validInline(entry) || error.name === 'AbortError') return;
      const retryLimit = error.status === 429 ? 6 : 3;
      // 已完成但失败的解码交给用户重试；缺密钥等条件不变时，自动重试只会阻塞整页队列。
      if (entry.attempts < retryLimit && [429, 503].includes(error.status) && error.code !== 'decode_failed') {
        const delay = Math.min(8000, 1000 * (2 ** (entry.attempts - 1)));
        // 后台解码槽位由整页共用；繁忙时暂停队列，避免后续图片逐张撞上同一限制。
        inlineRetryAt = Math.max(inlineRetryAt, Date.now() + delay);
        entry.retryTimer = setTimeout(() => { if (validInline(entry)) { inlineQueue.push(entry); pumpInline(); } }, delay);
      } else { entry.status = 'error'; entry.error = errorText(error); paintInline(entry); }
    }
  }
  function pumpInline() {
    clearTimeout(inlinePumpTimer);
    if (Date.now() < inlineRetryAt) {
      inlinePumpTimer = setTimeout(pumpInline, inlineRetryAt - Date.now());
      return;
    }
    while (inlineActive < 2 && inlineQueue.length) {
      const entry = inlineQueue.shift(); if (!validInline(entry) || entry.status !== 'pending') continue;
      inlineActive++; decodeInline(entry).finally(() => { inlineActive--; pumpInline(); });
    }
  }
  function renderInline(message) {
    const node = el('div', 'inline-image'), binding = inlineBinding(message);
    if (!binding) { node.append(el('p', 'muted', '图片尚无可用的精确消息身份')); return node; }
    let entry = inlineImages.get(binding.key);
    if (!entry) {
      entry = { ...binding, epoch: model.epoch, generation: inlineGeneration, nodes: [], status: 'pending', attempts: 0, bytes: 0 };
      inlineImages.set(entry.key, entry); inlineQueue.push(entry);
    }
    entry.nodes.push(node); paintInline(entry); return node;
  }
  function renderMessages() {
    const query = $('message-search').value.toLocaleLowerCase(), type = $('message-type').value;
    const messages = model.messages.filter((item) => (!query || [messageText(item), item.sender_name, item.sender].join(' ').toLocaleLowerCase().includes(query)) && (!type || messageType(item) === type));
    const target = $('messages'); target.replaceChildren();
    // 本地筛选只隐藏消息，不取消当前页的解码；否则恢复筛选会重复占用后台槽位。
    const wantedImages = new Set();
    for (const item of model.messages) {
      const binding = messageType(item) === 'image' ? inlineBinding(item) : null;
      if (binding) wantedImages.add(binding.key);
    }
    for (const entry of inlineImages.values()) entry.nodes = [];
    if (!messages.length) empty(target, model.selected ? query || type ? '本页没有符合条件的消息' : '该会话暂无消息' : '选择会话查看消息记录');
    const fragment = document.createDocumentFragment();
    messages.forEach((item) => {
      const node = el('article', `message${item.is_self || item.is_send ? ' self' : ''}`), meta = el('div', 'message-meta');
      meta.append(el('span', 'message-sender', item.sender_name || item.sender_display_name || item.sender || '未知发送人'), el('time', '', timestamp(item.timestamp || item.create_time || item.send_time || item.time)));
      const kind = messageType(item), kindLabels = { text: '文字', image: '图片', voice: '语音', video: '视频', file: '文件', other: '其他' };
      const kindLabel = kind === 'other' ? String(item.type_name || item.type || item.msg_type || item.local_type || kindLabels.other) : kindLabels[kind];
      if (kind !== 'text') meta.append(el('span', 'message-type', kindLabel));
      node.append(meta, renderRich(item.rich) || el('div', 'message-content', messageText(item) || `[${kindLabel}]`));
      if (kind === 'image' && model.source === 'wechat') node.append(renderInline(item));
      if (model.source === 'wechat' && Number.isSafeInteger(Number(item.local_id)) && Number(item.local_id) > 0 && Number(item.create_time ?? item.timestamp) > 0) {
        const detail = button('', () => openMessageDetail(item), 'icon-button');
        detail.title = '结构化消息详情'; detail.setAttribute('aria-label', '结构化消息详情'); detail.append(icon('list-checks')); meta.append(detail);
      }
      fragment.append(node);
    });
    target.append(fragment);
    for (const [key, entry] of inlineImages) { if (!wantedImages.has(key)) { releaseInline(entry); inlineImages.delete(key); if ($('inline-image-dialog')?.open) $('inline-image-dialog').close(); } }
    pumpInline();
    $('message-count').textContent = model.selected ? `本页 ${messages.length} / ${model.messages.length} 条${model.total !== null ? ` · 共 ${model.total} 条` : ''}` : '暂无记录';
    $('page-label').textContent = String(Math.floor(model.offset / Number($('page-size').value)) + 1);
    $('previous-page').disabled = !model.selected || model.offset === 0; $('next-page').disabled = !model.selected || !model.hasMore;
  }
  async function loadHistory(quiet = false) {
    if (!model.selected) return;
    historyController?.abort(); historyController = new AbortController();
    const seq = ++model.historySeq, epoch = model.epoch, limit = Number($('page-size').value);
    const offset = model.offset, selected = username(model.selected), oldScroll = $('messages').scrollTop;
    $('messages').setAttribute('aria-busy', 'true'); $('previous-page').disabled = true; $('next-page').disabled = true;
    if (!quiet) empty($('messages'), '正在加载消息…'); notice('history-error');
    try {
      // 已落地的原生 Filter 使用 chat 字段；不把令牌放入查询参数。
      const params = new URLSearchParams({ chat: selected, source: model.source, limit: String(limit), offset: String(offset) });
      if (model.source === 'wechat') {
        for (const key of ['since', 'until']) { const value = $(`history-${key}`).value; if (value) params.set(key, String(Math.floor(new Date(value).getTime() / 1000))); }
        if (params.has('since') && params.has('until') && Number(params.get('since')) > Number(params.get('until'))) throw new Error('开始时间不能晚于结束时间');
        const types = Array.from($('history-types').selectedOptions, option => option.value).filter(Boolean);
        if (types.length) params.set('msg_types', types.join(','));
        params.set('oldest_first', String($('history-oldest').checked));
      }
      const data = await request(`/api/history?${params}`, { signal: historyController.signal });
      if (seq !== model.historySeq || epoch !== model.epoch) return;
      model.messages = rows(data, 'messages'); model.total = Number.isFinite(data?.total) ? data.total : null;
      model.hasMore = typeof data?.has_more === 'boolean' ? data.has_more : model.total !== null ? offset + model.messages.length < model.total : model.messages.length === limit;
      renderMessages(); $('messages').scrollTop = quiet ? oldScroll : 0;
    } catch (error) {
      if (error.name === 'AbortError' || seq !== model.historySeq || epoch !== model.epoch) return;
      notice('history-error', errorText(error)); if (!quiet) empty($('messages'), '消息加载失败');
      $('previous-page').disabled = offset === 0;
    } finally { if (seq === model.historySeq) $('messages').setAttribute('aria-busy', 'false'); }
  }
  const querySpecs = {
    search: { fields: ['keyword!', 'chats', 'since', 'until', 'msg_type', 'limit'], rows: 'results', columns: ['chat', 'sender', 'time', 'content'] },
    unread: { fields: ['filter', 'limit'], rows: 'sessions', columns: ['chat', 'username', 'unread', 'summary'] },
    members: { fields: ['chat!'], rows: 'members', columns: ['display', 'username', 'group_nickname', 'is_owner'] },
    stats: { fields: ['chat!', 'since', 'until'], rows: null },
    favorites: { fields: ['query', 'fav_type', 'limit'], rows: 'items', columns: ['type', 'time', 'from', 'preview'] },
    articles: { fields: ['account', 'since', 'until', 'unread', 'limit'], rows: 'articles', columns: ['account', 'time', 'title', 'digest'] },
    'sns-feed': { fields: ['user', 'since', 'until', 'limit'], rows: 'posts', columns: ['author', 'time', 'content', 'media_count'] },
    'sns-search': { fields: ['keyword!', 'user', 'since', 'until', 'limit'], rows: 'posts', columns: ['author', 'time', 'content', 'media_count'] },
    'sns-notifications': { fields: ['since', 'until', 'include_read', 'limit'], rows: 'notifications', columns: ['from_nickname', 'type', 'time', 'content', 'feed_preview'] },
    'voice-messages': { fields: ['chat!', 'since', 'until', 'limit'], rows: 'voices', columns: ['create_time', 'voice_data_bytes'] }
  };
  const queryLabels = { keyword: '关键词', chats: '会话（逗号分隔）', chat: '会话账号', since: '开始时间', until: '结束时间', msg_type: '消息类型', limit: '数量上限', filter: '会话类型', query: '收藏关键词', fav_type: '收藏类型', account: '公众号', unread: '仅未读公众号', user: '作者', include_read: '包含已读互动', display: '名称', name: '名称', username: '账号', unread_count: '未读数', summary: '摘要', group_nickname: '群昵称', is_owner: '群主', sender: '发送人', time: '时间', content: '内容', type: '类型', from: '来源', preview: '摘要', title: '标题', digest: '摘要', author: '作者', media_count: '媒体数量', from_nickname: '互动人', feed_preview: '原帖摘要', create_time: '时间戳', voice_size: '字节数', total: '消息总数', count: '数量', hour: '小时' };
  let queryOffset = 0, queryController, querySequence = 0, queryLastParams;
  queryLabels.voice_data_bytes = '字节数';
  queryLabels.unread = '未读数';
  Object.assign(queryLabels, { refer: '引用内容', refer_chatusr: '引用会话', refer_createtime: '引用时间', refer_displayname: '引用发送人', refer_fromusr: '来源账号', refer_sender: '发送账号', refer_summary: '引用摘要', refer_svrid: '原消息标识', refer_type_label: '消息类型', reply_text: '回复正文', metadata: '附件信息', reference: '关联结果', identity: '消息身份', status: '状态', binding: '关联依据', size: '字节数', equivalent_copies: '相同副本', warning: '提示', description: '描述', extension: '扩展名', item_count: '条目数量', item_index: '条目序号', kind: '内容类型', expected_size: '声明字节数' });
  function queryFields() {
    queryController?.abort(); ++querySequence; queryOffset = 0; queryLastParams = null;
    const kind = $('query-kind').value, target = $('query-fields'); target.replaceChildren();
    for (const descriptor of querySpecs[kind].fields) {
      const key = descriptor.replace('!', ''), label = el('label', '', key === 'unread' ? '仅未读公众号' : queryLabels[key]);
      let input;
      const choices = key === 'msg_type' ? ['', 'text', 'image', 'voice', 'video', 'sticker', 'location', 'link', 'call', 'system'] : key === 'filter' ? ['', 'private', 'group', 'official', 'folded'] : key === 'fav_type' ? ['', '1', '2', '5', '19', '20'] : null;
      if (choices) {
        input = el('select');
        const labels = { '': '全部', text: '文字', image: '图片', voice: '语音', video: '视频', sticker: '表情', location: '位置', link: '链接', file: '文件', call: '通话事件', system: '系统', private: '单聊', group: '群聊', official: '公众号', folded: '折叠会话', 1: '文本', 2: '图片', 5: '文章', 19: '名片', 20: '视频' };
        labels.link = '应用消息（含链接、文件）';
        choices.forEach(value => { const option = el('option', '', labels[value]); option.value = value; input.append(option); });
      } else {
        input = el('input'); input.type = ['unread', 'include_read'].includes(key) ? 'checkbox' : ['since', 'until'].includes(key) ? 'datetime-local' : key === 'limit' ? 'number' : 'text';
      }
      input.name = key; input.id = `query-${key}`; input.required = descriptor.endsWith('!');
      if (key === 'limit') { input.min = '1'; input.max = kind === 'voice-messages' ? '500' : '2000'; input.step = '1'; input.value = '100'; }
      if (key === 'chat' && model.selected) input.value = username(model.selected);
      label.append(input); target.append(label);
    }
    $('query-results').replaceChildren(); $('query-status').textContent = '尚未查询'; notice('query-error');
    $('query-previous').disabled = true; $('query-next').disabled = true; $('query-page').textContent = '';
    $('query-submit').disabled = false;
  }
  function queryParameters() {
    const params = new URLSearchParams();
    for (const input of $('query-fields').querySelectorAll('input,select')) {
      if (input.type === 'checkbox') { params.set(input.name, String(input.checked)); continue; }
      if (!input.value.trim()) continue;
      const value = input.type === 'datetime-local' ? String(Math.floor(new Date(input.value).getTime() / 1000)) : input.value.trim();
      params.set(input.name, value);
    }
    if (params.has('since') && params.has('until') && Number(params.get('since')) > Number(params.get('until'))) throw new Error('开始时间不能晚于结束时间');
    return params;
  }
  function valueText(value) {
    if (value === null || value === undefined || value === '') return '未知';
    if (typeof value === 'boolean') return value ? '是' : '否';
    return typeof value === 'object' ? JSON.stringify(value) : String(value);
  }
  function resultTable(items, columns) {
    const table = el('table', 'query-table'), head = el('thead'), title = el('tr'), body = el('tbody');
    columns.forEach(key => title.append(el('th', '', queryLabels[key] || key))); head.append(title);
    for (const item of items) { const row = el('tr'); columns.forEach(key => row.append(el('td', '', valueText(item[key])))); body.append(row); }
    table.append(head, body); return table;
  }
  function resultEvidence(data) {
    const details = el('details'), summary = el('summary', '', '完整字段与来源证据');
    details.append(summary, el('pre', '', JSON.stringify(data, null, 2))); return details;
  }
  function detailFields(data, depth = 0) {
    const list = el('dl', 'key-values');
    for (const [key, value] of Object.entries(data)) {
      if (['source', 'local_id', 'local_type', 'exit_code', 'refer_type', 'datatype', 'path', 'md5', 'expected_md5', 'raw_content', 'raw_xml'].includes(key) || (depth === 0 && key === 'text')) continue;
      const term = el('dt', '', queryLabels[key] || key), definition = el('dd');
      if (value && typeof value === 'object' && depth < 3) definition.append(detailFields(value, depth + 1));
      else definition.textContent = valueText(value);
      list.append(term, definition);
    }
    return list;
  }
  async function runQuery(paging = false) {
    if (!$('query-form').reportValidity()) return;
    const kind = $('query-kind').value, spec = querySpecs[kind];
    let params;
    try { params = paging && queryLastParams ? new URLSearchParams(queryLastParams) : queryParameters(); } catch (error) { notice('query-error', errorText(error)); return; }
    if (!paging) queryOffset = 0;
    queryLastParams = params.toString(); if (kind === 'voice-messages') params.set('offset', String(queryOffset));
    queryController?.abort(); queryController = new AbortController(); const seq = ++querySequence;
    $('query-submit').disabled = true; $('query-previous').disabled = true; $('query-next').disabled = true;
    $('query-results').replaceChildren(); $('query-status').textContent = '查询中…'; notice('query-error');
    try {
      const data = await request(`/api/${kind}?${params}`, { signal: queryController.signal });
      if (seq !== querySequence) return;
      const target = $('query-results'), items = spec.rows ? rows(data, spec.rows) : [];
      if (kind === 'stats') {
        target.append(el('h3', '', `消息总数：${valueText(data.total)}`));
        for (const [key, title, columns] of [['by_type', '类型分布', ['type', 'count']], ['by_hour', '小时分布', ['hour', 'count']], ['top_senders', '发言排行', ['sender', 'count']]]) {
          target.append(el('h3', '', title), resultTable(rows(data, key), columns));
        }
      } else if (items.length) target.append(resultTable(items, spec.columns)); else empty(target, '没有符合条件的记录');
      const partial = data.partial || data.meta?.partial || data.membership_complete === false || data.meta?.unreadable > 0 || data.meta?.author_conflicts > 0;
      const more = data.has_more || data.meta?.has_more || data.meta?.scan_truncated || data.source_unfinished;
      $('query-status').textContent = `${spec.rows ? `${items.length} 条` : '统计完成'}${partial ? ' · 部分结果' : ''}${more ? ' · 结果未结束' : ''}${kind.startsWith('sns-') ? ' · 本地缓存' : ''}${data.meta?.unreadable ? ` · ${data.meta.unreadable} 条解析失败` : ''}${data.meta?.author_conflicts ? ` · ${data.meta.author_conflicts} 条作者冲突` : ''}`;
      target.append(resultEvidence(data));
      if (kind === 'voice-messages') {
        const limit = Number(params.get('limit'));
        $('query-previous').disabled = queryOffset === 0; $('query-next').disabled = items.length < limit;
        $('query-page').textContent = `第 ${Math.floor(queryOffset / limit) + 1} 页${items.length === limit ? ' · 可能还有记录' : ''}`;
      }
    } catch (error) { if (seq !== querySequence || error.name === 'AbortError') return; notice('query-error', errorText(error)); $('query-status').textContent = '查询失败'; }
    finally { if (seq === querySequence) $('query-submit').disabled = false; }
  }
  let detailMessage, detailController, detailSequence = 0;
  function openMessageDetail(item) {
    detailMessage = { chat: username(model.selected), local_id: item.local_id, create_time: item.create_time ?? item.timestamp };
    $('message-detail-result').replaceChildren(); notice('message-detail-error'); $('message-detail-dialog').showModal();
  }
  async function loadMessageDetail(event) {
    event.preventDefault(); if (!detailMessage) return;
    const params = new URLSearchParams(detailMessage), kind = $('message-detail-kind').value;
    if (kind === 'record-item') params.set('item_index', $('message-item-index').value);
    detailController?.abort(); detailController = new AbortController(); const seq = ++detailSequence;
    notice('message-detail-error'); empty($('message-detail-result'), '查询中…');
    try {
      const data = await request(`/api/decode-${kind}?${params}`, { signal: detailController.signal });
      if (seq !== detailSequence) return;
      const target = $('message-detail-result'); target.replaceChildren();
      target.append(detailFields(data), resultEvidence(data));
    } catch (error) { if (seq !== detailSequence || error.name === 'AbortError') return; $('message-detail-result').replaceChildren(); notice('message-detail-error', errorText(error)); }
  }
  function descriptors(state) {
    const advertised = state.task_kinds || state.available_tasks || state.capabilities?.tasks || (Array.isArray(state.capabilities) ? state.capabilities : null);
    if (!advertised) return catalog.filter(spec => !spec.advertisedOnly);
    const list = Array.isArray(advertised) ? advertised : Object.entries(advertised).map(([kind, spec]) => ({ kind, ...(typeof spec === 'object' ? spec : { name: spec }) }));
    return list.map((value) => {
      const spec = typeof value === 'string' ? { kind: value } : value;
      return { ...(catalog.find((item) => item.kind === spec.kind) || { group: '其他任务', icon: 'play' }), ...spec };
    }).filter((item) => typeof item.kind === 'string');
  }
  function renderTools() {
    const target = $('task-tools'); target.replaceChildren(); let group = '';
    availableTasks.forEach((spec) => {
      if (spec.contextOnly) return;
      if (group !== spec.group) { group = spec.group; target.append(el('p', 'tool-group', group || '任务')); }
      const node = button('', () => { if (model.online && spec.enabled !== false) openTask(spec); }, 'tool-button'); node.append(icon(spec.icon), el('span', '', spec.name || spec.kind));
      // 能力禁用仍可聚焦，从原生 tooltip 读取不可用原因。
      const disabled = !model.online || spec.enabled === false; node.setAttribute('aria-disabled', String(disabled)); node.classList.toggle('tool-disabled', disabled); if (spec.reason) node.title = spec.reason; target.append(node);
    });
  }
  function renderState() {
    const state = model.state, account = state.account || state.current_account || {};
    $('account-summary').textContent = typeof account === 'string' ? account : account.name || account.username || account.wxid || state.account_id || state.runtime_id || '账号未提供';
    const target = $('account-state'); target.replaceChildren();
    const fields = [['账号/运行实例', $('account-summary').textContent], ['账号状态', account.status || state.account_status || '未提供'], ['工作目录', state.workspace || state.data_dir || '未提供'], ['版本', state.version || (state.api_version ? `API ${state.api_version} · ${state.engine || ''}` : '未提供')], ['任务历史', state.history_persisted === undefined ? '未提供' : state.history_persisted ? '已持久化' : '未持久化'], ['服务地址', location.origin], ['认证状态', token ? '已设置访问令牌' : '未设置访问令牌']];
    fields.forEach(([key, value]) => target.append(el('dt', '', key), el('dd', '', value)));
    $('auth-state').textContent = token ? '访问令牌已保存在当前标签页' : '尚未设置访问令牌';
    availableTasks = descriptors(state); renderTools();
    $('export-chat').disabled = !model.online || !model.selected || !availableTasks.some((x) => x.kind === 'export_all' && x.enabled !== false);
    updateImageButton();
    loadNotificationSettings();
  }
  function progressNode(task) {
    const raw = task.progress, progress = el('progress'); progress.max = 100;
    const percent = typeof raw === 'number' ? raw : Number.isFinite(raw?.percent) ? raw.percent : Number.isFinite(raw?.total) && raw.total > 0 ? 100 * Number(raw.current ?? raw.completed ?? 0) / raw.total : null;
    if (percent !== null && Number.isFinite(percent)) progress.value = Math.max(0, Math.min(100, percent));
    progress.setAttribute('aria-label', `${taskName(task)}进度`); return progress;
  }
  function renderQueue() {
    const filter = $('queue-filter').value;
    const list = model.tasks.filter((task) => filter === 'all' || (filter === 'active' ? activeStates.has(statusOf(task)) : filter === 'failed' ? statusOf(task) === 'failed' : !activeStates.has(statusOf(task))));
    $('queue-count').textContent = String(model.tasks.filter((task) => activeStates.has(statusOf(task))).length);
    const target = $('queue'); target.replaceChildren();
    if (!list.length) { empty(target, model.online ? '暂无任务' : '尚未加载任务'); return; }
    list.forEach((task) => {
      const node = el('article', 'task-row'), state = statusOf(task), open = button('', () => openDetail(taskId(task)));
      open.append(el('span', 'task-name', taskName(task)), el('span', `task-state ${Object.hasOwn(labels, state) ? state : ''}`, labels[state] || state));
      node.append(open); if (activeStates.has(state)) node.append(progressNode(task));
      node.append(el('p', 'task-note', redact(task.stage || task.message || timestamp(task.created_at || task.started_at)))); target.append(node);
    });
  }
  function applyState(state) {
    const key = JSON.stringify(state.runtime_id || state.account_id || state.account || state.current_account || null);
    const changed = model.accountKey !== undefined && model.accountKey !== key;
    if (changed) { stopEvents(); ++model.epoch; clearRecords(); }
    model.accountKey = key; model.state = state || {}; model.online = true; renderState();
    if (changed) { if (refreshPending) refreshQueued = true; else refreshAll(); }
    return !changed;
  }
  async function loadDirectories(revealSelection = true) {
    directoryController?.abort(); directoryController = new AbortController();
    const signal = directoryController.signal;
    const epoch = model.epoch, source = model.source, seq = ++directorySeq, revision = directoryRevision;
    const current = () => epoch === model.epoch && source === model.source && seq === directorySeq;
    const values = {}, errors = {};
    for (const key of ['contacts', 'sessions']) {
      try { values[key] = rows(await request(`/api/${key}?${new URLSearchParams({ source, limit: '2000' })}`, { signal }), key); }
      catch (error) { if (!current()) return; values[key] = []; errors[key] = errorText(error); }
      if (!current()) return;
    }
    model.contacts = values.contacts; model.sessions = values.sessions; model.directoryError = errors;
    const wasChatOpen = document.body.classList.contains('chat-open');
    directoriesDirty = Object.keys(errors).length > 0 || revision !== directoryRevision;
    renderTags(); renderDirectory();
    if (model.selected) {
      const id = username(model.selected);
      const replacement = model.sessions.find((item) => username(item) === id)
        || model.contacts.find((item) => username(item) === id);
      if (replacement) { model.selected = replacement; $('chat-title').textContent = displayName(replacement); loadHistory(true); }
      else clearSelected();
    } else {
      const first = model.sessions.find(item => item.chat_type !== 'folded'
        && !['brandsessionholder', '@placeholder_foldgroup'].includes(username(item)));
      if (first) selectPerson(first);
    }
    if (!revealSelection && !wasChatOpen) document.body.classList.remove('chat-open');
    renderState();
  }
  async function loadTasks(refreshDirectory = true) {
    const epoch = model.epoch, seq = ++model.tasksSeq;
    const data = await request('/api/tasks'); if (epoch !== model.epoch || seq !== model.tasksSeq) return;
    const previous = new Map(model.tasks.map((task) => [taskId(task), statusOf(task)]));
    model.tasks = rows(data, 'tasks'); renderQueue();
    const changed = model.tasks.filter((task) => !activeStates.has(statusOf(task)) && previous.get(taskId(task)) !== statusOf(task));
    capabilitiesDirty = capabilitiesDirty || changed.length > 0;
    if (changed.some((task) => ['wechat_decrypt', 'wechat_keys'].includes(task.kind))) {
      directoriesDirty = true; ++directoryRevision;
    }
    if (capabilitiesDirty) {
      const state = await request('/api/state');
      if (epoch !== model.epoch || seq !== model.tasksSeq) return;
      if (!applyState(state || {})) return;
      capabilitiesDirty = false;
    }
    if (refreshDirectory && directoriesDirty) await loadDirectories(false);
  }
  async function refreshAll() {
    if (refreshPending) { refreshQueued = true; return; }
    refreshPending = true; const epoch = model.epoch; $('refresh-all').disabled = true;
    notice('global-error'); connected('正在同步');
    try {
      const state = await request('/api/state'); if (epoch !== model.epoch) return;
      if (!applyState(state || {})) return;
      const errors = [];
      try { await loadTasks(false); } catch (error) { errors.push(`任务：${errorText(error)}`); }
      if (epoch !== model.epoch) return;
      await loadDirectories(); if (epoch !== model.epoch) return;
      for (const [key, error] of Object.entries(model.directoryError)) errors.push(`${key === 'contacts' ? '联系人' : '会话'}：${error}`);
      if (model.source === 'wechat') {
        try { const data = await request('/api/tags'); if (epoch !== model.epoch) return; model.tags = rows(data, 'tags'); }
        catch (error) { if (epoch !== model.epoch) return; model.tags = []; errors.push(`标签：${errorText(error)}`); }
      } else model.tags = [];
      if (epoch !== model.epoch) return;
      renderTags(); renderDirectory();
      $('updated-at').textContent = `更新于 ${new Date().toLocaleTimeString('zh-CN', { hour12: false })}`;
      notice('global-error', errors.join('；'));
      if (model.online) { connected(errors.length ? '部分加载失败' : '已连接', errors.length ? 'failed' : 'online'); if (model.live && !streamController) startEvents(); }
    } catch (error) {
      if (epoch === model.epoch && error.name !== 'AbortError') { model.online = false; connected('连接失败', 'failed'); notice('global-error', errorText(error)); renderTools(); }
    } finally { refreshPending = false; $('refresh-all').disabled = false; if (refreshQueued) { refreshQueued = false; refreshAll(); } }
  }
  function field(spec, container) {
    // 只接受明确类型的字段，不提供命令、参数字符串或 JSON 编辑器。
    if (!spec || !/^[a-z][a-z0-9_]*$/i.test(spec.name || '') || /^(command|cmd|shell|args|argv|script|__proto__|constructor|prototype)$/i.test(spec.name)) return false;
    const type = spec.type || 'string';
    if (!['string', 'path', 'integer', 'number', 'boolean', 'enum'].includes(type)) return false;
    const label = el('label', type === 'boolean' ? 'check' : 'field');
    let input;
    if (type === 'enum') { input = el('select'); (spec.choices || spec.enum || []).forEach((choice) => input.add(new Option(typeof choice === 'object' ? choice.label : choice, typeof choice === 'object' ? choice.value : choice))); }
    else { input = el('input'); input.type = type === 'boolean' ? 'checkbox' : ['integer', 'number'].includes(type) ? 'number' : 'text'; }
    input.name = spec.name; input.required = Boolean(spec.required); input.autocomplete = 'off';
    if (spec.min !== undefined) input.min = spec.min; if (spec.max !== undefined) input.max = spec.max; if (type === 'integer') input.step = '1';
    if (type === 'boolean') input.checked = Boolean(spec.default); else if (spec.default !== undefined) input.value = String(spec.default);
    if (type === 'boolean') label.append(input, el('span', '', spec.label || spec.name)); else label.append(el('span', '', spec.label || spec.name), input);
    container.append(label);
    formReaders.push((options) => { if (type === 'boolean') options[spec.name] = input.checked; else if (input.value !== '') options[spec.name] = ['integer', 'number'].includes(type) ? Number(input.value) : input.value; });
    return true;
  }
  function taskFlags(spec, target) {
    const fallback = spec.kind === 'export_all' ? ['include_sns', 'include_sns_media'] : spec.kind === 'sns_decrypt' ? ['include_sns_media'] : [];
    const allowed = new Set(spec.options || fallback);
    const flags = [
      { name: 'include_images', label: '导出聊天图片', default: true },
      { name: 'dry_run', label: '仅核对导出计划', default: false },
      { name: 'allow_missing_media', label: '允许媒体缺失', default: false },
      { name: 'include_sns', label: '导出朋友圈', default: false },
      { name: 'include_sns_media', label: '下载朋友圈媒体', default: false },
      { name: 'authorize_memory_scan', label: '允许本任务读取个人微信进程内存以提取密钥', default: false, required: spec.requires_memory_consent === true || ['image_key', 'wechat_keys'].includes(spec.kind) }
    ].filter((flag) => allowed.has(flag.name));
    if (!flags.length) return;
    const group = el('fieldset'); group.append(el('legend', '', '处理选项'));
    flags.forEach((flag) => field({ ...flag, type: 'boolean' }, group));
    const dependency = (childName, parentName) => {
      const child = group.querySelector(`[name=${childName}]`), parent = group.querySelector(`[name=${parentName}]`);
      if (!child || !parent) return;
      const update = () => { child.disabled = !parent.checked; if (child.disabled) child.checked = false; };
      parent.addEventListener('change', update); update();
    };
    dependency('include_sns_media', 'include_sns'); dependency('allow_missing_media', 'include_images');
    target.append(group);
  }
  function exportBudgets(spec, target) {
    if (spec.kind !== 'export_all' || !spec.options?.includes('max_media_bytes')) return;
    const group = el('fieldset'); group.append(el('legend', '', '聊天媒体预算'));
    const inputs = [];
    for (const [name, label, max, fallback] of [
      ['max_media_bytes', '单文件上限（字节）', 524288000, 67108864],
      ['max_total_media_bytes', '每会话总上限（字节）', 17179869184, 2147483648]
    ]) {
      const row = el('label', 'field'), input = el('input');
      input.name = name; input.type = 'number'; input.min = '1'; input.max = String(max); input.step = '1';
      input.placeholder = `默认 ${fallback}`; row.append(el('span', '', label), input); group.append(row); inputs.push(input);
    }
    const images = target.querySelector('[name=include_images]'), dry = target.querySelector('[name=dry_run]');
    const sns = target.querySelector('[name=include_sns]'), snsMedia = target.querySelector('[name=include_sns_media]');
    const update = () => {
      for (const input of inputs) input.disabled = images ? !images.checked : false;
      if (sns && dry) { sns.disabled = dry.checked; if (dry.checked) sns.checked = false; }
      if (snsMedia && sns && dry?.checked) { snsMedia.checked = false; snsMedia.disabled = true; }
    };
    images?.addEventListener('change', update); dry?.addEventListener('change', update); update();
    formReaders.push(options => {
      if (!options.dry_run) delete options.dry_run;
      for (const input of inputs) {
        if (input.disabled || input.value === '') continue;
        const value = Number(input.value);
        if (!Number.isSafeInteger(value) || value < 1 || value > Number(input.max)) throw new Error('媒体预算必须为范围内的整数字节数');
        options[input.name] = value;
      }
      if ((options.max_total_media_bytes ?? 2147483648) < (options.max_media_bytes ?? 67108864)) throw new Error('每会话总预算不能小于单文件预算');
      if (options.dry_run && options.include_sns) throw new Error('仅核对计划不能同时导出朋友圈');
    });
    target.append(group);
  }
  function historyExportRequest({ chat, since = '', until = '', limit = 500, format = 'markdown' }) {
    if (typeof chat !== 'string' || !chat.trim() || new TextEncoder().encode(chat).length > 256 || /[\u0000-\u001f\u007f-\u009f]/u.test(chat)) throw new Error('会话标识必须为 1–256 字节，且不含控制字符');
    const count = Number(limit);
    if (!Number.isSafeInteger(count) || count < 1) throw new Error('消息条数必须为正的安全整数');
    if (!['markdown', 'txt', 'json', 'yaml'].includes(format)) throw new Error('请选择有效的导出格式');
    const request = { chat, limit: count, format };
    for (const [key, raw] of [['since', since], ['until', until]]) {
      if (!raw) continue;
      const value = raw.replace('T', ' ');
      if (!/^\d{4}-\d{2}-\d{2}( \d{2}:\d{2}(:\d{2})?)?$/.test(value)) throw new Error('时间须为日期或本地日期时刻');
      request[key] = value;
    }
    const boundary = (value, end) => value.length === 10 ? `${value} ${end ? '23:59:59' : '00:00:00'}` : value.length === 16 ? `${value}:00` : value;
    if (request.since && request.until && boundary(request.since, false) > boundary(request.until, true)) throw new Error('开始时间不能晚于结束时间');
    return request;
  }
  function historyExportFields(target, selectedUser) {
    const group = el('fieldset', 'history-export-fields'); group.append(el('legend', '', '单会话历史'));
    const chatRow = el('label', 'field'), chat = el('input'), choices = el('datalist');
    choices.id = 'history-export-chats'; chat.name = 'history_chat'; chat.type = 'text'; chat.required = true; chat.autocomplete = 'off';
    chat.setAttribute('list', choices.id); chat.placeholder = 'username 或唯一会话名称';
    chat.value = selectedUser || (model.selected ? username(model.selected) : '');
    const seen = new Set();
    for (const person of [...model.sessions, ...model.contacts]) {
      const id = username(person); if (!id || seen.has(id)) continue;
      seen.add(id); const option = el('option'); option.value = id; option.label = displayName(person); choices.append(option);
    }
    chatRow.append(el('span', '', '会话 username / 名称'), chat); group.append(chatRow, choices);
    const dates = el('div', 'history-export-dates'); const inputs = {};
    for (const [key, label] of [['since', '开始时间'], ['until', '结束时间']]) {
      const row = el('div', 'history-export-date'), mode = el('select'), fieldLabel = el('label', 'field'), input = el('input');
      mode.setAttribute('aria-label', `${label}精度`); mode.add(new Option('日期', 'date')); mode.add(new Option('日期时刻', 'datetime-local'));
      input.type = 'date'; input.name = `history_${key}`;
      mode.addEventListener('change', () => {
        const old = input.value; input.type = mode.value;
        if (mode.value === 'datetime-local') { input.step = '1'; input.value = old ? `${old.slice(0, 10)}T${key === 'until' ? '23:59:59' : '00:00:00'}` : ''; }
        else { input.removeAttribute('step'); input.value = old.slice(0, 10); }
        refresh();
      });
      fieldLabel.append(el('span', '', `${label}（可选）`), input); row.append(mode, fieldLabel); dates.append(row); inputs[key] = input;
    }
    group.append(dates);
    const countRow = el('label', 'field'), count = el('input');
    count.name = 'history_limit'; count.type = 'number'; count.min = '1'; count.max = String(Number.MAX_SAFE_INTEGER); count.step = '1'; count.required = true; count.value = '500';
    countRow.append(el('span', '', '最多消息条数'), count);
    const formatRow = el('label', 'field'), format = el('select'); format.name = 'history_format';
    for (const [value, label] of [['markdown', 'Markdown (.md)'], ['txt', 'TXT (.txt)'], ['json', 'JSON (.json)'], ['yaml', 'YAML (.yaml)']]) format.add(new Option(label, value));
    formatRow.append(el('span', '', '导出格式'), format); group.append(countRow, formatRow);
    const summary = el('dl', 'key-values history-export-summary'); summary.setAttribute('aria-live', 'polite');
    const refresh = () => {
      summary.replaceChildren();
      const end = inputs.until.value;
      const facts = [['时间基准', '宿主本地时间'], ['结束边界', end ? `${end.replace('T', ' ')}${end.length === 10 ? ' 23:59:59' : ''}（含）` : '不限'], ['条数上限', count.value || '500']];
      for (const [key, value] of facts) summary.append(el('dt', '', key), el('dd', '', value));
    };
    inputs.until.addEventListener('input', refresh); count.addEventListener('input', refresh); refresh();
    group.append(summary); target.append(group);
    formReaders.push(options => { options.history_export = historyExportRequest({ chat: chat.value, since: inputs.since.value, until: inputs.until.value, limit: count.value, format: format.value }); });
  }
  function planRequest({ scope, users = '', exclude = '', size_mode = 'estimate', threads = 1, start = '', end = '' }, allowedModes) {
    const names = value => [...new Set(value.split(/\r?\n/).filter(name => name !== ''))];
    const count = Number(threads);
    if (!Number.isInteger(count) || count < 1 || count > 6) throw new Error('线程数必须为 1–6');
    if (!['all', 'selected'].includes(scope) || !allowedModes.includes(size_mode)) throw new Error('计划范围或扫描模式未获授权');
    const request = { users: scope === 'all' ? null : names(users), exclude_users: names(exclude), size_mode, threads: count };
    for (const name of [...(request.users || []), ...request.exclude_users]) {
      if (new TextEncoder().encode(name).length > 1024 || /[\u0000-\u001f\u007f-\u009f]/u.test(name)) throw new Error('username 长度或字符无效');
    }
    for (const [key, value] of [['start', start], ['end', end]]) if (value) request[key] = value.replace('T', ' ');
    return request;
  }
  function planGenerationFields(spec, target) {
    const group = el('fieldset', 'plan-fields'); group.append(el('legend', '', '当前账号计划'));
    const add = (label, input) => { const row = el('label', 'field'); row.append(el('span', '', label), input); group.append(row); return input; };
    const scope = el('select'); scope.name = 'plan_scope'; scope.add(new Option('当前账号全部会话', 'all')); scope.add(new Option('明确选择', 'selected')); add('会话范围', scope);
    const users = el('textarea'); users.name = 'plan_users'; users.rows = 3; users.value = model.selected ? username(model.selected) : ''; add('包含 username（每行一个）', users);
    const exclude = el('textarea'); exclude.name = 'plan_exclude'; exclude.rows = 2; add('排除 username（每行一个）', exclude);
    const modes = spec.size_modes?.includes('scan') ? ['estimate', 'scan'] : ['estimate'];
    const mode = el('select'); mode.name = 'plan_size_mode'; mode.add(new Option('估算', 'estimate')); if (modes.includes('scan')) mode.add(new Option('扫描', 'scan')); add('容量统计', mode);
    const threads = el('input'); threads.name = 'plan_threads'; threads.type = 'number'; threads.min = '1'; threads.max = '6'; threads.step = '1'; threads.value = '1'; threads.required = true; add('线程数', threads);
    const times = {};
    for (const [key, label] of [['start', '开始时间'], ['end', '结束时间']]) {
      const precision = el('select'); precision.setAttribute('aria-label', `计划${label}精度`);
      for (const [value, text] of [['date', '日期'], ['datetime-local', '日期时刻'], ['text', 'Unix 秒']]) precision.add(new Option(text, value));
      const input = el('input'); input.type = 'date'; input.name = `plan_${key}`;
      precision.addEventListener('change', () => { input.type = precision.value; input.value = ''; if (precision.value === 'datetime-local') input.step = '1'; refresh(); });
      group.append(precision); add(`${label}（可选）`, input); times[key] = input;
    }
    const summary = el('dl', 'key-values'); group.append(summary);
    const refresh = () => {
      users.disabled = scope.value === 'all'; summary.replaceChildren();
      const end = times.end.value;
      for (const [key, value] of [['来源', '当前账号新建快照'], ['选择范围', scope.value === 'all' ? '全部会话（扣除排除项）' : `明确选择 ${new Set(users.value.split(/\r?\n/).filter(Boolean)).size} 个`], ['时间基准', '宿主本地时间'], ['结束边界', end ? `${end.replace('T', ' ')}${end.length === 10 && end.includes('-') ? ' 00:00:00' : ''}（含）` : '不限']]) summary.append(el('dt', '', key), el('dd', '', value));
    };
    scope.addEventListener('change', refresh); users.addEventListener('input', refresh); times.end.addEventListener('input', refresh); refresh();
    target.append(group);
    formReaders.push(options => { options.chat_plan = planRequest({ scope: scope.value, users: users.value, exclude: exclude.value, size_mode: mode.value, threads: threads.value, start: times.start.value, end: times.end.value }, modes); });
  }
  function validPlanRef(ref) { return ref && ['task_id', 'artifact_id', 'sha256'].every(key => /^[a-f0-9]{64}$/.test(ref[key] || '')); }
  function planSelected(flag, mode) { return mode === 'whitelist' ? flag === '1' : flag !== '0'; }
  function planPreviewCount(base, changes, mode) {
    return base + [...changes.values()].reduce((count, change) => count + Number(planSelected(change.export, mode)) - Number(planSelected(change.original, mode)), 0);
  }
  function setPlanChange(changes, row, flag) {
    if (!['', '0', '1'].includes(flag)) throw new Error('导出标记无效');
    if (flag === row.export) changes.delete(row.username);
    else changes.set(row.username, { username: row.username, original: row.export, export: flag });
  }
  function planSizeStatus(status) {
    if (status === 'ok') return '完整';
    const labels = { media_error: '媒体读取失败', media_missing: '媒体来源缺失', message_db_missing: '消息库缺失', message_error: '消息读取失败', no_message_table: '会话消息表缺失', resource_error: '资源读取失败', resource_missing: '资源库缺失', scan_base_missing: '扫描根缺失', scan_depth_limited: '扫描深度受限', scan_error: '扫描失败', scan_limited: '扫描数量受限', scan_missing: '扫描来源缺失', scan_overflow: '扫描统计溢出', scan_reparse_skipped: '已跳过重解析点' };
    return status.startsWith('partial:') ? `部分统计：${status.slice(8).split(',').map(code => labels[code] || code).join('、')}` : status;
  }
  let planReview = { sequence: 0, ref: null, changes: new Map(), page: null, mode: 'blacklist', busy: false };
  function updatePlanActions() {
    const dirty = planReview.changes.size > 0, page = planReview.page;
    const count = page ? planPreviewCount(page.selected_count, planReview.changes, planReview.mode) : null;
    $('plan-selection-count').textContent = count === null ? '选择数未确定' : `预览选择 ${count} / ${page.total} 个 · 未保存修改 ${planReview.changes.size} 项`;
    $('plan-save-review').disabled = planReview.busy || !page || !dirty || !availableTasks.some(spec => spec.kind === 'chat_plan_review' && spec.enabled !== false);
    $('plan-apply').disabled = planReview.busy || !page || dirty || count === 0 || !availableTasks.some(spec => spec.kind === 'chat_plan_apply' && spec.enabled !== false);
    $('plan-read-mode').disabled = planReview.busy;
  }
  async function openPlanReview(ref) {
    if (!validPlanRef(ref)) return;
    if (planReview.changes.size && !window.confirm('放弃未保存的计划标记修改？')) return;
    planReview = { sequence: planReview.sequence + 1, ref: { ...ref }, changes: new Map(), page: null, mode: 'blacklist', busy: false };
    $('plan-read-mode').value = 'blacklist'; $('plan-apply-dry').checked = false;
    $('plan-review-dialog').showModal(); await readPlanPage(0);
  }
  async function readPlanPage(offset) {
    const epoch = model.epoch, sequence = ++planReview.sequence, ref = planReview.ref, mode = planReview.mode;
    if (!ref) return;
    planReview.busy = true; planReview.page = null; updatePlanActions(); notice('plan-error'); $('plan-rows').replaceChildren();
    $('plan-meta').replaceChildren(); $('plan-page').textContent = ''; $('plan-previous').disabled = true; $('plan-next').disabled = true;
    const current = () => epoch === model.epoch && sequence === planReview.sequence && $('plan-review-dialog').open;
    try {
      const page = await request(`/api/tasks/${ref.task_id}/artifacts/${ref.artifact_id}/plan?${new URLSearchParams({ sha256: ref.sha256, plan_mode: mode, offset, limit: 50 })}`);
      if (!current()) return;
      if (page.version !== 1 || !validPlanRef(page.plan_ref) || !['task_id', 'artifact_id', 'sha256'].every(key => page.plan_ref[key] === ref[key]) || page.plan_mode !== mode || page.source_kind !== 'runtime_snapshot') throw new Error('计划引用或来源不符');
      planReview.page = page;
      const meta = $('plan-meta'); meta.replaceChildren();
      const time = value => value === null ? '不限' : new Date(value * 1000).toISOString().replace('T', ' ').replace('.000Z', ' UTC');
      for (const [key, value] of [['来源', '当前账号新建快照'], ['计划任务', ref.task_id], ['版本 SHA-256', ref.sha256], ['开始边界（含）', time(page.start_ts)], ['结束边界（含）', time(page.end_ts)]]) meta.append(el('dt', '', key), el('dd', '', value));
      for (const row of page.rows) {
        const tr = el('tr'), identity = el('td'), statistics = el('td'), sizes = el('td'), marker = el('td'), select = el('select');
        identity.append(el('div', '', row.chat_name), el('div', 'muted', row.username), el('div', 'muted', { direct: '单聊', group: '群聊', public: '公众号' }[row.chat_type] || row.chat_type));
        select.setAttribute('aria-label', `导出标记 ${row.username}`);
        for (const [value, text] of [['', '未设'], ['0', '0 · 排除'], ['1', '1 · 纳入']]) select.add(new Option(text, value));
        select.value = planReview.changes.get(row.username)?.export ?? row.export;
        select.addEventListener('change', () => {
          setPlanChange(planReview.changes, row, select.value);
          updatePlanActions();
        });
        marker.append(select);
        statistics.append(el('div', '', `消息：${row.message_count < 0 || /message_db_missing|message_error/.test(row.size_status) ? '未统计' : row.message_count}`), el('div', 'muted', `首条：${row.first_time || '未提供'}`), el('div', 'muted', `末条：${row.last_time || '未提供'}`));
        const bytes = value => value === null || value < 0 ? '未统计' : `${value.toLocaleString('zh-CN')} 字节`;
        sizes.append(el('div', '', `附件估算：${bytes(row.attachment_estimated_bytes)}`), el('div', '', `附件扫描：${bytes(row.attachment_scanned_bytes)}`), el('div', '', `总估算：${bytes(row.total_estimated_bytes)}`), el('div', 'muted', `统计状态：${planSizeStatus(row.size_status)}`));
        tr.append(el('td', '', row.index), identity, marker, statistics, sizes); $('plan-rows').append(tr);
      }
      $('plan-previous').disabled = offset === 0; $('plan-next').disabled = page.next_offset === null;
      $('plan-page').textContent = `${Math.floor(offset / 50) + 1} / ${Math.max(1, Math.ceil(page.total / 50))}`;
    } catch (error) { if (current()) notice('plan-error', errorText(error)); }
    finally { if (current()) { planReview.busy = false; updatePlanActions(); } }
  }
  async function submitPlanAction(kind) {
    if (!planReview.page || planReview.busy) return;
    const spec = availableTasks.find(spec => spec.kind === kind && spec.enabled !== false); if (!spec) return;
    const ref = { ...planReview.ref };
    let request;
    if (kind === 'chat_plan_review') {
      if (!planReview.changes.size) return;
      request = { plan_ref: ref, changes: [...planReview.changes.values()].map(({ username, export: flag }) => ({ username, export: flag })) };
    } else {
      if (planReview.changes.size || planReview.page.selected_count === 0) return;
      request = { plan_ref: ref, plan_mode: planReview.mode, dry_run: $('plan-apply-dry').checked };
    }
    await openTask(spec, undefined, request);
  }
  function planReferenceFields(kind, request, target) {
    if (!request || !validPlanRef(request.plan_ref)) { notice('task-error', '请从已登记计划进入审阅或应用'); $('task-submit').disabled = true; return; }
    const summary = el('dl', 'key-values');
    const facts = [['计划任务', request.plan_ref.task_id], ['版本 SHA-256', request.plan_ref.sha256]];
    if (kind === 'chat_plan_review') facts.push(['修改标记', request.changes.length], ['发布方式', '新的不可变计划版本']);
    else facts.push(['选择模式', request.plan_mode === 'whitelist' ? '白名单：仅纳入 1' : '黑名单：仅排除 0'], ['执行意图', request.dry_run ? '仅预演选择' : '导出到全新归档'], ['预览选择', planReview.page.selected_count]);
    for (const [key, value] of facts) summary.append(el('dt', '', key), el('dd', '', value));
    target.append(summary); formReaders.push(options => { options[kind] = request; });
  }
  async function openTask(spec, selectedUser, planOptions) {
    if (!model.online || submitting) return;
    taskSpec = spec; formReaders = []; selection = new Set(selectedUser ? [selectedUser] : []);
    const generation = ++formGeneration; $('task-options').replaceChildren(); $('task-title').textContent = spec.name || spec.kind; notice('task-error');
    $('task-submit').disabled = false; $('task-dialog').showModal();
    const target = $('task-options');
    if (spec.kind === 'chat_plan') { planGenerationFields(spec, target); return; }
    if (['chat_plan_review', 'chat_plan_apply'].includes(spec.kind)) { planReferenceFields(spec.kind, planOptions, target); return; }
    if (spec.kind === 'export_history') { historyExportFields(target, selectedUser); return; }
    if (!Array.isArray(spec.fields)) taskFlags(spec, target);
    exportBudgets(spec, target);
    if (Array.isArray(spec.fields)) {
      let unsupported = false;
      spec.fields.forEach((entry) => { if (!field(entry, target) && entry.required) unsupported = true; });
      if (unsupported) { notice('task-error', '此任务包含当前界面不支持的必填参数。'); $('task-submit').disabled = true; }
    } else if (spec.export || spec.users) {
      if (spec.export) {
      const formats = el('fieldset'), choices = el('div', 'formats'); formats.append(el('legend', '', '导出格式'), choices);
      (spec.formats || ['json']).forEach((value) => { const input = el('input'); input.type = 'checkbox'; input.value = value; input.checked = value === 'json'; const label = el('label', 'check'); label.append(input, el('span', '', value.toUpperCase())); choices.append(label); });
      formReaders.push((options) => { options.formats = [...choices.querySelectorAll('input:checked')].map((input) => input.value); if (!options.formats.length) throw new Error('请选择至少一种导出格式'); });
      target.append(formats);
      }
      const sessions = el('fieldset'), toolbar = el('div', 'choice-actions'), search = el('input'), list = el('div', 'choice-list'), count = el('span', 'muted');
      const filters = el('div', 'filter-row'), type = el('select'), tag = el('select');
      type.setAttribute('aria-label', '待导出会话类型'); [['', '全部类型'], ['direct', '单聊'], ['group', '群聊'], ['public', '公众号']].forEach(([value, label]) => type.add(new Option(label, value)));
      tag.setAttribute('aria-label', '待导出会话标签'); tag.add(new Option('全部标签', '')); filters.append(type, tag);
      sessions.append(el('legend', '', '选择会话')); search.type = 'search'; search.placeholder = '筛选待导出会话'; search.setAttribute('aria-label', '筛选待导出会话'); sessions.append(search, filters, toolbar, list); target.append(sessions);
      const source = 'wechat', needsFetch = directoriesDirty;
      let pickerTags = model.tags, tagBusy = false, tagError = '', tagSequence = 0;
      let people = needsFetch ? [] : model.sessions;
      const itemTags = (item) => tagNames(item, source);
      const filtered = () => people.filter((item) => `${displayName(item)} ${username(item)} ${itemTags(item).join(' ')}`.toLocaleLowerCase().includes(search.value.toLocaleLowerCase()) && (!type.value || personType(item) === type.value) && (!tag.value || itemTags(item).includes(tag.value)));
      const render = () => {
        list.replaceChildren();
        count.textContent = `已选 ${selection.size} / ${people.length}`;
        if (tagBusy || tagError) { empty(list, tagError || '正在加载标签成员…'); return; }
        const visible = filtered(); if (!visible.length) empty(list, '暂无可选会话'); visible.forEach((item) => { const label = el('label'), input = el('input'); input.type = 'checkbox'; input.checked = selection.has(username(item)); input.addEventListener('change', () => { input.checked ? selection.add(username(item)) : selection.delete(username(item)); count.textContent = `已选 ${selection.size} / ${people.length}`; }); label.append(input, el('span', '', `${displayName(item)} (${username(item)})`)); list.append(label); });
      };
      toolbar.append(button('选择筛选结果', () => { if (tagBusy || tagError) return; filtered().forEach((item) => selection.add(username(item))); render(); }), button('清空', () => { selection.clear(); render(); }), count); search.addEventListener('input', render); type.addEventListener('change', render);
      tag.addEventListener('change', async () => {
        const seq = ++tagSequence; tagError = ''; tagBusy = Boolean(tag.value && source === 'wechat' && !model.tagCache.has(tag.value)); render();
        try { if (tag.value && source === 'wechat') await loadTag(tag.value); }
        catch (error) { if (seq === tagSequence && generation === formGeneration) tagError = `标签成员加载失败：${errorText(error)}`; }
        finally { if (seq === tagSequence && generation === formGeneration) { tagBusy = false; render(); } }
      });
      formReaders.push((options) => {
        if (!selection.size) throw new Error('请选择至少一个会话'); if (selection.size > 200) throw new Error('一次最多选择 200 个会话'); options.users = [...selection];
      });
      if (needsFetch) {
        $('task-submit').disabled = true; empty(list, '正在读取会话…');
        try { const data = await request(`/api/sessions?${new URLSearchParams({ source, limit: '2000' })}`); if (generation !== formGeneration) return; people = rows(data, 'sessions'); selection = new Set([...selection].filter((id) => people.some((item) => username(item) === id))); $('task-submit').disabled = false; }
        catch (error) { if (generation === formGeneration) { notice('task-error', errorText(error)); empty(list, '会话加载失败'); } return; }
        if (source === 'wechat') {
          try { pickerTags = rows(await request('/api/tags'), 'tags'); if (generation !== formGeneration) return; }
          catch (error) { if (generation === formGeneration) notice('task-error', `标签加载失败：${errorText(error)}`); }
        }
      }
      [...new Set([...people.flatMap(itemTags), ...pickerTags.map((entry) => entry.name)])].filter(Boolean).sort((a, b) => a.localeCompare(b, 'zh-CN')).forEach((name) => tag.add(new Option(name, name)));
      render();
    } else {
      const summary = el('dl', 'key-values'); summary.append(el('dt', '', '账号'), el('dd', '', $('account-summary').textContent), el('dt', '', '参数'), el('dd', '', '使用当前账号配置')); target.append(summary);
    }
  }
  let pendingSubmission = null;
  function pendingKey() { return `wx-export-pending:${location.host}:${model.accountKey}`; }
  function savedSubmission() {
    if (pendingSubmission?.account === model.accountKey) return pendingSubmission;
    try {
      const saved = JSON.parse(sessionStorage.getItem(pendingKey()) || 'null');
      if (saved?.account === model.accountKey && /^[a-f0-9]{64}$/.test(saved.key) && saved.body?.kind && saved.body?.options) return saved;
    } catch { /* Storage may be unavailable; keep the in-memory attempt. */ }
    return null;
  }
  async function submitTask(event) {
    event.preventDefault(); if (submitting || !taskSpec || !model.online) return;
    notice('task-error'); const options = Object.create(null);
    let pending = savedSubmission();
    if (['chat_plan', 'chat_plan_review', 'chat_plan_apply'].includes(pending?.body.kind || taskSpec.kind) && !availableTasks.some(spec => spec.kind === (pending?.body.kind || taskSpec.kind) && spec.enabled !== false)) {
      notice('task-error', '当前服务未启用此计划任务；待确认提交已保留。'); return;
    }
    if ((pending?.body.kind || taskSpec.kind) === 'export_history' && !availableTasks.some(spec => spec.kind === 'export_history' && spec.enabled !== false)) {
      notice('task-error', '当前服务不支持或未启用单会话历史导出；待确认提交已保留。'); return;
    }
    try {
      if (!pending) {
      formReaders.forEach((read) => read(options));
      if (Array.isArray(taskSpec.options)) Object.keys(options).forEach((key) => { if (!taskSpec.options.includes(key)) delete options[key]; });
      if (new TextEncoder().encode(JSON.stringify({ kind: taskSpec.kind, options })).length > 65536) throw new Error('本次提交超过 64 KiB，请减少修改标记或选择项');
      const key = [...crypto.getRandomValues(new Uint8Array(32))].map(byte => byte.toString(16).padStart(2, '0')).join('');
      pending = { account: model.accountKey, key, body: { kind: taskSpec.kind, options } };
      } else if (!window.confirm('上一笔提交尚未确认。是否使用原参数和原幂等键重试上一笔任务？')) return;
      pendingSubmission = pending;
      try { sessionStorage.setItem(pendingKey(), JSON.stringify(pending)); } catch { /* Retain the in-memory attempt. */ }
    } catch (error) { notice('task-error', errorText(error)); return; }
    submitting = true; $('task-submit').disabled = true;
    const epoch = model.epoch;
    try {
      const data = await request('/api/tasks', { method: 'POST', body: pending.body, idempotencyKey: pending.key });
      if (epoch !== model.epoch) return;
      pendingSubmission = null; try { sessionStorage.removeItem(pendingKey()); } catch { /* No persistent attempt. */ }
      const task = data?.task || data;
      $('task-dialog').close();
      if (['chat_plan_review', 'chat_plan_apply'].includes(pending.body.kind)) { $('plan-review-dialog').close(); planReview.changes.clear(); }
      try { await loadTasks(); } catch (error) { notice('global-error', `任务已提交，但队列刷新失败：${errorText(error)}`); }
      if (task && taskId(task)) openDetail(taskId(task));
    } catch (error) {
      if (epoch === model.epoch) {
        if ([400, 401, 403, 404, 422, 429].includes(error.status) || ['plan_ref_changed', 'plan_ref_unavailable', 'plan_selection_invalid'].includes(error.code)) {
          pendingSubmission = null; try { sessionStorage.removeItem(pendingKey()); } catch { /* No persistent attempt. */ }
        }
        notice('task-error', `${errorText(error)}。${savedSubmission() ? '提交尚未确认；再次提交将重试原任务，不采用新参数。' : '请检查参数或稍后重试。'}`);
      }
    } finally { submitting = false; $('task-submit').disabled = false; }
  }
  function logsOf(task) {
    const logs = task.logs ?? task.log ?? task.output ?? '';
    const text = Array.isArray(logs) ? logs.map((entry) => typeof entry === 'string' ? entry : [timestamp(entry.timestamp || entry.time), entry.level || entry.stream, entry.line || entry.message || entry.text].filter(Boolean).join(' ')).join('\n') : typeof logs === 'string' ? logs : JSON.stringify(logs, null, 2);
    return redact((task.log_start_seq > 0 ? `日志从序号 ${task.log_start_seq} 开始，较早内容未包含在当前响应中。\n` : '') + text);
  }
  let artifactPage = { id: null, offset: 0, generation: 0 };
  const artifactErrors = {
    task_not_terminal: '任务尚未结束', result_unavailable: '暂无可信聊天产物', artifact_unavailable: '产物暂不可读取',
    artifact_changed: '产物已变更，下载已停止', artifact_unsafe: '产物未通过安全核验', artifact_busy: '读取繁忙，请稍后重试',
    invalid_artifact_request: '产物请求参数无效', not_found: '产物不存在', configuration_changed: '账号配置已变更'
  };
  async function downloadArtifact(id, item, control) {
    const epoch = model.epoch; control.disabled = true;
    try {
      const ticket = await request(`/api/tasks/${encodeURIComponent(id)}/artifacts/${encodeURIComponent(item.artifact_id)}/ticket`, { method: 'POST', body: {} });
      if (epoch !== model.epoch || id !== model.detailId) return;
      const url = new URL(ticket.url, location.origin);
      const expected = `/api/tasks/${encodeURIComponent(id)}/artifacts/${encodeURIComponent(item.artifact_id)}/download`;
      if (url.origin !== location.origin || url.pathname !== expected) throw new Error('下载票据身份不符');
      const link = el('a'); link.href = url.href;
      link.download = item.name; link.rel = 'noopener noreferrer'; link.referrerPolicy = 'no-referrer'; document.body.append(link); link.click(); link.remove();
    } catch (error) { if (epoch === model.epoch) notice('detail-error', artifactErrors[error.code] || errorText(error)); }
    finally { control.disabled = false; }
  }
  async function loadArtifacts(id, offset = 0) {
    const epoch = model.epoch, generation = ++artifactPage.generation;
    artifactPage.id = id; artifactPage.offset = offset;
    const target = $('task-artifacts'); empty(target, '正在加载聊天产物…');
    const current = () => epoch === model.epoch && id === model.detailId && generation === artifactPage.generation;
    try {
      const page = await request(`/api/tasks/${encodeURIComponent(id)}/artifacts?${new URLSearchParams({ offset, limit: 50 })}`);
      if (!current()) return;
      target.replaceChildren();
      if (page.scope !== model.detail?.result?.scope) throw new Error('产物范围与任务结果不符');
      target.append(el('h3', '', { chat_history: '历史导出文档', chat_plan: '计划文件', chat_plan_apply: '计划导出文档' }[page.scope] || '聊天产物'), el('p', 'muted', `共 ${page.total} 项${page.complete ? '' : ' · 清单不完整'}`));
      for (const item of page.items) {
        if (!/^[a-f0-9]{64}$/.test(item.artifact_id)) continue;
        const row = el('div', 'artifact-row'), info = el('div', 'artifact-info');
        const role = {chat_plan_csv: '导出计划 CSV', chat_document: '聊天文档', media_manifest: '媒体清单', voice_manifest: '语音清单', media: '媒体文件', chat_info: '会话信息', export_inventory: '导出清单'}[item.role] || '文件';
        info.append(el('span', 'artifact-name', item.name), el('span', 'muted', `${Number(item.size).toLocaleString('zh-CN')} 字节 · ${role}`));
        const download = button('', () => downloadArtifact(id, item, download), 'icon-button');
        download.title = `下载 ${item.name}`; download.setAttribute('aria-label', download.title); download.append(icon('download'));
        row.append(info, download); target.append(row);
      }
      if (!page.items.length) target.append(el('p', 'muted', '暂无可下载聊天产物'));
      const nav = el('div', 'artifact-pagination');
      const previous = button('', () => loadArtifacts(id, Math.max(0, offset - 50)), 'icon-button');
      previous.append(icon('chevron-left')); previous.title = '上一页'; previous.setAttribute('aria-label', previous.title); previous.disabled = offset === 0;
      const next = button('', () => loadArtifacts(id, page.next_offset), 'icon-button');
      next.append(icon('chevron-right')); next.title = '下一页'; next.setAttribute('aria-label', next.title); next.disabled = page.next_offset == null;
      nav.append(previous, el('span', 'muted', `${Math.floor(offset / 50) + 1} / ${Math.max(1, Math.ceil(page.total / 50))}`), next); target.append(nav);
    } catch (error) {
      if (!current()) return;
      target.replaceChildren(el('p', 'notice error', artifactErrors[error.code] || errorText(error)), button('重试', () => loadArtifacts(id, offset)));
    }
  }
  function historyResultFacts(result) {
    const outcomes = { success: '成功', partial: '含查询警告', failure: '失败', refused: '已拒绝' };
    const facts = [['历史导出结果', outcomes[result.outcome] || '尚未确定'], ['格式', { markdown: 'Markdown', txt: 'TXT', json: 'JSON', yaml: 'YAML' }[result.format] || '未提供']];
    const time = value => {
      if (value === null) return '不限';
      const date = new Date(value * 1000);
      return Number.isFinite(date.getTime()) ? date.toISOString().replace('T', ' ').replace('.000Z', ' UTC') : `${value} Unix 秒`;
    };
    if (result.query === null) facts.push(['查询摘要', '尚未取得可信查询结果']);
    else if (result.query) {
      facts.push(['实际会话 username', result.query.username], ['开始边界（含）', time(result.query.since_ts)], ['结束边界（含）', time(result.query.until_ts)],
        ['请求条数上限', result.query.limit], ['实际消息条数', result.query.messages]);
      if (result.query.messages === 0) facts.push(['查询结果', '合法空结果']);
    }
    facts.push(['已登记产物', result.artifact_count]);
    return facts;
  }
  function planResultFacts(result) {
    const outcomes = { success: '成功', partial: '部分完成', failure: '失败', refused: '已拒绝' };
    const facts = [['计划结果', outcomes[result.outcome] || '尚未确定']];
    if (result.scope === 'chat_plan') {
      const time = value => value === null ? '不限' : new Date(value * 1000).toISOString().replace('T', ' ').replace('.000Z', ' UTC');
      facts.push(['来源', result.source_kind === 'runtime_snapshot' ? '当前账号新建快照' : '未确定'], ['计划行数', result.row_count], ['部分统计行', result.partial_rows],
        ['开始边界（含）', time(result.start_ts)], ['结束边界（含）', time(result.end_ts)], ['版本 SHA-256', result.published_plan_ref?.sha256 || '尚未发布']);
      if (result.parent_ref) facts.push(['父版本 SHA-256', result.parent_ref.sha256]);
    } else {
      facts.push(['引用 SHA-256', result.plan_ref.sha256], ['选择模式', result.plan_mode === 'whitelist' ? '白名单：仅纳入 1' : '黑名单：仅排除 0'],
        ['执行意图', result.dry_run ? '仅预演选择' : '全新归档'], ['已选择会话', result.selected_count === 0 && !result.finalized ? '尚未确认' : result.selected_count], ['已发布文档', result.published_count], [result.finalized ? '失败会话' : '已记录失败会话', result.failed_count], ['已发布消息', result.messages]);
    }
    facts.push(['已登记产物', result.artifact_count]); return facts;
  }
  function renderExportResult(task) {
    const target = $('task-result'), result = task.result;
    target.replaceChildren();
    $('export-result-title').textContent = { export_history: '单会话历史导出结果', chat_plan: '导出计划结果', chat_plan_review: '计划修订结果', chat_plan_apply: '计划应用结果' }[task.kind] || '聊天导出结果';
    const expectedScope = { export_history: 'chat_history', export_all: 'chat_directory', chat_plan: 'chat_plan', chat_plan_review: 'chat_plan', chat_plan_apply: 'chat_plan_apply' }[task.kind] || null;
    if (!result || result.version !== 1 || !expectedScope || result.scope !== expectedScope) {
      target.textContent = '暂无聊天导出结果'; $('task-artifacts').replaceChildren(); return;
    }
    const outcomes = { success: '完整', partial: '部分完成', failure: '失败', refused: '已拒绝' };
    const summary = el('dl', 'key-values');
    const facts = ['chat_plan', 'chat_plan_apply'].includes(result.scope) ? planResultFacts(result) : result.scope === 'chat_history' ? historyResultFacts(result) : result.dry_run ? [['模式', '仅核对计划'], ['计划会话', result.planned_chats ?? '未确定']]
      : [['聊天结果', outcomes[result.outcome] || '尚未确定'], ['计划会话', result.planned_chats ?? '未确定'],
        ['已导出会话', result.exported_chats], ['失败会话', result.failed_chats], ['消息', result.messages],
        ['媒体问题', result.media_issues], ['已登记产物', result.artifact_count]];
    facts.push(['报告', result.finalized ? '已终结' : '未终结'], ['产物清单', result.artifacts_complete ? '完整' : '不完整']);
    for (const [key, value] of facts) summary.append(el('dt', '', key), el('dd', '', String(value)));
    target.append(summary);
    const codes = { history_query_warning: '历史查询警告', history_export_failed: '历史导出失败', chat_export_failed: '聊天导出失败', media_unavailable: '媒体缺失', artifact_limit_exceeded: '产物数量超限', export_interrupted: '导出中断', ...artifactErrors };
    for (const diagnostic of result.diagnostics || []) target.append(el('p', 'notice', `${codes[diagnostic.code] || diagnostic.code}：${diagnostic.count}`));
    const ready = ['succeeded', 'failed', 'cancelled', 'interrupted'].includes(statusOf(task));
    if (ready && result.scope === 'chat_plan' && validPlanRef(result.published_plan_ref)) target.append(button('审阅与应用', () => openPlanReview(result.published_plan_ref)));
    if (result.dry_run || !ready) { ++artifactPage.generation; $('task-artifacts').replaceChildren(); return; }
    const capabilities = model.state.capabilities;
    const supported = Array.isArray(capabilities) ? capabilities.includes('task_artifacts_v1') : capabilities?.task_artifacts_v1 === true;
    if (!supported) { $('task-artifacts').textContent = '此版本不支持产物读取'; return; }
    if (artifactPage.loaded !== taskId(task)) {
      artifactPage.loaded = taskId(task); loadArtifacts(taskId(task));
    }
  }
  function renderDetail(task) {
    model.detail = task; const state = statusOf(task), target = $('detail-meta'); target.replaceChildren();
    $('detail-title').textContent = taskName(task); $('detail-id').textContent = taskId(task);
    const meta = el('dl', 'key-values');
    [['状态', labels[state] || state], ['阶段', task.stage || task.message || '未提供'], ['创建时间', timestamp(task.created_at) || '未提供'], ['结束时间', timestamp(task.finished_at || task.completed_at) || '未提供'], ['退出码', task.exit_code ?? '未提供']].forEach(([key, value]) => meta.append(el('dt', '', key), el('dd', '', redact(value))));
    target.append(meta); if (activeStates.has(state)) target.append(progressNode(task));
    $('task-log').textContent = logsOf(task) || '暂无日志'; if ($('follow-log').checked) $('task-log').scrollTop = $('task-log').scrollHeight;
    renderExportResult(task);
    notice('detail-error', task.error ? redact(task.error) : '');
    $('cancel-task').disabled = !activeStates.has(state) || ['cancelling', 'cancel_requested'].includes(state) || !model.online;
  }
  async function openDetail(id) {
    if (!id) return;
    model.detailId = id; model.detail = null; $('detail-title').textContent = '任务详情'; $('detail-id').textContent = id;
    artifactPage = { id, offset: 0, generation: artifactPage.generation + 1 }; $('task-artifacts').replaceChildren();
    $('detail-meta').replaceChildren(); $('task-result').textContent = ''; $('task-log').textContent = '正在加载…'; notice('detail-error'); $('cancel-task').disabled = true;
    if (!$('task-detail-dialog').open) $('task-detail-dialog').showModal(); await loadDetail();
  }
  async function loadDetail() {
    const id = model.detailId, epoch = model.epoch, seq = ++model.detailSeq; if (!id) return;
    try { const data = await request(`/api/tasks/${encodeURIComponent(id)}`); if (id === model.detailId && epoch === model.epoch && seq === model.detailSeq) renderDetail(data?.task || data); }
    catch (error) { if (id === model.detailId && epoch === model.epoch) notice('detail-error', errorText(error)); }
  }
  async function cancelTask() {
    const id = model.detailId; if (!id || !model.detail || !activeStates.has(statusOf(model.detail))) return;
    if (!window.confirm(`取消“${taskName(model.detail)}”？`)) return;
    $('cancel-task').disabled = true;
    try { await request(`/api/tasks/${encodeURIComponent(id)}/cancel`, { method: 'POST', body: {} }); await loadDetail(); await loadTasks(); }
    catch (error) { notice('detail-error', errorText(error)); if (model.detailId === id) $('cancel-task').disabled = false; }
  }
  function scheduleRefresh(tasksOnly = false) {
    taskEventsPending = true; if (!tasksOnly) dataEventsPending = true;
    if (refreshTimer) return;
    refreshTimer = setTimeout(() => {
      refreshTimer = null; const data = dataEventsPending, tasks = taskEventsPending; dataEventsPending = false; taskEventsPending = false;
      if (model.online && model.live) {
        if (data) refreshAll(); else if (tasks) loadTasks().catch((error) => notice('global-error', `任务同步失败：${errorText(error)}`));
        if ($('task-detail-dialog').open) loadDetail();
      }
    }, 700);
  }
  function receiveEvent(type, data, id) {
    if (type === 'ready') {
      try { notificationStreamReady = JSON.parse(data).replay === false; } catch { notificationStreamReady = false; }
    } else if (type === 'reset') { notificationStreamReady = false; startEvents(); }
    else if (type === 'message') { try { notifyLiveMessage(JSON.parse(data)); } catch { /* 无效事件不触发通知。 */ } }
    if (type === 'monitor_status') {
      let status; try { status = JSON.parse(data).status; } catch { status = 'error'; }
      const warnings = { catching_up: '实时消息正在补读，游标暂未推进。', limit_reached: '实时消息达到补读上限，游标已保留；请通过历史查询核对消息。', error: '实时消息查询暂未完成，游标已保留。' };
      notice('monitor-warning', model.source === 'wechat' ? warnings[status] || '' : '');
      return;
    }
    if (['heartbeat', 'ping'].includes(type)) return;
    // 不假定事件包含完整任务；统一补读权威状态，避免日志丢失或重复拼接。
    if (data || type) scheduleRefresh(['task', 'log', 'task_log', 'task_progress', 'task_done'].includes(type));
  }
  async function consumeEvents(response, signal) {
    if (!response.headers.get('content-type')?.includes('text/event-stream') || !response.body) throw new Error('实时事件接口未返回 SSE 数据流');
    const reader = response.body.getReader(), decoder = new TextDecoder();
    let buffer = '', event = '', data = [], id;
    function line(value) {
      if (signal.aborted) return;
      if (!value) { if (data.length) receiveEvent(event || 'message', data.join('\n'), id); event = ''; data = []; id = undefined; return; }
      if (value.startsWith(':')) return;
      const colon = value.indexOf(':'), field = colon < 0 ? value : value.slice(0, colon); let content = colon < 0 ? '' : value.slice(colon + 1); if (content.startsWith(' ')) content = content.slice(1);
      if (field === 'event') event = content; else if (field === 'data') data.push(content); else if (field === 'id') id = content;
    }
    try {
      while (!signal.aborted) {
        const part = await reader.read(); if (part.done) break;
        buffer += decoder.decode(part.value, { stream: true });
        if (buffer.length + data.reduce((sum, item) => sum + item.length, 0) > 2 * 1024 * 1024) throw new Error('实时事件超过大小限制');
        let position;
        while ((position = buffer.search(/[\r\n]/)) >= 0) {
          if (buffer[position] === '\r' && position === buffer.length - 1) break;
          const width = buffer[position] === '\r' && buffer[position + 1] === '\n' ? 2 : 1;
          line(buffer.slice(0, position)); buffer = buffer.slice(position + width);
        }
      }
    } finally { await reader.cancel().catch(() => {}); reader.releaseLock(); }
  }
  function stopEvents() {
    notificationStreamReady = false;
    ++streamGeneration; streamController?.abort(); streamController = null;
    clearTimeout(streamTimer); clearInterval(pollTimer); clearTimeout(refreshTimer); refreshTimer = null; pollTimer = null; taskEventsPending = false; dataEventsPending = false;
  }
  function startEvents() {
    stopEvents(); if (!model.live || !model.online) return;
    const generation = streamGeneration; let attempt = 0;
    const connect = async () => {
      if (generation !== streamGeneration || !model.live || !model.online) return;
      notificationStreamReady = false;
      const controller = new AbortController(); streamController = controller;
      try {
        const response = await request('/api/events', { stream: true, signal: controller.signal });
        connected('实时连接', 'online'); attempt = 0;
        if (model.connectedOnce) scheduleRefresh(); model.connectedOnce = true;
        await consumeEvents(response, controller.signal);
      } catch (error) { if (error.name !== 'AbortError' && generation === streamGeneration) connected('实时连接中断', 'failed'); }
      if (generation !== streamGeneration || controller.signal.aborted || !model.online) return;
      const delay = Math.min(30000, 1000 * 2 ** Math.min(attempt++, 5)); connected('等待重连', 'failed'); streamTimer = setTimeout(connect, delay);
    };
    connect();
    pollTimer = setInterval(() => { if (model.online && !document.hidden) { loadTasks().catch((error) => notice('global-error', `任务同步失败：${errorText(error)}`)); if ($('task-detail-dialog').open) loadDetail(); } }, 10000);
  }
  function clearRecords() {
    queryController?.abort(); detailController?.abort(); ++querySequence; ++detailSequence;
    queryLastParams = null; detailMessage = null;
    $('query-results').replaceChildren(); $('message-detail-result').replaceChildren();
    closeNotifications(); resetInlineImages();
    closeImages();
    capabilitiesDirty = false; directoriesDirty = false; ++directorySeq; ++directoryRevision; notice('monitor-warning');
    directoryController?.abort();
    historyController?.abort(); ++model.historySeq; ++model.detailSeq;
    model.selected = null; model.contacts = []; model.sessions = []; model.tags = []; model.tagCache.clear(); model.tagMembership.clear(); model.tagBusy = false; model.tagError = ''; ++model.tagSeq; model.messages = []; model.tasks = []; model.detail = null; model.detailId = null; model.offset = 0; model.total = null; model.hasMore = false;
    document.querySelectorAll('dialog[open]').forEach((node) => node.close()); $('task-log').textContent = ''; $('task-result').textContent = '';
    $('chat-title').textContent = '消息记录'; $('chat-subtitle').textContent = '未选择会话'; $('export-chat').disabled = true; $('refresh-history').disabled = true;
    updateImageButton();
    renderDirectory(); renderMessages(); renderQueue();
  }
  function replaceToken(value) {
    notificationWatermarks.clear();
    stopEvents(); ++model.epoch; token = value.trim(); storage.set('wx.web.token', token); model.online = false; model.state = {}; model.accountKey = undefined; model.directoryError = {}; clearRecords(); renderState();
  }
  initNotificationSettings(); initInlinePreview();
  document.querySelectorAll('[data-icon]').forEach((node) => node.append(icon(node.dataset.icon)));
  const historyFilters = el('details', 'history-query');
  historyFilters.append(el('summary', '', '记录范围'));
  const historyForm = el('form', 'query-fields');
  for (const [key, title] of [['since', '开始时间'], ['until', '结束时间']]) {
    const label = el('label', '', title), input = el('input'); input.type = 'datetime-local'; input.id = `history-${key}`; label.append(input); historyForm.append(label);
  }
  const typesLabel = el('label', '', '消息类型'), typesSelect = el('select'); typesSelect.id = 'history-types'; typesSelect.multiple = true; typesSelect.size = 3;
  for (const [key, title] of [['text', '文字'], ['image', '图片'], ['voice', '语音'], ['video', '视频'], ['sticker', '表情'], ['location', '位置'], ['link', '应用消息（含链接、文件）'], ['call', '通话事件'], ['system', '系统']]) { const option = el('option', '', title); option.value = key; typesSelect.append(option); }
  typesLabel.append(typesSelect); historyForm.append(typesLabel);
  const oldestLabel = el('label', 'check', '从旧到新'), oldest = el('input'); oldest.type = 'checkbox'; oldest.id = 'history-oldest'; oldestLabel.prepend(oldest); historyForm.append(oldestLabel);
  const applyHistory = el('button', 'primary', '应用筛选'); applyHistory.type = 'submit'; historyForm.append(applyHistory);
  historyForm.addEventListener('submit', event => { event.preventDefault(); model.offset = 0; loadHistory(); }); historyFilters.append(historyForm);
  document.querySelector('.message-filters').after(historyFilters);
  $('query-open').addEventListener('click', () => { queryFields(); $('query-dialog').showModal(); });
  $('query-kind').addEventListener('change', queryFields);
  $('query-form').addEventListener('submit', event => { event.preventDefault(); runQuery(); });
  $('query-previous').addEventListener('click', () => { queryOffset = Math.max(0, queryOffset - Number(new URLSearchParams(queryLastParams).get('limit'))); runQuery(true); });
  $('query-next').addEventListener('click', () => { queryOffset += Number(new URLSearchParams(queryLastParams).get('limit')); runQuery(true); });
  $('query-dialog').addEventListener('close', () => { queryController?.abort(); ++querySequence; });
  $('message-detail-form').addEventListener('submit', loadMessageDetail);
  $('message-detail-kind').addEventListener('change', () => { $('message-item-field').hidden = $('message-detail-kind').value !== 'record-item'; });
  $('message-detail-dialog').addEventListener('close', () => { detailController?.abort(); ++detailSequence; detailMessage = null; });
  document.querySelectorAll('[data-close]').forEach((node) => node.addEventListener('click', () => $(node.dataset.close).close()));
  $('task-dialog').addEventListener('close', () => { ++formGeneration; });
  $('task-detail-dialog').addEventListener('close', () => { model.detailId = null; ++model.detailSeq; });
  $('images-dialog').addEventListener('close', resetImages);
  $('preview-images').addEventListener('click', () => {
    if ($('preview-images').disabled) return;
    closeImages(); imageOffset = 0; $('images-title').textContent = `${displayName(model.selected)} · 图片`;
    $('images-dialog').showModal(); loadImages();
  });
  $('images-previous').addEventListener('click', () => { imageOffset = Math.max(0, imageOffset - 20); loadImages(); });
  $('images-next').addEventListener('click', () => { imageOffset += 20; loadImages(); });
  $('images-refresh').addEventListener('click', loadImages);
  document.querySelectorAll('[data-view]').forEach((node) => {
    node.addEventListener('click', () => {
      model.view = node.dataset.view;
      document.querySelectorAll('[data-view]').forEach((tab) => { tab.setAttribute('aria-selected', String(tab === node)); tab.tabIndex = tab === node ? 0 : -1; });
      $('directory-list').setAttribute('aria-labelledby', node.id); renderDirectory();
    });
    node.addEventListener('keydown', (event) => { if (['ArrowLeft', 'ArrowRight', 'Home', 'End'].includes(event.key)) { event.preventDefault(); const next = event.key === 'Home' ? $('sessions-tab') : event.key === 'End' ? $('contacts-tab') : node.id === 'sessions-tab' ? $('contacts-tab') : $('sessions-tab'); next.click(); next.focus(); } });
  });
  ['directory-search', 'type-filter'].forEach((id) => $(id).addEventListener('input', renderDirectory));
  $('tag-filter').addEventListener('change', chooseTag);
  ['message-search', 'message-type'].forEach((id) => $(id).addEventListener('input', renderMessages));
  $('queue-filter').addEventListener('change', renderQueue);
  $('refresh-all').addEventListener('click', refreshAll);
  $('refresh-tasks').addEventListener('click', () => loadTasks().catch((error) => notice('global-error', errorText(error))));
  $('refresh-history').addEventListener('click', () => loadHistory());
  $('previous-page').addEventListener('click', () => { model.offset = Math.max(0, model.offset - Number($('page-size').value)); loadHistory(); });
  $('next-page').addEventListener('click', () => { model.offset += Number($('page-size').value); loadHistory(); });
  $('page-size').addEventListener('change', () => { model.offset = 0; loadHistory(); });
  $('export-chat').addEventListener('click', () => { const spec = availableTasks.find((item) => item.kind === 'export_all'); if (spec && model.selected && spec.enabled !== false) openTask(spec, username(model.selected)); });
  $('task-form').addEventListener('submit', submitTask);
  $('plan-read-mode').addEventListener('change', () => { planReview.mode = $('plan-read-mode').value; readPlanPage(0); });
  $('plan-previous').addEventListener('click', () => { if (!planReview.busy && planReview.page) readPlanPage(Math.max(0, planReview.page.offset - 50)); });
  $('plan-next').addEventListener('click', () => { if (!planReview.busy && planReview.page?.next_offset != null) readPlanPage(planReview.page.next_offset); });
  $('plan-reload').addEventListener('click', () => { if (!planReview.busy) readPlanPage(planReview.page?.offset || 0); });
  $('plan-save-review').addEventListener('click', () => submitPlanAction('chat_plan_review'));
  $('plan-apply').addEventListener('click', () => submitPlanAction('chat_plan_apply'));
  $('detail-refresh').addEventListener('click', loadDetail);
  $('cancel-task').addEventListener('click', cancelTask);
  $('download-log').addEventListener('click', () => {
    if (!model.detail) return;
    const url = URL.createObjectURL(new Blob([logsOf(model.detail)], { type: 'text/plain;charset=utf-8' }));
    const link = el('a'); link.href = url; link.download = `wx-task-${taskId(model.detail).replace(/[^a-z0-9_-]/gi, '_')}.log`; document.body.append(link); link.click(); link.remove(); setTimeout(() => URL.revokeObjectURL(url), 1000);
  });
  $('settings-open').addEventListener('click', () => { renderState(); updateNotificationPermission(); $('settings-dialog').showModal(); });
  $('token-form').addEventListener('submit', async (event) => { event.preventDefault(); replaceToken($('token-input').value); $('token-input').value = ''; await refreshAll(); });
  $('forget-token').addEventListener('click', () => { replaceToken(''); connected('未连接'); notice('global-error', '认证已清除'); });
  $('compact-mode').addEventListener('change', (event) => { document.body.classList.toggle('compact', event.target.checked); storage.set('wx.web.compact', event.target.checked ? 'yes' : 'no'); });
  $('live-mode').addEventListener('change', (event) => { model.live = event.target.checked; if (model.live) { refreshAll(); startEvents(); } else { stopEvents(); connected(model.online ? '实时更新已暂停' : '未连接'); } });
  $('sidebar-toggle').addEventListener('click', () => { const open = $('sidebar').classList.toggle('open'); $('sidebar-toggle').setAttribute('aria-expanded', String(open)); });
  document.addEventListener('click', (event) => { if ($('sidebar').classList.contains('open') && !event.target.closest('#sidebar, #sidebar-toggle, dialog')) { $('sidebar').classList.remove('open'); $('sidebar-toggle').setAttribute('aria-expanded', 'false'); } });
  $('back-directory').addEventListener('click', () => { document.body.classList.remove('chat-open'); $('directory-search').focus(); });
  document.addEventListener('keydown', (event) => { if (event.key === 'Escape') { $('sidebar').classList.remove('open'); $('sidebar-toggle').setAttribute('aria-expanded', 'false'); } });
  document.addEventListener('visibilitychange', () => { if (!document.hidden && model.live && model.online) scheduleRefresh(); });
  window.addEventListener('pagehide', () => { closeImages(); resetInlineImages(); closeNotifications(); stopEvents(); historyController?.abort(); directoryController?.abort(); });
  window.addEventListener('pageshow', (event) => { if (event.persisted) refreshAll(); });
  $('compact-mode').checked = storage.get('wx.web.compact') !== 'no'; document.body.classList.toggle('compact', $('compact-mode').checked);
  renderTools(); renderState(); refreshAll();
})();
