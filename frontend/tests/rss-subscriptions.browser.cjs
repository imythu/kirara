// Synthetic UI contract tests; atomic persistence and matching are tested in Rust.
const { chromium } = require(process.env.PLAYWRIGHT_MODULE || 'playwright');
const { fixture, responder, noOverflow } = require('./rss.browser.cjs');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const url = process.env.RSS_TEST_URL || 'http://localhost:1234';
const output = path.resolve('.impeccable/review/rss-redesign');
(async () => {
  fs.mkdirSync(output, { recursive: true });
  const browser = await chromium.launch({ headless: true, args: ['--no-sandbox'] });
  try {
    for (const width of [1440, 390]) {
      const page = await browser.newPage({ viewport: { width, height: 1000 } });
      const state = fixture(), respond = responder(state), saves = [], errors = [];
      let failSave = true;
      page.on('pageerror', e => errors.push(e.message));
      await page.route('**/api/**', async route => {
        const req = route.request(), address = new URL(req.url());
        const body = req.postDataJSON();
        let result;
        if (address.pathname === '/api/rss/subscriptions/preview') {
          result = await respond('/api/rss/rules/preview', 'POST', { rule: { filters: body.filters }, item_ids: [] });
        } else if (/\/subscriptions(?:\/\d+)?$/.test(address.pathname)) {
          if (req.method() === 'GET') result = { status: 200, data: { feed: state.feeds[0], rule: state.rules[0] } };
          else {
            saves.push(body);
            if (failSave) { failSave = false; result = { status: 503, data: { error: '保存失败，请重试；订阅未创建' } }; }
            else { const feed = { ...state.feeds[0], ...body.feed, id: req.method() === 'PUT' ? 1 : 3, version: 5 }; const rule = { ...state.rules[0], ...body.rule, id: 3, feed_ids: [feed.id], version: 5 }; if (req.method() === 'POST') { state.feeds.push(feed); state.rules.push(rule); } result = { status: 200, data: { feed, rule } }; }
          }
        } else result = await respond(address.pathname + address.search, req.method(), req.postData());
        await route.fulfill({ status: result.status, contentType: 'application/json', body: result.status === 204 ? '' : JSON.stringify(result.data) });
      });
      await page.goto(`${url}/#/rss`);
      await page.getByRole('button', { name: '查看资源', exact: true }).first().waitFor();
      await noOverflow(page);
      await page.screenshot({ path: path.join(output, `${width === 1440 ? 'desktop' : 'mobile'}.png`), fullPage: true });
      await page.getByRole('button', { name: '添加订阅', exact: true }).click();
      await page.getByRole('button', { name: '下一步：下载条件' }).click();
      assert.equal(await page.getByLabel('RSS 链接', { exact: true }).getAttribute('aria-invalid'), 'true');
      await page.getByLabel('RSS 链接', { exact: true }).fill('https://tracker.test/rss?passkey=test');
      await page.getByLabel('订阅名称', { exact: true }).fill('自然纪录片（测试数据）');
      await page.getByRole('button', { name: '下一步：下载条件' }).click();
      await page.getByLabel('排除关键词（选填）').fill('CAM, 预告');
      await page.getByRole('checkbox', { name: '只下载免费资源', exact: true }).check();
      await page.getByLabel('保存目录（选填）').fill('/downloads/nature');
      await noOverflow(page);
      await page.locator('.kirara-content').evaluate(n => n.scrollTo(0, 0));
      await page.screenshot({ path: path.join(output, `conditions-${width}.png`), fullPage: true });
      await page.getByRole('button', { name: '下一步：预览与确认' }).click();
      await page.getByRole('button', { name: '测试链接并预览' }).click();
      await page.getByRole('status').filter({ hasText: '读取 3 条' }).waitFor();
      assert.equal(saves.length, 0, 'preview must not save');
      await page.locator('.kirara-content').evaluate(n => n.scrollTo(0, 0));
      await noOverflow(page);
      await page.screenshot({ path: path.join(output, `preview-${width}.png`), fullPage: true });
      await page.getByRole('button', { name: '保存并开启订阅' }).click();
      await page.getByRole('alert').filter({ hasText: '保存失败' }).waitFor();
      await page.getByRole('button', { name: '保存并开启订阅' }).click();
      await page.getByRole('heading', { name: '自然纪录片（测试数据）', exact: true }).waitFor();
      assert.equal(saves.length, 2);
      assert.equal(saves[0].request_id, saves[1].request_id, 'unchanged retries reuse request ID');
      assert.equal(saves[1].rule.downloader_id, 1);
      assert.deepEqual(saves[1].rule.filters.exclude, ['CAM', '预告']);
      assert.equal(saves[1].rule.filters.free_only, true);
      assert.equal(saves[1].rule.options.save_path, '/downloads/nature');
      await page.goto(`${url}/#/rss?subscription=1`);
      await page.getByRole('heading', { name: /设置订阅/ }).waitFor();
      assert.equal(await page.locator('input[type=url]').count(), 0, 'saved URL stays masked');
      await page.getByLabel('订阅名称', { exact: true }).fill('修改后的订阅');
      await page.getByRole('button', { name: '返回我的订阅', exact: true }).click();
      await page.getByRole('dialog', { name: '放弃本次修改？' }).waitFor();
      await page.getByRole('button', { name: '继续编辑', exact: true }).click();
      assert.equal(await page.getByLabel('订阅名称', { exact: true }).inputValue(), '修改后的订阅');
      await page.getByRole('button', { name: '下一步：下载条件' }).click();
      await page.getByRole('button', { name: '下一步：预览与确认' }).click();
      await page.getByRole('checkbox', { name: '保存后开启订阅', exact: true }).uncheck();
      await page.getByRole('button', { name: '保存为已暂停' }).click();
      await page.getByRole('heading', { name: '修改后的订阅', exact: true }).waitFor();
      assert.equal(saves[2].feed.enabled, false);
      assert.equal(saves[2].feed.expected_version, 3);
      assert.equal(saves[2].rule.expected_version, 4);
      await page.goto(`${url}/#/rss?tab=downloads`);
      await page.getByRole('button', { name: '重试', exact: true }).click();
      await page.getByRole('status').filter({ hasText: '重试请求已保存' }).waitFor();
      assert(state.writes.some(write => write.path === '/api/rss/downloads/2/retry'));
      assert.deepEqual(errors, []);
      await page.close();
      console.log(`${width}px: create, validation, preview, retry identity, edit, discard, pause, download recovery passed`);
    }
  } finally { await browser.close(); }
})().catch(e => { console.error(e); process.exitCode = 1; });
