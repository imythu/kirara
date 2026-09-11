// UI contract checks with explicitly synthetic API responses; real parser/queue behavior is covered by Rust tests.
// Run against Vite: PLAYWRIGHT_MODULE=/path/to/playwright RSS_TEST_URL=http://127.0.0.1:4189 node frontend/tests/rss.browser.cjs
const { chromium } = require(process.env.PLAYWRIGHT_MODULE || 'playwright');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const url = process.env.RSS_TEST_URL || 'http://127.0.0.1:4189';
const output = path.resolve(process.env.RSS_CAPTURE_DIR || path.join(__dirname, '../../.impeccable/review/rss'));
const time = '2026-09-09T10:20:00Z';
const GiB = 1024 ** 3;
const clone = value => JSON.parse(JSON.stringify(value));

function fixture() {
  const filters = { include: ['Documentary', '1080p'], include_mode: 'all', exclude: ['CAMRip'], include_regex: null, exclude_regex: null, match_all: false, min_size_bytes: GiB, max_size_bytes: 20 * GiB, min_seeders: null, free_only: false, hr_policy: 'require_clear' };
  const options = { save_path: '/downloads/documentary', category: 'documentary', tags: ['rss'], paused: false, reserve_space_bytes: 10 * GiB };
  const feed = { id: 1, name: '纪录片订阅（测试数据）', url_display: 'https://tracker.example/••••?passkey=••••', site_id: 1, site_name: '测试站点', use_proxy: null, enabled: true, interval_minutes: 15, generation: 1, version: 3, initialized_at: time, last_sequence: 3, last_checked_at: time, next_run_at: '2026-09-09T10:35:00Z', last_status: 'success', last_error: null, item_count: 3, pending_count: 1, created_at: time, updated_at: time };
  const rule = { id: 1, name: '纪录片 WEB-DL', enabled: true, priority: 100, feed_ids: [1], filters, downloader_id: 1, downloader_name: '家中 qBittorrent', options, match_revision: 2, version: 4, last_error: null, matched_count: 12, created_at: time, updated_at: time };
  const evaluation = { matched: true, needs_attributes: false, reasons: [{ code: 'matched', message: '标题与已知属性均符合规则', field: 'title', actual: 'Documentary 1080p', expected: 'Documentary、1080p' }] };
  const attrs = { size_bytes: 6.4 * GiB, seeders: 26, leechers: 3, download_volume_factor: 0, upload_volume_factor: 1, hr: false, minimum_ratio: null, minimum_seed_time: null, free_until: null, observed_at: time, source: 'torznab', hints: [] };
  const item = { id: 1, feed_id: 1, feed_name: feed.name, generation: 1, item_key: 'item-1', sequence: 1, title: 'Deep Ocean Documentary 2026 1080p WEB-DL', detail_url: null, site_torrent_id: '101', published_at: time, categories: ['纪录片'], attributes: attrs, downloadable: true, content_revision: 1, first_seen_at: time, last_seen_at: time, status: 'baseline', decisions: [] };
  const items = [item, { ...clone(item), id: 2, item_key: 'item-2', title: 'Wild Forest Documentary 1080p WEB-DL', attributes: { ...attrs, hr: null }, status: 'attribute_unknown', decisions: [{ id: 2, item_id: 2, rule_id: 1, rule_name: rule.name, match_revision: 2, status: 'attribute_unknown', evaluation: { matched: false, needs_attributes: true, reasons: [{ code: 'attribute_unknown', message: '源未提供 H&R 信息，等待站点补充查询', field: 'hr', actual: '未知', expected: '确认无 H&R' }] }, checked_at: time, next_evaluate_at: '2026-09-09T10:21:00Z', job_id: null }] }, { ...clone(item), id: 3, item_key: 'item-3', title: 'Concert 2160p BluRay REMUX', attributes: { ...attrs, size_bytes: 32 * GiB }, status: 'skipped' }];
  const job = { id: 1, item_id: 1, feed_id: 1, feed_generation: 1, feed_name: feed.name, rule_id: 1, rule_name: rule.name, downloader_id: 1, downloader_name: rule.downloader_name, title: item.title, size_bytes: attrs.size_bytes, filters_snapshot: filters, options_snapshot: options, decision_snapshot: evaluation, status: 'submitted', infohash: 'a'.repeat(40), reserved_bytes: 0, attempts: 1, next_attempt_at: null, version: 2, last_error: null, created_at: time, updated_at: time, submitted_at: time, download_state: 'downloading', progress: 0.42, sampled_at: time };
  return { feeds: [feed, { ...clone(feed), id: 2, name: '音乐精选（测试数据）', enabled: false, last_status: 'error', last_error: '站点登录已失效，请更新站点 Cookie', item_count: 0, pending_count: 0 }], rules: [rule], items, jobs: [job, { ...clone(job), id: 2, title: items[1].title, status: 'failed', version: 1, last_error: '下载器暂时无法连接，请检查连接配置后重试', submitted_at: null, progress: null, download_state: null, sampled_at: null }, { ...clone(job), id: 3, title: 'Silent Planet Documentary 1080p WEB-DL', status: 'reconciling', version: 1, last_error: '提交响应超时，正在确认下载器是否已收到', submitted_at: null, progress: null, download_state: null, sampled_at: null }], runs: [], writes: [], reads: [], feedTestFail: true, feedSaveFail: true, ruleSaveFail: true, backfillFail: true, listFail: false, noDownloaders: false, archived: [] };
}

