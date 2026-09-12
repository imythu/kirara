// Synthetic files and mocked APIs only.
const { chromium } = require(process.env.PLAYWRIGHT_MODULE || 'playwright');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const output = process.env.PTD_IMPORT_SCREENSHOTS || '/tmp/kirara-ptd-import';
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
   await page.goto(`${process.env.SITES_TEST_URL || 'http://127.0.0.1:5173'}/#/sites`);
   await page.getByRole('button',{name:'更多站点操作',exact:true}).click();
   await page.getByRole('menuitem',{name:/导入 PTD 数据/}).click();
   await page.getByText(/PTD 即 PT-depiler/).waitFor();
   assert.equal(await page.getByRole('button',{name:'从备份导入',exact:true}).getAttribute('aria-pressed'),'true');
   const chooserEvent = page.waitForEvent('filechooser');
   await page.getByRole('button',{name:'选择文件',exact:true}).click();
   await chooserEvent;
   const panel=page.getByRole('region',{name:'PTD 配置导入'});
   const submit=panel.getByRole('button',{name:'开始导入',exact:true});
   assert(await submit.isDisabled());
   await panel.getByLabel('PTD 备份文件',{exact:true}).setInputFiles({name:'empty.json',mimeType:'application/json',buffer:Buffer.alloc(0)});
   await panel.getByText('请选择非空且不超过 8 MiB 的文件').waitFor();
   const payload=Buffer.from(JSON.stringify({'hdhome.org':[{domain:'hdhome.org',name:'c_secure_pass',value:'synthetic',path:'/'}]}));
   await panel.getByLabel('PTD 备份文件',{exact:true}).setInputFiles({name:'cookies.json',mimeType:'application/json',buffer:payload});
   await panel.getByRole('button',{name:'更换文件',exact:true}).waitFor();
   await panel.getByRole('button',{name:'移除文件',exact:true}).click();
   assert(await submit.isDisabled());
   await panel.getByLabel('PTD 备份文件',{exact:true}).setInputFiles({name:'cookies.json',mimeType:'application/json',buffer:payload});
   const dropzone=panel.getByRole('group',{name:'备份文件选择区'});
   async function dropFiles(names) {
    const transfer=await page.evaluateHandle(names=>{
     const data=new DataTransfer();
     for(const name of names) data.items.add(new File(['synthetic'],name,{type:'application/json'}));
     return data;
    },names);
    await dropzone.dispatchEvent('dragenter',{dataTransfer:transfer});
    await panel.getByText('松开以选择备份文件',{exact:true}).waitFor();
    await dropzone.dispatchEvent('drop',{dataTransfer:transfer});
    await transfer.dispose();
   }
   await dropFiles(['backup.txt']);
   await panel.getByText('请选择 PTD 导出的 ZIP 或 cookies.json 文件',{exact:true}).waitFor();
   assert(await submit.isDisabled());
   await dropFiles(['one.json','two.json']);
   await panel.getByText('一次请选择一个备份文件',{exact:true}).waitFor();
   await dropFiles(['cookies.json']);
   assert.equal(writes.length,0,'dropping a file does not submit it');
   assert.equal(await submit.isDisabled(),false);
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
