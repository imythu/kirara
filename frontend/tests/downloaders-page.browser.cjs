// Run with PLAYWRIGHT_MODULE set if Playwright is installed outside this project.
// API fixtures cover multi-select filters and current file data versus lifetime traffic.
const { chromium } = require(process.env.PLAYWRIGHT_MODULE || 'playwright');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const base = process.env.DOWNLOADERS_TEST_URL || 'http://127.0.0.1:5173';
const output = process.env.DOWNLOADERS_SCREENSHOTS;
const torrent = (id, values) => ({hash: String(id).repeat(40), name: `种子 ${id}`, size: 1000, downloaded: 0, completed: 1000, amount_left: 0, incomplete: false, progress: 1, state: 'stalledUP', save_path: '/downloads/shared', added_on: id, category: '', tags: '', ...values});
const torrents = [
  torrent(1, {name: '辅种电影', category: '电影', tags: '保种, 收藏'}),
  torrent(2, {name: '重复下载剧集', category: '剧集', tags: '保种', downloaded: 3000, completed: 400, amount_left: 600, incomplete: true, progress: .4, state: 'downloading'}),
  torrent(3, {name: '仅收藏电影', category: '电影', tags: '收藏', save_path: '/downloads/other'}),
  torrent(4, {name: '无分类标签', size: 2000, completed: 2000}),
  torrent(5, {name: '磁力元数据', size: 0, completed: 0, incomplete: true, progress: 0, state: 'metaDL', category: '剧集', tags: '收藏'}),
];
(async () => {
  const browser = await chromium.launch({headless: true, args: ['--no-sandbox']});
  try {
    const page = await browser.newPage({locale: 'zh-CN'});
    page.setDefaultTimeout(7000);
    const errors = [];
    page.on('pageerror', error => errors.push(error.message));
    let fail = false;
    await page.route('**/api/**', route => {
      const path = new URL(route.request().url()).pathname;
      let body = {};
      if (path === '/api/features') body = {self_use: false};
      if (path === '/api/downloaders') body = [{id: 1, name: '测试下载器', downloader_type: 'qbittorrent', url: 'http://qb.example', username: 'admin', password_configured: true, created_at: '2026-09-01', updated_at: '2026-09-01'}];
      if (path.endsWith('/space')) body = {free_space: 10000, pending_download_bytes: 600, effective_free_space: 9400, torrent_count: 5, incomplete_count: 2};
      if (path.endsWith('/torrents')) body = fail ? {error: '下载器暂不可用'} : torrents;
      return route.fulfill({status: fail && path.endsWith('/torrents') ? 502 : 200, contentType: 'application/json', body: JSON.stringify(body)});
    });
    const analysis = page.getByRole('region', {name: '保存路径占用分析'});
    async function openDetail() {
      await page.goto(`${base}/#/downloaders`);
      await page.reload();
      await page.getByRole('button', {name: '详情', exact: true}).filter({visible: true}).first().click();
    }
    async function select(label, values) {
      await analysis.getByLabel(label, {exact: true}).click();
      const list = page.getByRole('listbox');
      assert.equal(await list.getAttribute('aria-multiselectable'), 'true');
      for (const value of values) {
        await list.getByRole('option', {name: value, exact: true}).click();
        assert.equal(await list.getByRole('option', {name: value, exact: true}).getAttribute('aria-selected'), 'true');
      }
      await page.keyboard.press('Escape');
    }
    async function run(count, pending, incomplete) {
      await analysis.getByRole('button', {name: /^(开始统计|重新统计)$/}).click();
      await analysis.getByRole('button', {name: '重新统计', exact: true}).waitFor();
      const summary = await analysis.getByText('种子总数', {exact: true}).locator('..').innerText();
      assert.match(summary, new RegExp(`${count} 个`));
      const remaining = await analysis.getByText('待下载', {exact: true}).locator('..').innerText();
      assert.ok(remaining.includes(pending), remaining);
      assert.ok(remaining.includes(`${incomplete} 个未完成`), remaining);
    }
    for (const width of [1440, 390]) {
      await page.setViewportSize({width, height: 1000});
      await openDetail();
      await analysis.getByLabel('分类（多选）', {exact: true}).waitFor();
      await select('分类（多选）', ['电影', '剧集']);
      await select('标签（多选）', ['保种']);
      await run(2, '600.00 B', 1);
      await analysis.getByText('重复下载剧集', {exact: true}).waitFor();
      assert.equal(await analysis.getByText('无分类标签', {exact: true}).count(), 0);
      assert.equal(await analysis.getByText('仅收藏电影', {exact: true}).count(), 0);
      assert.ok((await analysis.innerText()).includes('400.00 B / 1000.00 B'));
      if (output) {
        fs.mkdirSync(output, {recursive: true});
        await analysis.screenshot({path: `${output}/${width}-filtered.png`});
        await analysis.getByLabel('标签（多选）', {exact: true}).click();
        await page.screenshot({path: `${output}/${width}-multi-select.png`});
        await page.keyboard.press('Escape');
      }
      await select('标签（多选）', ['收藏']);
      assert.equal(await analysis.getByText('种子总数', {exact: true}).count(), 0, 'old results must clear when filters change');
      await run(4, '600.00 B', 2);
      await analysis.getByRole('button', {name: '清空筛选', exact: true}).click();
      await select('分类（多选）', ['未分类']);
      await select('标签（多选）', ['无标签']);
      await run(1, '0 B', 0);
      await analysis.getByRole('button', {name: '清空筛选', exact: true}).click();
      await select('分类（多选）', ['电影']);
      await select('标签（多选）', ['无标签']);
      await run(0, '0 B', 0);
      await analysis.getByText('所选标签和分类下暂无种子。', {exact: true}).waitFor();
      await analysis.getByRole('button', {name: '清空筛选', exact: true}).click();
      await run(5, '600.00 B', 2);
      assert.equal(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), true);
    }
    fail = true;
    await openDetail();
    await analysis.getByRole('alert').waitFor();
    assert.equal(await analysis.getByRole('button', {name: '开始统计', exact: true}).isDisabled(), true);
    fail = false;
    await analysis.getByRole('button', {name: '重试加载', exact: true}).click();
    await run(5, '600.00 B', 2);
    await analysis.getByLabel('标签（多选）', {exact: true}).click();
    const search = page.getByRole('searchbox', {name: '搜索标签', exact: true});
    await search.fill('保种');
    await search.press('Enter');
    assert.equal(await page.getByRole('option', {name: '保种', exact: true}).getAttribute('aria-selected'), 'true');
    await search.press('Enter');
    assert.equal(await page.getByRole('option', {name: '保种', exact: true}).getAttribute('aria-selected'), 'false');
    await search.press('Escape');
    assert.equal(await analysis.getByLabel('标签（多选）', {exact: true}).evaluate(el => el === document.activeElement), true);
    assert.deepEqual(errors, []);
    console.log('PASS: pre-analysis multi-select, OR/AND matching, no tags/category, stale results, counts, metadata, mobile and retry');
  } finally { await browser.close(); }
})().catch(error => {console.error(error); process.exit(1);});