function pageOf(items, search) {
  const page = Number(search.get('page') || 1), size = Number(search.get('page_size') || 20);
  return { items: items.slice((page - 1) * size, page * size), total: items.length, page, page_size: size };
}

function responder(state) {
  return async (requestPath, method = 'GET', rawBody) => {
    const address = new URL(requestPath, 'http://mock'), route = address.pathname, search = address.searchParams;
    const body = typeof rawBody === 'string' ? JSON.parse(rawBody) : rawBody;
    const ok = (data, status = 200) => ({ status, data });
    const fail = (error, status = 422) => ok({ error }, status);
    if (method !== 'GET') state.writes.push({ path: route, method, body: clone(body) }); else state.reads.push(requestPath);
    if (route === '/api/features') return ok({ self_use: false });
    if (route === '/api/settings') return ok({ log_level: 'info', proxy: null, use_proxy_for_lightpanda: false, lightpanda: {}, browserless: {}, ocr_api_key: null });
    if (route === '/api/sites') return ok([{ id: 1, name: '测试站点', site_type: 'nexusphp', base_url: 'https://tracker.example', auth_configured: true, use_proxy: false }]);
    if (route === '/api/downloaders') return ok(state.noDownloaders ? [] : [{ id: 1, name: '家中 qBittorrent', downloader_type: 'qbittorrent', url: 'http://qb.local', username: 'test', password_configured: true }]);
    if (route === '/api/rss/summary') return ok({ feeds_total: state.feeds.length, running: state.feeds.filter(f => f.enabled).length, paused: state.feeds.filter(f => !f.enabled).length, needs_attention: 2, rules_enabled: state.rules.filter(r => r.enabled).length, queued: 0, submitted: 1, failed: 1 });
    if (route === '/api/rss/feeds/test') {
      assert.equal(method, 'POST');
      if (state.feedTestFail) { state.feedTestFail = false; return fail('源返回了登录页面，请更新站点凭据后重试'); }
      return ok({ title: '测试 RSS 来源', item_count: 3, items: clone(state.items), warnings: [], sample_time: time });
    }
    if (route === '/api/rss/rules/preview') {
      assert.equal(method, 'POST');
      if (body.rule.filters.include_regex === '(?=abc)') return fail('包含正则无效：不支持环视');
      const selected = body.item_ids?.length ? state.items.filter(item => body.item_ids.includes(item.id)) : state.items;
      const results = selected.map(item => ({ item: clone(item), evaluation: item.id === 1 ? clone(state.jobs[0].decision_snapshot) : item.id === 2 ? clone(state.items[1].decisions[0].evaluation) : { matched: false, needs_attributes: false, reasons: [{ code: 'include_not_matched', message: '标题缺少包含词 Documentary', field: 'title', actual: item.title, expected: 'Documentary' }] } }));
      return ok({ total: results.length, matched: results.filter(r => r.evaluation.matched).length, rejected: results.filter(r => !r.evaluation.matched && !r.evaluation.needs_attributes).length, unknown: results.filter(r => r.evaluation.needs_attributes).length, sample_limited: false, sample_time: time, items: results });
    }
    if (route === '/api/rss/backfills') {
      assert(body.request_id && body.expected_version && body.item_ids.length > 0);
      if (state.backfillFail) { state.backfillFail = false; return fail('下载器连接尚未恢复，补下请求未提交', 503); }
      const run = { id: 20, feed_id: 1, kind: 'backfill', status: 'completed', item_count: body.item_ids.length, new_count: 0, queued_count: 1, pending_count: 0, message: '所选资源已重新判定，符合项已加入队列', started_at: time, finished_at: time };
      state.runs.unshift(run); return ok(run, 202);
    }
    const collection = route.match(/^\/api\/rss\/(feeds|rules|downloads|items|runs)$/);
    if (collection) {
      const kind = collection[1], records = state[kind === 'downloads' ? 'jobs' : kind];
      if (method === 'POST') {
        assert(body.request_id);
        if (kind === 'feeds' && state.feedSaveFail) { state.feedSaveFail = false; return fail('保存暂时失败，请重试', 503); }
        if (kind === 'rules' && state.ruleSaveFail) { state.ruleSaveFail = false; return fail('规则保存暂时失败，请重试', 503); }
        const base = kind === 'feeds' ? clone(state.feeds[0]) : clone(state.rules[0]);
        const added = { ...base, ...body, id: Math.max(0, ...records.map(r => r.id)) + 1, version: 1 };
        if (kind === 'feeds') { delete added.url; added.url_display = 'https://new.example/••••'; added.item_count = 0; added.pending_count = 0; added.initialized_at = null; }
        records.unshift(added); return ok(added, 201);
      }
      if (state.listFail && kind === 'feeds') return fail('连接中断，无法刷新列表', 503);
      let found = records.filter(record => !state.archived.includes(`${kind}-${record.id}`));
      if (search.get('keyword')) found = found.filter(record => (record.name || record.title || '').includes(search.get('keyword')));
      if (search.get('feed_id')) found = found.filter(record => record.feed_ids ? record.feed_ids.includes(Number(search.get('feed_id'))) : record.feed_id === Number(search.get('feed_id')));
      if (search.get('rule_id')) found = found.filter(record => record.rule_id === Number(search.get('rule_id')));
      const status = search.get('status');
      if (status) found = found.filter(record => ['running', 'enabled'].includes(status) ? record.enabled : status === 'paused' ? !record.enabled : status === 'needs_attention' ? !!record.last_error || record.status === 'attribute_unknown' : record.status === status);
      return ok(pageOf(found, search));
    }
    const detail = route.match(/^\/api\/rss\/(feeds|rules|downloads|items|runs)\/(\d+)(?:\/(\w+))?$/);
    if (detail) {
      const [, kind, id, action] = detail, records = state[kind === 'downloads' ? 'jobs' : kind];
      const record = records.find(record => record.id === Number(id));
      if (!record) return fail('记录不存在', 404);
      if (method === 'GET') return ok(clone(record));
      assert.equal(body.expected_version, record.version);
      assert(body.request_id);
      if (method === 'DELETE') { state.archived.push(`${kind}-${id}`); return ok(null, 204); }
      if (action === 'check') { const run = { id: 10, feed_id: record.id, kind: 'manual', status: 'completed', item_count: 3, new_count: 1, queued_count: 0, pending_count: 0, message: '检查完成，没有新增下载任务', started_at: time, finished_at: time }; state.runs.unshift(run); return ok(run, 202); }
      if (method === 'PUT') { if (kind === 'rules' && state.ruleSaveFail) { state.ruleSaveFail = false; return fail('规则保存暂时失败，请重试', 503); } Object.assign(record, body); delete record.url; }
      if (action === 'pause' || action === 'resume') record.enabled = action === 'resume';
      if (action === 'retry') { record.status = 'queued'; record.last_error = null; }
      if (action === 'cancel') record.status = 'cancelled';
      if (action === 'reconcile') record.status = 'reconciling';
      record.version += 1; return ok(clone(record));
    }
    throw new Error(`Unexpected API ${method} ${requestPath}`);
  };
}

