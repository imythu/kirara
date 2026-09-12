// Run with Playwright installed: SITES_TEST_URL=http://127.0.0.1:5173 node frontend/tests/sites-page.browser.cjs
// Every API request is intercepted; this test never reads or changes real site data.
const { chromium } = require(process.env.PLAYWRIGHT_MODULE || 'playwright');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const url = process.env.SITES_TEST_URL || 'http://127.0.0.1:5173';
const screenshots = process.env.SITES_SCREENSHOTS;
const now = '2026-09-05T10:00:00Z';
const stats = { site_id: 1, uid: '12345', username: '测试账户', uploaded: 8e12, downloaded: 2e12, ratio: 4, bonus: 12345, seeding_count: 20, leeching_count: 0, updated_at: now, last_checked_at: now, last_error: null };
const sites = [
  { id: 1, name: '云海测试站', site_type: 'nexusphp', base_url: 'https://site.example.test', auth_type: 'cookie', auth_configured: true, use_proxy: false, stats },
  { id: 2, name: '连接失败测试站', site_type: 'mteam', base_url: 'https://failed.example.test', auth_type: 'api_key', auth_configured: true, use_proxy: true, stats: { ...stats, site_id: 2, last_error: '认证已失效，请更新凭据后重新测试连接。' } },
  { id: 3, name: '等待刷新测试站', site_type: 'gazelle', base_url: 'https://pending.example.test', auth_type: 'cookie', auth_configured: false, use_proxy: false, stats: null },
];
(async () => {
  const browser = await chromium.launch({ headless: true, args: ['--no-sandbox'] });
  try {
    const page = await browser.newPage({ viewport: { width: 1440, height: 1000 } });
    page.setDefaultTimeout(7000);
    const pageErrors = [], writes = [];
    let siteListReads = 0;
    let listFailure = true, overviewFailure = true, headersFailure = false, saveFailure = false, deleteFailure = true, empty = false;
    page.on('pageerror', error => pageErrors.push(error.message));
    await page.route('**/api/**', async route => {
      const request = route.request(), path = new URL(request.url()).pathname;
      let body = {}, status = 200;
      if (request.method() !== 'GET') writes.push({ path, method: request.method(), body: request.postData() ? request.postDataJSON() : null });
      if (path === '/api/sites') {
        if (request.method() === 'POST') { body = saveFailure ? { error: '保存失败，请稍后重试' } : { id: 4 }; status = saveFailure ? 500 : 200; }
        else { siteListReads++; body = listFailure ? { error: '测试服务暂时不可用' } : empty ? [] : sites; status = listFailure ? 500 : 200; }
      } else if (path === '/api/sites/search') {
        const query = new URL(request.url()).searchParams.get('q') || '';
        const records = empty ? [] : sites.filter(site => site.name.includes(query));
        body = { items: records.map(record => ({ record, matched_by: [] })), total: records.length, page: 1, page_size: 20, parsed_filters: [], semantic_status: 'not_used' };
      } else if (path === '/api/sites/stats-overview') { body = overviewFailure ? { error: '总览服务暂时不可用' } : sites; status = overviewFailure ? 500 : 200; }
      else if (path === '/api/sites/catalog') body = [{ ptd_id: 'demo', name: '预设测试站', base_url: 'https://preset.example.test', aliases: ['演示'], site_type: 'nexusphp' }];
      else if (path === '/api/sites/ptd-backup') body = { configured: false, enabled: false, password_configured: false, webdav_url: '', username: '', backup_interval_hours: 24, site_identifiers: { '1': 'demo' }, last_error: null };
      else if (path.endsWith('/request-headers')) { body = headersFailure ? { error: '请求头服务暂时不可用' } : [{ name: 'X-Saved', value: 'preserve-me' }]; status = headersFailure ? 500 : 200; }
      else if (path.endsWith('/credentials')) { body = { error: '凭据暂时无法读取' }; status = 500; }
      else if (path.endsWith('/sync')) body = path === '/api/sites/1/sync'
        ? { success: true, message: '站点账户数据已同步', user_stats: stats }
        : { success: false, message: '测试认证失败，请更新凭据', user_stats: null };
      else if (request.method() === 'DELETE') { body = deleteFailure ? { error: '删除失败，请重试' } : { ok: true }; status = deleteFailure ? 500 : 200; }
      else if (request.method() === 'PUT') body = { ok: true };
      else if (path === '/api/sites/refresh-all') body = { refreshing: false };
      await route.fulfill({ status, contentType: 'application/json', body: JSON.stringify(body) });
    });
    const visibleButton = name => page.getByRole('button', { name, exact: true }).filter({ visible: true });
    const dialog = name => page.getByRole('dialog', { name, exact: true });
    async function close(name) { await dialog(name).getByRole('button', { name: `关闭${name}`, exact: true }).click(); await dialog(name).waitFor({ state: 'hidden' }); }
    async function capture(name) { if (screenshots) { fs.mkdirSync(screenshots, { recursive: true }); await page.screenshot({ path: `${screenshots}/${name}.png`, fullPage: true }); } }
    async function noOverflow(width) {
      assert.equal(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), true, `page overflow at ${width}`);
    }
    await page.goto(`${url}/#/sites`);
    await page.getByText('站点列表加载失败', { exact: true }).waitFor();
    assert.equal(await page.getByText('还没有配置 PT 站点', { exact: true }).count(), 0);
    listFailure = false;
    await visibleButton('重新加载').click();
    await visibleButton('编辑云海测试站').waitFor();
    await noOverflow(1440);
    const table = page.locator('table').filter({ visible: true }).first();
    assert.equal(await table.evaluate(el => el.scrollWidth <= el.clientWidth), true, 'table fits');
    assert.equal(await visibleButton('云海测试站的更多操作').evaluate(el => el.getBoundingClientRect().right < innerWidth), true);
    await capture('desktop');
    await visibleButton('更多站点操作').click();
    await page.getByRole('menuitem', { name: /数据总览/ }).click();
    await dialog('PT 数据总览').getByText('站点总览加载失败', { exact: true }).waitFor();
    assert.equal(await dialog('PT 数据总览').getByText('暂无站点统计数据', { exact: true }).count(), 0);
    overviewFailure = false;
    await dialog('PT 数据总览').getByRole('button', { name: '重新加载总览' }).click();
    const accountDataMetric = dialog('PT 数据总览').getByText('有账户数据', { exact: true }).locator('..').locator('..');
    await accountDataMetric.getByText('2', { exact: true }).waitFor();
    assert.equal(await accountDataMetric.getByText('/ 3', { exact: true }).innerText(), '/ 3');
    await close('PT 数据总览');
    await page.getByLabel('搜索站点', { exact: true }).fill('不存在');
    await visibleButton('清除筛选').click();
    assert.equal(await page.getByLabel('搜索站点', { exact: true }).inputValue(), '');
    const readsBeforeSync = siteListReads;
    await visibleButton('同步云海测试站账户数据').click();
    await dialog('同步站点').getByText('站点账户数据已同步', { exact: true }).waitFor();
    await close('同步站点');
    await visibleButton('同步云海测试站账户数据').waitFor();
    assert.ok(siteListReads > readsBeforeSync, 'sync reloads persisted site statistics');
    assert.ok(writes.some(write => write.path === '/api/sites/1/sync' && write.method === 'POST'));
    assert.ok(!writes.some(write => /^\/api\/sites\/\d+\/test$/.test(write.path)));
    await visibleButton('同步连接失败测试站账户数据').click();
    await dialog('同步站点').getByText('测试认证失败，请更新凭据', { exact: true }).waitFor();
    await dialog('同步站点').getByRole('button', { name: '重新同步' }).click();
    await dialog('同步站点').getByRole('button', { name: '编辑连接配置' }).click();
    await dialog('编辑站点').getByLabel('API Key', { exact: true }).waitFor();
    await close('编辑站点');
    await visibleButton('添加站点').click();
    let form = dialog('添加站点');
    assert.equal(await form.locator('details').getAttribute('open'), null, 'advanced collapsed');
    await form.getByRole('button', { name: '添加', exact: true }).focus();
    await page.keyboard.press('Tab');
    assert.equal(await page.evaluate(() => document.activeElement.getAttribute('aria-label')), '关闭添加站点', 'fixed footer remains in focus trap');
    await form.locator('#site-auth-type').focus();
    await page.keyboard.press('ArrowDown');
    await page.waitForFunction(() => document.activeElement?.getAttribute('role') === 'option');
    await page.keyboard.press('ArrowDown');
    await page.keyboard.press('Enter');
    await form.getByLabel('Passkey', { exact: true }).waitFor();
    await form.locator('#site-auth-type').click();
    await page.getByRole('option', { name: 'Cookie', exact: true }).click();
    const cookie = form.getByLabel('Cookie', { exact: true });
    assert.equal(await cookie.getAttribute('type'), 'password');
    assert.equal(await cookie.evaluate(el => el.labels.length), 1);
    await form.getByRole('button', { name: '显示Cookie', exact: true }).click();
    assert.equal(await cookie.getAttribute('type'), 'text');
    await form.getByRole('button', { name: '添加', exact: true }).click();
    await form.getByText('请填写站点名称', { exact: true }).waitFor();
    assert.equal(await page.evaluate(() => document.activeElement.id), 'site-name');
    await form.getByLabel('名称', { exact: true }).fill('新建测试站');
    await form.getByLabel('基础 URL', { exact: true }).fill('javascript:alert(1)');
    await form.getByRole('button', { name: '添加', exact: true }).click();
    assert.equal(await page.evaluate(() => document.activeElement.id), 'site-base-url');
    await form.getByLabel('基础 URL', { exact: true }).fill('https://new.example.test');
    await cookie.fill('fixture-cookie');
    await form.locator('summary').click();
    assert.equal(await form.getByLabel('第 1 个请求头名称', { exact: true }).inputValue(), 'Accept');
    await form.locator('summary').click();
    saveFailure = true;
    await form.getByRole('button', { name: '添加', exact: true }).click();
    await form.getByRole('alert').getByText('保存失败，请稍后重试', { exact: true }).waitFor();
    assert.equal(await cookie.inputValue(), 'fixture-cookie', 'failed save preserves input');
    saveFailure = false;
    await form.getByRole('button', { name: '添加', exact: true }).click();
    await form.waitFor({ state: 'hidden' });
    const creation = writes.filter(row => row.path === '/api/sites' && row.method === 'POST').at(-1);
    assert.equal(creation.body.request_headers.length, 14, 'collapsed headers still saved');
    assert.equal(creation.body.auth_config.cookie, 'fixture-cookie');
    await visibleButton('添加站点').click();
    form = dialog('添加站点');
    await form.locator('#site-preset').click();
    await page.getByRole('option').filter({ hasText: '预设测试站' }).click();
    assert.equal(await form.getByLabel('基础 URL', { exact: true }).inputValue(), 'https://preset.example.test');
    await capture('add-desktop');
    await page.keyboard.press('Escape');
    await form.waitFor({ state: 'hidden' });
    headersFailure = true;
    await visibleButton('编辑云海测试站').click();
    form = dialog('编辑站点');
    await form.getByText('请求头加载失败，暂时无法保存', { exact: true }).waitFor();
    assert.equal(await form.getByRole('button', { name: '保存', exact: true }).isDisabled(), true);
    await form.getByLabel('名称', { exact: true }).fill('保留草稿');
    headersFailure = false;
    await form.getByRole('button', { name: '重新加载请求头' }).click();
    await page.waitForFunction(() => !document.querySelector('button[form="site-connection-form"]').disabled);
    assert.equal(await form.getByLabel('名称', { exact: true }).inputValue(), '保留草稿');
    await form.getByRole('button', { name: '保存', exact: true }).click();
    await form.waitFor({ state: 'hidden' });
    const edit = writes.find(row => row.path === '/api/sites/1' && row.method === 'PUT');
    assert.equal(edit.body.clear_auth_config, false);
    assert.deepEqual(edit.body.auth_config, { auth_type: 'cookie', cookie: '' }, 'blank credentials preserve the existing backend contract');
    assert.deepEqual(edit.body.request_headers, [{ name: 'X-Saved', value: 'preserve-me' }]);
    await visibleButton('编辑云海测试站').click();
    form = dialog('编辑站点');
    await page.waitForFunction(() => !document.querySelector('button[form="site-connection-form"]').disabled);
    await form.getByLabel('清除已保存的认证凭据', { exact: true }).check();
    listFailure = true;
    await form.getByRole('button', { name: '保存', exact: true }).click();
    await form.waitFor({ state: 'hidden' });
    assert.equal(writes.filter(row => row.path === '/api/sites/1' && row.method === 'PUT').at(-1).body.clear_auth_config, true);
    await page.getByText('站点列表加载失败', { exact: true }).waitFor();
    await visibleButton('编辑云海测试站').waitFor();
    assert.equal(await page.getByText('还没有配置 PT 站点', { exact: true }).count(), 0, 'reload failure retains old data');
    listFailure = false;
    await visibleButton('重新加载').click();
    await page.getByText('站点列表加载失败', { exact: true }).waitFor({ state: 'hidden' });
    await visibleButton('云海测试站的更多操作').click();
    await dialog('云海测试站 · 更多操作').getByRole('button', { name: '查看凭据', exact: true }).click();
    await dialog('站点凭据').getByRole('button', { name: '显示云海测试站的Cookie', exact: true }).click();
    await dialog('站点凭据').getByText('凭据暂时无法读取', { exact: true }).waitFor();
    await close('站点凭据');
    await visibleButton('云海测试站的更多操作').click();
    await dialog('云海测试站 · 更多操作').getByRole('button', { name: '删除站点', exact: true }).click();
    await dialog('确认删除').getByRole('button', { name: '删除', exact: true }).click();
    await dialog('确认删除').getByRole('alert').waitFor();
    await close('确认删除');
    for (const width of [390, 320, 768, 1024]) {
      await page.setViewportSize({ width, height: 844 });
      await noOverflow(width);
      const failedCard = page.getByRole('article', { name: '连接失败测试站', exact: true });
      if ((await failedCard.locator('details').getAttribute('open')) === null) await failedCard.locator('summary').click();
      await failedCard.getByText('认证已失效，请更新凭据后重新测试连接。', { exact: true }).waitFor();
      assert.equal(await failedCard.getByRole('button', { name: '编辑连接失败测试站', exact: true }).evaluate(el => el.getBoundingClientRect().height >= 44), true);
      if (width === 390) await capture('mobile');
      await visibleButton('添加站点').click();
      form = dialog('添加站点');
      assert.equal(await form.evaluate(el => el.scrollWidth <= el.clientWidth), true, `form fits ${width}`);
      const submit = form.getByRole('button', { name: '添加', exact: true });
      assert.equal(await submit.evaluate(el => el.getBoundingClientRect().bottom <= innerHeight), true, 'submit visible');
      if (width === 390) await capture('add-mobile');
      await form.locator('summary').click();
      await form.getByRole('button', { name: '添加请求头', exact: true }).click();
      assert.equal(await submit.evaluate(el => el.getBoundingClientRect().bottom <= innerHeight), true, 'footer fixed with advanced expanded');
      await noOverflow(width);
      await close('添加站点');
    }
    empty = true;
    await page.reload();
    await page.getByText('还没有配置 PT 站点', { exact: true }).waitFor();
    assert.deepEqual(pageErrors, []);
    console.log('PASS: list/overview recovery; validation; preset; hidden headers; credential preservation; modal errors; desktop/mobile/tablet geometry and fixed footer.');
  } finally { await browser.close(); }
})().catch(error => { console.error(error); process.exitCode = 1; });
