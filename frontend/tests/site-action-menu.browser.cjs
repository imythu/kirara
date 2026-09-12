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
    const page = await browser.newPage({ viewport: { width: 1440, height: 1000 }, hasTouch: true });
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
    listFailure = false; overviewFailure = false;
    const menu = page.getByRole('menu', {name:'更多站点操作',exact:true});
    const arrow = page.getByRole('button', {name:'更多站点操作',exact:true});
    const hive = page.getByRole('button', {name:'同步到蜂巢',exact:true});
    async function closeDialog(name) { await page.getByRole('button',{name:`关闭${name}`,exact:true}).click(); await page.getByRole('dialog',{name,exact:true}).waitFor({state:'hidden'}); }
    for (const width of [1440,390,320]) {
      await page.setViewportSize({width,height:844});
      await page.goto(`${url}/#/sites`);
      await page.getByRole('button',{name:'编辑云海测试站',exact:true}).filter({visible:true}).waitFor();
      assert.equal(await page.evaluate(()=>document.documentElement.scrollWidth <= innerWidth),true);
      const mainBox=await hive.boundingBox(), arrowBox=await arrow.boundingBox();
      assert.equal(mainBox.y,arrowBox.y);
      assert.ok(Math.abs(mainBox.x+mainBox.width-arrowBox.x)<1,'split segments touch');
      assert.ok(arrowBox.width>=44 && arrowBox.height>=44,'arrow has touch target');
      const siteTop=await page.getByRole('button',{name:'编辑云海测试站',exact:true}).filter({visible:true}).evaluate(el=>el.getBoundingClientRect().top);
      await hive.click();
      await page.getByRole('dialog',{name:'同步到蜂巢',exact:true}).getByLabel('WebDAV 地址',{exact:true}).waitFor();
      assert.equal(await menu.count(),0,'main action never opens menu');
      await closeDialog('同步到蜂巢');
      await arrow.hover();
      await menu.waitFor();
      await menu.hover();
      await page.waitForTimeout(220);
      assert.equal(await menu.isVisible(),true,'pointer can reach menu');
      await page.mouse.move(1,1);
      await menu.waitFor({state:'hidden'});
      await arrow.tap();
      await menu.waitFor();
      assert.equal(await page.getByRole('button',{name:'编辑云海测试站',exact:true}).filter({visible:true}).evaluate(el=>el.getBoundingClientRect().top),siteTop,'menu does not shift content');
      const box=await menu.boundingBox();
      assert.ok(box.x>=0 && box.x+box.width<=width && box.y+box.height<=844,'menu fits viewport');
      await page.waitForTimeout(180);
      if(screenshots) {fs.mkdirSync(screenshots,{recursive:true});await page.screenshot({path:`${screenshots}/${width}-menu.png`});}
      await page.keyboard.press('End');
      assert.ok((await page.locator(':focus').innerText()).includes('导入 PTD 数据'));
      await page.keyboard.press('Home');
      assert.ok((await page.locator(':focus').innerText()).includes('全部同步'));
      await page.keyboard.press('Escape');
      assert.equal(await arrow.evaluate(el=>el===document.activeElement),true);
      await page.keyboard.press('ArrowUp');
      assert.ok((await page.locator(':focus').innerText()).includes('导入 PTD 数据'));
      await page.keyboard.press('Enter');
      await page.getByRole('dialog',{name:'导入 PTD 数据',exact:true}).waitFor();
      assert.equal(await menu.count(),0);
      await closeDialog('导入 PTD 数据');
      await arrow.tap();
      await menu.waitFor();
      await page.mouse.click(1,1);
      await menu.waitFor({state:'hidden'});
      if(screenshots) await page.screenshot({path:`${screenshots}/${width}-closed.png`});
    }
    assert.deepEqual(pageErrors,[]);
    console.log('PASS: split action isolation, hover bridge, touch, keyboard, outside click, dialog entry, no layout shift; 1440/390/320 geometry.');
  } finally {await browser.close();}
})().catch(error=>{console.error(error);process.exitCode=1;});
