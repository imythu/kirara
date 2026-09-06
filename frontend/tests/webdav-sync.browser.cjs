// Mocked requests only: never contacts PT sites or modifies the user's database.
// WEBDAV_TEST_URL=http://127.0.0.1:4179 PLAYWRIGHT_MODULE=/path/to/playwright node frontend/tests/webdav-sync.browser.cjs
const { chromium } = require(process.env.PLAYWRIGHT_MODULE || 'playwright');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const output = process.env.WEBDAV_SCREENSHOTS || '/tmp/kirara-webdav-ui';
const origin = process.env.WEBDAV_TEST_URL || 'http://127.0.0.1:4179';
const now = Date.now();
(async () => {
  fs.mkdirSync(output, { recursive: true });
  const browser = await chromium.launch({ headless: true, args: ['--no-sandbox'] });
  try {
    for (const width of [1440, 390]) {
      const page = await browser.newPage({ viewport: { width, height: 1000 } });
      page.setDefaultTimeout(8000);
      const errors = [], writes = [];
      let config = { enabled: false, username: 'ptd', password_configured: false, existing_policy: 'update', auto_create: true, running: false, runtime_error: null, last_received_at: null };
      let loadFailure = true, saveFailure = false, retryFailure = true, history = [];
      page.on('pageerror', error => errors.push(error.message));
      await page.route('**/api/**', async route => {
        const req = route.request(), path = new URL(req.url()).pathname;
        let body = {}, status = 200;
        if (path === '/api/sites/webdav-sync') {
          if (req.method() === 'PUT') {
            const input = req.postDataJSON(); writes.push(input);
            if (saveFailure) { status = 500; body = { error: '模拟保存失败，请重试' }; }
            else {
              config = { ...config, ...input, password_configured: true, running: input.enabled };
              delete config.rotate_password;
              body = { ...config, ...(input.rotate_password ? { new_password: 'a'.repeat(64) } : {}) };
            }
          } else if (loadFailure) { status = 500; body = { error: '模拟加载失败' }; }
          else body = config;
        } else if (path === '/api/sites/webdav-sync/runs') body = history;
        else if (path.endsWith('/retry')) {
          if (retryFailure) { status = 500; body = { error: '模拟重试失败，请再试一次' }; }
          else { history = history.map(run => ({ ...run, status: 'pending' })); body = { ok: true }; }
        }
        else if (path.endsWith('/search')) body = { items: [], total: 0, page: 1, page_size: 20, parsed_filters: [], semantic_status: 'not_needed' };
        else if (path === '/api/sites/ptd-backup') body = { enabled: false, configured: false, password_configured: false, webdav_url: '', username: '', backup_interval_hours: 24, site_identifiers: {} };
        else if (path === '/api/features') body = { self_use: false };
        else if (path.includes('/sites')) body = [];
        await route.fulfill({ status, contentType: 'application/json', body: JSON.stringify(body) });
      });
      await page.goto(`${origin}/#/sites`);
      await page.getByRole('button', { name: '备份与同步', exact: true }).click();
      const panel = page.getByRole('region', { name: 'WebDAV 接收' });
      await panel.getByText('模拟加载失败').waitFor();
      assert.equal(await panel.getByRole('button', { name: '保存接收设置' }).count(), 0);
      loadFailure = false;
      await panel.getByRole('button', { name: '重新加载', exact: true }).click();
      await panel.getByLabel('接收用户名', { exact: true }).waitFor();
      assert.equal(await panel.getByLabel('端口', { exact: true }).count(), 0);
      await panel.getByText(new URL('/dav/ptd/', page.url()).href, { exact: true }).waitFor();
      await panel.getByLabel('启用 WebDAV 接收').check();
      await panel.getByLabel('接收用户名', { exact: true }).fill('3456');
      await panel.getByRole('button', { name: '刷新同步记录', exact: true }).click();
      await page.waitForTimeout(150);
      assert.equal(await panel.getByLabel('接收用户名', { exact: true }).inputValue(), '3456', 'polling preserves unsaved edits');
      saveFailure = true;
      await panel.getByRole('button', { name: '保存接收设置' }).click();
      await panel.getByText('模拟保存失败，请重试').waitFor();
      assert.equal(await panel.getByLabel('接收用户名', { exact: true }).inputValue(), '3456');
      saveFailure = false;
      await panel.getByRole('button', { name: '保存接收设置' }).click();
      await panel.getByLabel('本次生成的密码', { exact: true }).waitFor();
      assert.equal(writes.at(-1).rotate_password, true);
      assert.equal(writes.at(-1).username, '3456');
      assert.equal(await panel.getByLabel('本次生成的密码', { exact: true }).inputValue(), 'a'.repeat(64));
      await panel.getByRole('button', { name: '保存接收设置' }).click();
      await page.waitForTimeout(150);
      assert.equal(writes.at(-1).rotate_password, false);
      history = [
        { id: 2, name: 'PTD_backup_fixture.zip', received_at: now, status: 'done', error: null, can_retry: true, result: { created: 1, updated: 3, unchanged: 8, skipped: 1, details: ['示例站：现有认证方式不使用 Cookie'] } },
        { id: 1, name: 'cookies.json', received_at: now - 60_000, status: 'failed', error: '数据库处理失败，已回滚本次站点变更', can_retry: true, result: null },
      ];
      config = { ...config, last_received_at: now };
      await panel.getByRole('button', { name: '刷新同步记录', exact: true }).click();
      await panel.getByText('新增 1 · 更新 3 · 未变化 8 · 跳过 1', { exact: true }).waitFor();
      await panel.getByText('查看跳过原因', { exact: true }).click();
      await panel.getByText('示例站：现有认证方式不使用 Cookie').waitFor();
      assert.equal(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), true);
      const dialog = page.getByRole('dialog', { name: '备份与同步', exact: true });
      // Capture both the top controls and the lower result area inside the scrolling dialog.
      await panel.getByLabel('接收用户名', { exact: true }).scrollIntoViewIfNeeded();
      const saveBox = await panel.getByRole('button', { name: '保存接收设置' }).boundingBox();
      assert(saveBox && saveBox.y + saveBox.height <= 1000, 'save action fully visible in settings viewport');
      await page.screenshot({ path: `${output}/${width}-settings.png`, fullPage: true });
      await panel.getByRole('button', { name: '重新同步', exact: true }).scrollIntoViewIfNeeded();
      await page.screenshot({ path: `${output}/${width}-results.png`, fullPage: true });
      await panel.getByRole('button', { name: '重新同步', exact: true }).click();
      await panel.getByRole('alert').filter({ hasText: '模拟重试失败，请再试一次' }).waitFor();
      const retryRow = panel.getByRole('listitem').filter({ has: page.getByRole('button', { name: '重新同步', exact: true }) });
      assert.equal(await retryRow.getByRole('alert').innerText(), '模拟重试失败，请再试一次');
      await page.screenshot({ path: `${output}/${width}-retry-error.png`, fullPage: true });
      retryFailure = false;
      await panel.getByRole('button', { name: '重新同步', exact: true }).click();
      await panel.getByText('等待同步', { exact: true }).first().waitFor();
      await dialog.getByRole('button', { name: '账户数据备份', exact: true }).click();
      await dialog.getByText('蜂巢 PTD 备份', { exact: true }).waitFor();
      await dialog.getByRole('button', { name: 'Cookie 自动同步', exact: true }).click();
      await panel.getByLabel('接收用户名', { exact: true }).waitFor();
      assert.equal(await panel.getByLabel('本次生成的密码', { exact: true }).count(), 0, 'password not recovered from GET');
      await page.keyboard.press('Escape');
      await dialog.waitFor({ state: 'hidden' });
      assert.deepEqual(errors, []);
      console.log(`${width}: loading recovery, save retry, polling, one-time password, sync history and tabs passed`);
      await page.close();
    }
  } finally { await browser.close(); }
})().catch(error => { console.error(error); process.exit(1); });
