// Synthetic files and mocked APIs only.
const { chromium } = require(process.env.PLAYWRIGHT_MODULE || 'playwright');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const output = '/workspace/rflush/.impeccable/review/ptd-import';
(async () => {
 fs.mkdirSync(output,{recursive:true});
 const browser = await chromium.launch({headless:true,args:['--no-sandbox']});
 try {
  for (const width of [1440,390]) {
   const page = await browser.newPage({viewport:{width,height:1000}});
   const errors=[],writes=[]; let fail=true;
   page.on('pageerror',e=>errors.push(e.message));
   await page.route('**/api/**',async route=>{
    const path=new URL(route.request().url()).pathname; let body={},status=200;
    if(path==='/api/sites/ptd-import') {
     writes.push(route.request().postDataJSON());
     await new Promise(r=>setTimeout(r,200));
     if(fail){status=422;body={error:'备份未包含 cookies.json，请在 PTD 中勾选 Cookie'};}
     else body={created:1,updated:1,unchanged:1,skipped:1,details:['测试站点：Cookie 不适用于现有访问域名']};
    } else if(path.endsWith('/search')) body={items:[],total:0,page:1,page_size:20,parsed_filters:[],semantic_status:'not_needed'};
    else if(path==='/api/features') body={self_use:false};
    else if(path.includes('/sites')) body=[];
    await route.fulfill({status,contentType:'application/json',body:JSON.stringify(body)});
   });
   await page.goto('http://127.0.0.1:4179/#/sites');
   await page.getByRole('button',{name:'导入 PTD 配置',exact:true}).click();
   const panel=page.getByRole('region',{name:'PTD 配置导入'});
   const submit=panel.getByRole('button',{name:'开始导入',exact:true});
   assert(await submit.isDisabled());
   await panel.getByLabel('PTD 备份文件',{exact:true}).setInputFiles({name:'empty.json',mimeType:'application/json',buffer:Buffer.alloc(0)});
   await panel.getByText('请选择非空且不超过 8 MiB 的文件').waitFor();
   const payload=Buffer.from(JSON.stringify({'hdhome.org':[{domain:'hdhome.org',name:'c_secure_pass',value:'synthetic',path:'/'}]}));
   await panel.getByLabel('PTD 备份文件',{exact:true}).setInputFiles({name:'cookies.json',mimeType:'application/json',buffer:payload});
   await page.screenshot({path:`${output}/${width}-settings.png`,fullPage:true});
   await submit.click();
   await panel.getByText('备份未包含 cookies.json，请在 PTD 中勾选 Cookie').waitFor();
   assert.equal(Buffer.from(writes[0].content_base64,'base64').toString(),payload.toString());
   fail=false;await submit.click();
   await panel.getByText('导入完成',{exact:true}).waitFor();
   await panel.getByText('查看跳过原因',{exact:true}).click();
   await panel.getByText('测试站点：Cookie 不适用于现有访问域名').waitFor();
   await panel.getByText('导入完成',{exact:true}).scrollIntoViewIfNeeded();
   await page.screenshot({path:`${output}/${width}-result.png`,fullPage:true});
   await panel.getByText(/相同 Cookie 重复导入/).scrollIntoViewIfNeeded();
   await page.screenshot({path:`${output}/${width}-guide.png`,fullPage:true});
   assert.equal(await page.evaluate(()=>document.documentElement.scrollWidth>innerWidth),false);
   assert.deepEqual(errors,[]);
   console.log(`${width}: entry, file validation, upload error recovery, base64 payload, results and no overflow passed`);
   await page.close();
  }
 } finally {await browser.close();}
})();