async function install(page, state, desktop = false) {
  const respond = responder(state);
  if (desktop) {
    await page.exposeBinding('__rssBridge', (_, request) => respond(request.path, request.method, request.body));
    await page.addInitScript(() => {
      window.isTauri = true;
      window.__TAURI_INTERNALS__ = { invoke: async (command, args) => {
        if (command !== 'api_request') throw new Error(`Unexpected IPC ${command}`);
        const response = await window.__rssBridge(args.request);
        return { status: response.status, statusText: response.status === 200 ? 'OK' : 'Test response', body: response.status === 204 ? '' : JSON.stringify(response.data) };
      } };
    });
    await page.route('**/api/**', () => { throw new Error('Desktop page must use IPC'); });
  } else await page.route('**/api/**', async route => {
    const request = route.request();
    const response = await respond(new URL(request.url()).pathname + new URL(request.url()).search, request.method(), request.postData());
    await route.fulfill({ status: response.status, contentType: 'application/json', body: response.status === 204 ? '' : JSON.stringify(response.data) });
  });
}

async function noOverflow(page) {
  assert.equal(await page.evaluate(() => document.documentElement.scrollWidth > innerWidth), false, 'document overflows horizontally');
  const bad = await page.locator('[data-rss-page]').evaluate(root => [...root.querySelectorAll('input,textarea,button,[role="dialog"]')].filter(node => node.getBoundingClientRect().width && (node.getBoundingClientRect().left < -1 || node.getBoundingClientRect().right > innerWidth + 1)).map(node => node.textContent || node.id));
  assert.deepEqual(bad, [], 'RSS controls must fit viewport');
}

