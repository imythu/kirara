#!/usr/bin/env node
// Maintenance only: regenerate reference vectors, or extract the published MIT model.
// Usage: node reference.cjs /path/to/node_modules/@ternlight/mini reference|extract
const fs=require('fs'),path=require('path'),crypto=require('crypto'),Module=require('module');
const directory=path.resolve(process.argv[2]);const mode=process.argv[3]||'reference';
const root=path.resolve(__dirname,'../../..');
const meta=JSON.parse(fs.readFileSync(path.join(directory,'package.json')));
if(meta.name!=='@ternlight/mini'||meta.version!=='0.1.1'||meta.license!=='MIT')throw Error('requires MIT @ternlight/mini@0.1.1');
const js=path.join(directory,'pkg-node/tern_engine.js');
if(mode==='extract'){
 const loaded=new Module(js); loaded.filename=js; loaded.paths=Module._nodeModulePaths(path.dirname(js));
 loaded._compile(fs.readFileSync(js,'utf8')+'\nmodule.exports.__memory = wasm.memory;\n',js);
 const bytes=Buffer.from(loaded.exports.__memory.buffer);const start=bytes.indexOf(Buffer.from([84,69,82,78,1,0,3,0]));
 if(start<0)throw Error('mini header missing');
 const model=bytes.subarray(start,start+4839512);
 if(!crypto.createHash('sha256').update(model.subarray(0,-32)).digest().equals(model.subarray(-32)))throw Error('embedded model SHA256 mismatch');
 const expected='07d8cfdba5773ad69a3fe6164b6c964e87b2368cc3ad6c2bdaf8566f2e5b6c98';
 if(crypto.createHash('sha256').update(model).digest('hex')!==expected)throw Error('unexpected pinned model');
 fs.writeFileSync(path.join(root,'assets/search/models/mini/model.bin'),model);
 console.log('Extracted verified model bytes from the published package; no WASM is shipped.');
}else{
 const engine=require(js);
 const queries=['lossless music','anime for beginners','无损音乐','适合新手的动漫站','An archive of high quality films.','Hello, WORLD!','Ｍ－Ｔｅａｍ','重庆音乐','children family education','rare obscure arthouse films','music audio lossless soundtrack concert. 无损音乐','anime animation cartoons manga beginner friendly easy economy. 适合新手的动漫站','xqzvjk qprst nonsense','天气预报明天下雨','Café naïve résumé','a '.repeat(180),'','   ','QingWa 青蛙 2026','Dolby Atmos 4K Remux'];
 const rows=queries.map(text=>({text,tokens:Array.from(engine.tokenize(text)),vector:Array.from(engine.embed(text))}));
 fs.writeFileSync(path.join(root,'crates/search-encoder/tests/fixtures/mini-reference.json'),JSON.stringify({package:'@ternlight/mini@0.1.1',rows})+'\n');
 console.log('Generated '+rows.length+' tokenizer/vector reference fixtures');
}
