// Runs a real server with an isolated temporary database and synthetic credentials.
// Build kirara first; WEBDAV_MODULE may point to a test-only webdav@5.10.0 installation.
const {createClient} = await import(process.env.WEBDAV_MODULE || 'webdav');
import {fileURLToPath} from 'node:url';
import assert from 'node:assert/strict';
import {spawn, execFileSync} from 'node:child_process';
import {mkdtemp, rm} from 'node:fs/promises';
import {createServer} from 'node:net';
import {once} from 'node:events';
async function port(){const s=createServer();s.listen(0,'127.0.0.1');await once(s,'listening');const p=s.address().port;await new Promise(r=>s.close(r));return p;}
const adminPort=await port(),dir=await mkdtemp('/tmp/kirara-dav-e2e-');
const child=spawn(process.env.KIRARA_TEST_BINARY || fileURLToPath(new URL('../../target/debug/kirara', import.meta.url)),['--host','127.0.0.1','--port',String(adminPort),'--data-dir',dir],{stdio:'ignore'});
const base=`http://127.0.0.1:${adminPort}`;
async function api(path,init){const r=await fetch(base+path,init);if(!r.ok)throw Error(`API ${r.status}`);return r.json();}
async function wait(check){for(let i=0;i<120;i++){try{if(await check())return;}catch{}await new Promise(r=>setTimeout(r,100));}throw Error('timeout');}
try {
 await wait(async()=>{await api('/api/sites/webdav-sync');return true;});
 const cfg=await api('/api/sites/webdav-sync',{method:'PUT',headers:{'Content-Type':'application/json'},body:JSON.stringify({enabled:true,username:'ptd',existing_policy:'update',auto_create:true,rotate_password:true})});
 await wait(async()=>(await api('/api/sites/webdav-sync')).running);
 assert.equal((await fetch(base + '/')).status, 200);
 assert.equal((await fetch(base + '/dav/ptd/', {method:'OPTIONS',headers:{Origin:'http://127.0.0.1:5173','Access-Control-Request-Method':'PUT'}})).status, 401);
 const dav=createClient(`http://127.0.0.1:${adminPort}/dav/ptd/`,{username:'ptd',password:cfg.new_password});
 assert.deepEqual(await dav.getDirectoryContents('/'),[]);
 const archive=execFileSync('python3',['-c',`import json,hashlib,zipfile,io,time,sys
raw=json.dumps({'hdhome.org':[{'domain':'hdhome.org','hostOnly':True,'name':'c_secure_pass','value':'synthetic-only','path':'/','secure':True}]}).encode()
b=io.BytesIO()
with zipfile.ZipFile(b,'w',zipfile.ZIP_DEFLATED) as z:
 z.writestr('cookies.json',raw)
 z.writestr('manifest.json',json.dumps({'encryption':False,'time':int(time.time()*1000),'files':{'cookies':{'name':'cookies.json','hash':hashlib.md5(raw).hexdigest()}}}))
sys.stdout.buffer.write(b.getvalue())`]);
 assert.equal(await dav.putFileContents('PTD_backup_fixture.zip',archive),true);
 const files=await dav.getDirectoryContents('/',{glob:'*.zip'});
 assert.equal(files.length,1);assert.equal(files[0].basename,'PTD_backup_fixture.zip');assert.equal(files[0].size,archive.length);
 assert.deepEqual(Buffer.from(await dav.getFileContents(files[0].filename)),archive);
 await wait(async()=>{const runs=await api('/api/sites/webdav-sync/runs');return runs[0]?.status==='done';});
 assert.equal((await api('/api/sites')).length,1);
 await dav.putFileContents('PTD_backup_fixture.zip',archive);
 assert.equal((await api('/api/sites/webdav-sync/runs')).length,1);
 await dav.deleteFile(files[0].filename);assert.deepEqual(await dav.getDirectoryContents('/'),[]);
 assert.equal((await api('/api/sites')).length,1);
 const raw = JSON.stringify({'hdhome.org':[{domain:'hdhome.org',hostOnly:true,name:'c_secure_pass',value:'synthetic-manual',path:'/',secure:true}]});
 const imported = await api('/api/sites/ptd-import',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({content_base64:Buffer.from(' '.repeat(2 * 1024 * 1024) + raw).toString('base64'),existing_policy:'update',auto_create:true})});
 assert.equal(imported.updated,1);
 assert.equal((await api('/api/sites')).length,1);
 console.log('manual import: main router, body larger than default JSON limit, shared matching and update passed');
 console.log('webdav@5.10.0: real process, shared Web port, Basic auth, PTD ZIP push, PROPFIND/glob, GET roundtrip, automatic site creation, duplicate retry, DELETE passed; synthetic credentials only.');
} finally {child.kill('SIGTERM');await Promise.race([once(child,'exit'),new Promise(r=>setTimeout(r,8000))]);if(child.exitCode===null)child.kill('SIGKILL');await rm(dir,{recursive:true,force:true});}