async function capture(page, name) {
  await page.locator('.kirara-content').evaluate(node => node.scrollTo({ top: 0, behavior: 'instant' }));
  await page.evaluate(() => { window.scrollTo({ top: 0, behavior: 'instant' }); document.querySelectorAll('[role="dialog"] .overflow-auto').forEach(node => node.scrollTo({ top: 0, behavior: 'instant' })); });
  if (process.env.RSS_CAPTURE !== '0') await page.screenshot({ path: path.join(output, `${name}.png`), fullPage: true, animations: 'disabled' });
  await noOverflow(page);
}

async function localFeedback(page, scope, message, action, name) {
  const alert = scope.getByRole('alert').filter({ hasText: message });
  await alert.waitFor();
  await page.evaluate(() => new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve))));
  for (const target of [alert, action]) {
    const visible = await target.evaluate(node => {
      const rect = node.getBoundingClientRect();
      const atCenter = document.elementFromPoint(rect.left + rect.width / 2, rect.top + rect.height / 2);
      return rect.top >= 0 && rect.bottom <= innerHeight && !!atCenter && (node.contains(atCenter) || atCenter.contains(node));
    });
    assert(visible, `${name}: feedback and recovery action must be visible together without scrolling`);
  }
  if (process.env.RSS_CAPTURE !== '0') await page.screenshot({ path: path.join(output, `${name}.png`), fullPage: false, animations: 'disabled' });
}

(async () => {
  fs.mkdirSync(output, { recursive: true });
  const browser = await chromium.launch({ headless: true, args: ['--no-sandbox'] });
  try {
    for (const width of [1440, 390]) {
      const page = await browser.newPage({ viewport: { width, height: 1000 } });
      const state = fixture(), errors = [];
      page.on('pageerror', error => errors.push(error.message));
      await install(page, state);
      await page.goto(`${url}/#/rss`);
      await page.getByRole('button', { name: state.feeds[0].name, exact: true }).waitFor();
      await capture(page, width === 1440 ? 'desktop' : 'mobile');

      // Refresh failures keep the last successful data visible.
      state.listFail = true;
      await page.getByRole('button', { name: '刷新列表', exact: true }).click();
      await page.getByRole('alert').filter({ hasText: '连接中断，无法刷新列表' }).waitFor();
      assert(await page.getByRole('button', { name: state.feeds[0].name, exact: true }).isVisible());
      state.listFail = false;
      await page.getByRole('button', { name: '重新加载', exact: true }).click();

      // Saved addresses are never put back into the input; testing credentials uses POST.
      await page.getByRole('button', { name: `编辑${state.feeds[0].name}`, exact: true }).click();
      const dialog = page.getByRole('dialog', { name: '编辑订阅源', exact: true });
      assert.equal(await dialog.locator('input[type="url"]').count(), 0, 'saved RSS addresses must not be editable');
      assert(await dialog.getByText('地址保存后不可修改。如需使用其他地址，请添加订阅源。', { exact: true }).isVisible());
      await dialog.getByLabel('订阅源名称', { exact: true }).fill('纪录片订阅（测试数据）');
      await dialog.getByRole('button', { name: '测试并预览', exact: true }).click();
      await dialog.getByText('源返回了登录页面，请更新站点凭据后重试', { exact: true }).waitFor();
      assert.equal(await dialog.getByLabel('订阅源名称', { exact: true }).inputValue(), '纪录片订阅（测试数据）');
      await dialog.getByRole('button', { name: '测试并预览', exact: true }).click();
      await dialog.getByText('测试 RSS 来源 · 3 条', { exact: true }).waitFor();
      assert.equal(state.writes.filter(r => r.path.endsWith('/feeds/test')).at(-1).body.url, null);
      await capture(page, `feed-dialog-${width}`);
      await dialog.getByRole('button', { name: '保存订阅源', exact: true }).click();
      await dialog.waitFor({ state: 'hidden' });
      const edit = state.writes.find(request => request.path === '/api/rss/feeds/1' && request.method === 'PUT');
      assert.equal(edit.body.url, null, 'editing other settings must never resend the masked URL');
      await page.goto(`${url}/#/rss`);
      await page.getByRole('button', { name: state.feeds[0].name, exact: true }).waitFor();

      // Discard confirmation stays beside Cancel even after scrolling to the bottom.
      await page.getByRole('button', { name: '添加订阅源', exact: true }).click();
      const unsaved = page.getByRole('dialog', { name: '添加订阅源', exact: true });
      await unsaved.getByLabel('订阅源名称', { exact: true }).fill('未保存的草稿');
      await unsaved.getByLabel('RSS 地址', { exact: true }).fill('https://draft.example/rss');
      await unsaved.locator('.overflow-auto').evaluate(node => node.scrollTo({ top: node.scrollHeight, behavior: 'instant' }));
      const beforeDiscard = state.writes.length;
      await unsaved.getByRole('button', { name: '取消', exact: true }).click();
      const keepEditing = unsaved.getByRole('button', { name: '继续编辑', exact: true });
      await localFeedback(page, unsaved, '当前有未保存的修改', keepEditing, `discard-confirmation-${width}`);
      await localFeedback(page, unsaved, '当前有未保存的修改', unsaved.getByRole('button', { name: '丢弃修改并关闭', exact: true }), `discard-confirmation-${width}`);
      assert(await keepEditing.evaluate(node => node === document.activeElement));
      await keepEditing.click();
      assert.equal(await unsaved.getByLabel('订阅源名称', { exact: true }).inputValue(), '未保存的草稿');
      await unsaved.getByRole('button', { name: '关闭添加订阅源', exact: true }).click();
      await page.keyboard.press('Escape');
      assert(await unsaved.isVisible(), 'Escape must not discard an unsaved draft');
      await unsaved.getByRole('button', { name: '丢弃修改并关闭', exact: true }).click();
      await unsaved.waitFor({ state: 'hidden' });
      assert.equal(state.writes.length, beforeDiscard, 'closing must not save or download');

      // Create: an HTTP failure preserves both the draft and its idempotency key.
      await page.getByRole('button', { name: '添加订阅源', exact: true }).click();
      const add = page.getByRole('dialog', { name: '添加订阅源', exact: true });
      await add.getByLabel('订阅源名称', { exact: true }).fill('新来源（测试数据）');
      const feedSave = add.getByRole('button', { name: '保存订阅源', exact: true });
      await add.getByLabel('RSS 地址', { exact: true }).fill('invalid-url');
      await add.locator('.overflow-auto').evaluate(node => node.scrollTo({ top: node.scrollHeight, behavior: 'instant' }));
      await feedSave.click();
      await localFeedback(page, add, '请输入完整的 RSS 地址', feedSave, `feed-validation-${width}`);
      assert.equal(await add.getByLabel('RSS 地址', { exact: true }).getAttribute('aria-invalid'), 'true');
      assert((await add.getByLabel('RSS 地址', { exact: true }).getAttribute('aria-describedby')).includes('rss-feed-url-error'));
      assert.equal(await add.getByLabel('RSS 地址', { exact: true }).evaluate(node => node === document.activeElement), false, 'validation must not steal focus');
      await add.getByLabel('RSS 地址', { exact: true }).fill('https://new.example/rss?passkey=synthetic-only');
      await add.getByRole('button', { name: '保存订阅源', exact: true }).click();
      await add.getByText('保存暂时失败，请重试', { exact: true }).waitFor();
      await localFeedback(page, add, '保存暂时失败，请重试', feedSave, `feed-save-error-${width}`);
      await add.getByRole('button', { name: '保存订阅源', exact: true }).click();
      await page.getByRole('heading', { name: '新来源（测试数据）', exact: true }).waitFor();
      const creates = state.writes.filter(r => r.path === '/api/rss/feeds' && r.method === 'POST');
      assert.equal(creates.length, 2); assert.equal(creates[0].body.request_id, creates[1].body.request_id);
      assert(!state.writes.some(r => r.path === '/api/rss/backfills'));

      await page.goto(`${url}/#/rss?tab=feeds&feed=1`);
      await page.getByRole('heading', { name: '纪录片订阅（测试数据）', exact: true }).waitFor();
      await page.getByText('查看筛选结果与资源信息', { exact: true }).nth(1).click();
      await page.getByText('源未提供 H&R 信息，等待站点补充查询', { exact: true }).waitFor();
      await capture(page, `items-${width}`);
      await page.getByLabel(`选择 ${state.items[0].title}`, { exact: true }).check();
      await page.getByRole('button', { name: '补下所选条目', exact: true }).click();
      const backfill = page.getByRole('dialog', { name: '补下已有条目', exact: true });
      const submit = backfill.getByRole('button', { name: '确认补下所选条目', exact: true });
      assert(await submit.isDisabled());
      await backfill.getByRole('button', { name: '查看匹配结果', exact: true }).click();
      await backfill.getByText('符合 1', { exact: true }).waitFor();
      await backfill.locator('summary').filter({ hasText: state.items[0].title }).click();
      await backfill.locator('.overflow-auto').evaluate(node => node.scrollTo({ top: node.scrollHeight, behavior: 'instant' }));
      await submit.click();
      await backfill.getByText('下载器连接尚未恢复，补下请求未提交', { exact: true }).waitFor();
      await localFeedback(page, backfill, '下载器连接尚未恢复，补下请求未提交', submit, `backfill-error-${width}`);
      await submit.click();
      await backfill.waitFor({ state: 'hidden' });
      const backfills = state.writes.filter(r => r.path === '/api/rss/backfills');
      assert.equal(backfills[0].body.request_id, backfills[1].body.request_id); assert.deepEqual(backfills[1].body.item_ids, [1]);

      await page.getByRole('button', { name: '立即检查', exact: true }).click();
      await page.getByRole('button', { name: '查看处理记录', exact: true }).click();
      await page.getByRole('dialog', { name: '检查与处理记录', exact: true }).getByText('检查完成，没有新增下载任务', { exact: true }).waitFor();
      await page.getByRole('button', { name: '关闭检查与处理记录', exact: true }).click();

      // The rule editor consumes backend explanations and does not infer unknown H&R as false.
      await page.goto(`${url}/#/rss?tab=rules&edit=1`);
      await page.getByLabel('规则名称', { exact: true }).waitFor();
      assert.equal(await page.getByLabel('规则名称', { exact: true }).inputValue(), '纪录片 WEB-DL');
      if (width < 1280) await page.getByRole('tab', { name: '匹配预览', exact: true }).click();
      await page.getByRole('button', { name: '查看匹配结果', exact: true }).click();
      await page.getByText('信息不足 1', { exact: true }).waitFor();
      await page.locator('summary').filter({ hasText: state.items[1].title }).click();
      await page.getByText('源未提供 H&R 信息，等待站点补充查询', { exact: true }).waitFor();
      await capture(page, `rule-preview-${width}`);
      await page.getByRole('button', { name: '读取最新资源', exact: true }).click();
      await page.getByRole('button', { name: '查看匹配结果', exact: true }).waitFor();
      assert.equal(state.writes.filter(r => r.path.endsWith('/rules/preview')).at(-1).body.refresh_samples, true);
      if (width < 1280) await page.getByRole('tab', { name: '规则配置', exact: true }).click();
      const hrHelp = page.locator('summary').filter({ hasText: 'H&R 是什么？' });
      await hrHelp.focus();
      await page.keyboard.press('Enter');
      assert.equal(await hrHelp.evaluate(node => node.parentElement.open), true);
      assert(await page.getByText('有些资源要求下载后继续上传分享', { exact: false }).isVisible());
      await hrHelp.scrollIntoViewIfNeeded();
      if (process.env.RSS_CAPTURE !== '0') await page.screenshot({ path: path.join(output, `rule-help-${width}.png`), animations: 'disabled' });
      await hrHelp.click();
      await page.getByLabel('规则名称', { exact: true }).fill('纪录片规则改名（测试数据）');
      await capture(page, `rule-config-${width}`);
      await page.getByLabel('规则优先级', { exact: true }).scrollIntoViewIfNeeded();
      if (process.env.RSS_CAPTURE !== '0') await page.screenshot({ path: path.join(output, `rule-destination-${width}.png`), animations: 'disabled' });
      await noOverflow(page);
      const ruleSaveArea = page.getByRole('region', { name: '保存规则', exact: true });
      const ruleSave = ruleSaveArea.getByRole('button', { name: '保存并启用', exact: true });
      await page.getByLabel('规则名称', { exact: true }).fill('');
      await ruleSave.scrollIntoViewIfNeeded();
      await ruleSave.click();
      await localFeedback(page, ruleSaveArea, '请填写规则名称。', ruleSave, `rule-validation-${width}`);
      assert.equal(await page.getByLabel('规则名称', { exact: true }).getAttribute('aria-invalid'), 'true');
      assert((await page.getByLabel('规则名称', { exact: true }).getAttribute('aria-describedby')).includes('rss-rule-name-error'));
      assert.equal(await page.getByLabel('规则名称', { exact: true }).evaluate(node => node === document.activeElement), false, 'validation must not steal focus');
      await page.getByLabel('规则名称', { exact: true }).fill('纪录片规则改名（测试数据）');
      await ruleSave.scrollIntoViewIfNeeded();
      await ruleSave.click();
      await page.getByText('规则保存暂时失败，请重试', { exact: true }).waitFor();
      await localFeedback(page, ruleSaveArea, '规则保存暂时失败，请重试', ruleSave, `rule-save-error-${width}`);
      assert.equal(await page.getByLabel('规则名称', { exact: true }).inputValue(), '纪录片规则改名（测试数据）');
      await page.getByRole('button', { name: '保存并启用', exact: true }).click();
      await page.getByRole('button', { name: '纪录片规则改名（测试数据）', exact: true }).waitFor();
      const saves = state.writes.filter(r => r.path === '/api/rss/rules/1' && r.method === 'PUT');
      assert.equal(saves[0].body.request_id, saves[1].body.request_id); assert.equal(saves[1].body.expected_version, 4); assert.equal(saves[1].body.filters.hr_policy, 'require_clear');

      // Hash filters survive refresh; job actions follow the actual delivery state.
      await page.goto(`${url}/#/rss?tab=downloads&job=1`);
      const jobDialog = page.getByRole('dialog', { name: '下载任务详情', exact: true });
      await jobDialog.getByText('下载中', { exact: true }).waitFor();
      assert.equal(await jobDialog.getByRole('button', { name: '重试任务', exact: true }).count(), 0);
      assert.equal(await jobDialog.getByRole('button', { name: '取消未提交任务', exact: true }).count(), 0);
      await jobDialog.getByText('查看此任务使用的设置', { exact: true }).click();
      await capture(page, `job-${width}`);
      await page.getByRole('button', { name: '关闭下载任务详情', exact: true }).click();
      await page.getByRole('button', { name: '重试', exact: true }).click();
      await page.getByText('重试请求已保存。', { exact: true }).waitFor();
      assert.equal(state.jobs[1].status, 'queued');
      await page.getByRole('button', { name: `取消任务 ${state.jobs[1].title}`, exact: true }).click();
      await page.getByRole('button', { name: '确认取消任务', exact: true }).click();
      await page.getByText('未提交任务已取消。', { exact: true }).waitFor();
      assert.equal(state.jobs[1].status, 'cancelled');
      await page.getByRole('button', { name: '确认添加结果', exact: true }).click();
      assert(state.writes.some(r => r.path === '/api/rss/downloads/3/reconcile'));
      await page.getByLabel('搜索资源', { exact: true }).fill('Deep Ocean');
      await page.waitForURL(/q=Deep/);
      await page.reload();
      assert.equal(await page.getByLabel('搜索资源', { exact: true }).inputValue(), 'Deep Ocean');
      await capture(page, `downloads-${width}`);

      await page.goto(`${url}/#/rss`);
      await page.getByRole('button', { name: '暂停纪录片订阅（测试数据）', exact: true }).click();
      await page.getByRole('button', { name: '恢复纪录片订阅（测试数据）', exact: true }).waitFor();
      await page.getByRole('button', { name: '恢复纪录片订阅（测试数据）', exact: true }).click();
      await page.getByRole('dialog', { name: '恢复订阅源', exact: true }).getByRole('button', { name: '恢复检查', exact: true }).click();
      await page.getByRole('button', { name: '暂停纪录片订阅（测试数据）', exact: true }).waitFor();
      await page.getByRole('button', { name: '归档音乐精选（测试数据）', exact: true }).click();
      await page.getByRole('dialog', { name: '归档订阅源', exact: true }).getByRole('button', { name: '确认归档', exact: true }).click();
      await page.getByRole('button', { name: '音乐精选（测试数据）', exact: true }).waitFor({ state: 'hidden' });
      await page.getByRole('tab', { name: '下载规则', exact: true }).click();
      await page.getByRole('button', { name: '暂停纪录片规则改名（测试数据）', exact: true }).click();
      await page.getByRole('button', { name: '启用纪录片规则改名（测试数据）', exact: true }).click();
      await page.getByRole('dialog', { name: '启用下载规则', exact: true }).getByRole('button', { name: '启用规则', exact: true }).click();
      await page.getByRole('button', { name: '暂停纪录片规则改名（测试数据）', exact: true }).waitFor();
      assert(state.writes.some(r => r.method === 'DELETE' && r.path === '/api/rss/feeds/2'));

      // Unknown routes and raw URL leakage are failures, including on narrow screens.
      assert(!state.reads.some(path => path.includes('synthetic-only')));
      assert(!await page.locator('[data-rss-page]').innerText().then(text => text.includes('synthetic-only')));
      assert.deepEqual(errors, []);
      await page.close();
      console.log(`${width}px: source CRUD/test, draft+idempotency recovery, server preview, history backfill, run records, delivery recovery, hash persistence and overflow passed`);
    }

    const desktop = await browser.newPage({ viewport: { width: 1440, height: 1000 } });
    const desktopState = fixture(); desktopState.noDownloaders = true; desktopState.ruleSaveFail = false;
    await install(desktop, desktopState, true);
    await desktop.goto(`${url}/#/rss?tab=rules&edit=new&feed=1`);
    await desktop.getByLabel('规则名称', { exact: true }).fill('待配置规则（测试数据）');
    await desktop.getByLabel('标题包含', { exact: true }).fill('Documentary');
    for (const title of ['文件大小与做种人数（选填）', '高级标题匹配（正则）', '分类、标签与添加方式（选填）', '更多执行设置（优先级、磁盘空间）']) {
      assert.equal(await desktop.locator('summary').filter({ hasText: title }).evaluate(node => node.parentElement.open), false);
    }
    const executionSettings = desktop.locator('summary').filter({ hasText: '更多执行设置（优先级、磁盘空间）' });
    await executionSettings.click();
    await desktop.getByLabel('至少保留可用空间（GiB）', { exact: true }).fill('10');
    await desktop.getByLabel('至少保留可用空间（GiB）', { exact: true }).fill('0');
    assert.equal(await executionSettings.evaluate(node => node.parentElement.open), true, 'clearing an optional value must not close its section');
    await executionSettings.click();
    await desktop.getByLabel('标题不能包含（选填）', { exact: true }).fill('CAMRip');
    await desktop.getByRole('checkbox', { name: '接收所有标题', exact: true }).check();
    assert.equal(await desktop.getByLabel('标题包含', { exact: true }).count(), 0);
    await desktop.getByRole('button', { name: '查看匹配结果', exact: true }).click();
    const allPreview = desktopState.writes.filter(r => r.path.endsWith('/rules/preview')).at(-1).body.rule;
    assert.equal(allPreview.filters.match_all, true);
    assert.deepEqual(allPreview.filters.include, []);
    assert.equal(allPreview.filters.include_regex, null);
    assert.deepEqual(allPreview.filters.exclude, ['CAMRip']);
    await desktop.getByRole('checkbox', { name: '接收所有标题', exact: true }).uncheck();
    assert.equal(await desktop.getByLabel('标题包含', { exact: true }).inputValue(), 'Documentary', 'switching title mode must preserve the draft');
    assert(await desktop.getByRole('button', { name: '保存并启用', exact: true }).isDisabled());
    await desktop.getByRole('button', { name: '查看匹配结果', exact: true }).click();
    await desktop.getByText('信息不足 1', { exact: true }).waitFor();
    await desktop.getByRole('button', { name: '仅保存，暂不启用', exact: true }).click();
    await desktop.getByRole('button', { name: '待配置规则（测试数据）', exact: true }).waitFor();
    const disabledSave = desktopState.writes.find(r => r.path === '/api/rss/rules');
    assert.equal(disabledSave.body.enabled, false); assert.equal(disabledSave.body.downloader_id, null);
    await desktop.close();
    console.log('Desktop IPC: real API bridge, preview without downloader, disabled rule save passed');

    // Advancing an installed browser clock verifies polling cadence without waiting in real time.
    const polling = await browser.newPage({ viewport: { width: 1440, height: 1000 } });
    const pollingState = fixture();
    await install(polling, pollingState);
    await polling.clock.install();
    await polling.goto(`${url}/#/rss`);
    await polling.getByRole('button', { name: '纪录片订阅（测试数据）', exact: true }).waitFor();
    await polling.waitForFunction(() => !document.querySelector('button[aria-label="刷新列表"]')?.disabled);
    const initialReads = pollingState.reads.length;
    await polling.clock.runFor(6000);
    assert.equal(pollingState.reads.length, initialReads, 'idle sources should not poll at 5 seconds');
    await polling.clock.runFor(25000);
    await polling.waitForFunction(() => !document.querySelector('button[aria-label="刷新列表"]')?.disabled);
    assert(pollingState.reads.length > initialReads, 'idle sources poll at 30 seconds');
    await polling.evaluate(() => { Object.defineProperty(document, 'hidden', { configurable: true, value: true }); document.dispatchEvent(new Event('visibilitychange')); });
    const hiddenReads = pollingState.reads.length;
    await polling.clock.runFor(60000);
    assert.equal(pollingState.reads.length, hiddenReads, 'hidden pages must stop polling');
    await polling.evaluate(() => { Object.defineProperty(document, 'hidden', { configurable: true, value: false }); document.dispatchEvent(new Event('visibilitychange')); });
    await polling.goto(`${url}/#/rss?tab=downloads`);
    await polling.getByRole('button', { name: pollingState.jobs[0].title, exact: true }).waitFor();
    await polling.waitForFunction(() => !document.querySelector('button[aria-label="刷新列表"]')?.disabled);
    const activeReads = pollingState.reads.length;
    await polling.clock.runFor(5500);
    await polling.waitForFunction(() => !document.querySelector('button[aria-label="刷新列表"]')?.disabled);
    assert(pollingState.reads.length > activeReads, 'in-progress downloads poll at 5 seconds');
    await polling.goto(`${url}/#/rss?tab=rules&edit=1`);
    await polling.getByLabel('规则名称', { exact: true }).fill('保留未保存草稿');
    await polling.clock.runFor(31000);
    assert.equal(await polling.getByLabel('规则名称', { exact: true }).inputValue(), '保留未保存草稿');
    await polling.close();
    console.log('Polling: idle 30s, active 5s, hidden pause, editable draft preservation passed');
  } finally { await browser.close(); }
})().catch(error => { console.error(error); process.exitCode = 1; });
