#!/usr/bin/env node
// Maintenance comparison: node evaluate.mjs /path/to/pt-sites /path/to/new-hybrid-results.json
import fs from 'node:fs';import path from 'node:path';import {createRequire} from 'node:module';import {pathToFileURL,fileURLToPath} from 'node:url';
const require=createRequire(import.meta.url),root=path.resolve(path.dirname(fileURLToPath(import.meta.url)),'../../..'),source=path.resolve(process.argv[2]);
const {decodeIndex,searchIndex}=await import(pathToFileURL(path.join(source,'src/search-core.js')));
const {embed}=require(path.join(source,'node_modules/@ternlight/mini'));
const bytes=fs.readFileSync(path.join(source,'public/data/site-index.bin'));const index=decodeIndex(bytes.buffer.slice(bytes.byteOffset,bytes.byteOffset+bytes.byteLength));
const fixturePath=process.argv[4]||path.join(root,'assets/search/evaluation.json');
const fixture=JSON.parse(fs.readFileSync(fixturePath));
const newResults=process.argv[3]?JSON.parse(fs.readFileSync(process.argv[3])):[];
const resultMap=new Map((Array.isArray(newResults)?newResults:newResults.queries).map(x=>[x.query,x.ids??x.results??x.ranking]));
function measures(ids,relevance){
 const dcg=ids.slice(0,10).reduce((a,id,i)=>a+(2**(relevance[id]??0)-1)/Math.log2(i+2),0);
 const ideal=Object.values(relevance).sort((a,b)=>b-a).slice(0,10).reduce((a,g,i)=>a+(2**g-1)/Math.log2(i+2),0);
 return {ndcg10:ideal?dcg/ideal:null,precision5:ids.slice(0,5).filter(id=>relevance[id]>0).length/5,returned:ids.length};
}
const rows=fixture.queries.map(({query,relevance,topic})=>{
 const old=searchIndex(index,query,embed,{limit:index.count}).map(x=>x.site.id);
 // Existing lexical/concept baseline drops zero-evidence rows (the upstream UI
 // normally ranks all public rows, which is inappropriate for no-result scoring).
 const lexical=searchIndex(index,query,null,{limit:index.count}).filter(x=>x.lexical>0).map(x=>x.site.id);
 const current=resultMap.get(query)||[];
 return {query,topic,reference_wasm:measures(old,relevance),lexical_concept:measures(lexical,relevance),native_hybrid:measures(current,relevance),top5:{reference:old.slice(0,5),lexical:lexical.slice(0,5),native:current.slice(0,5)}};
});
const summaries={};for(const channel of ['reference_wasm','lexical_concept','native_hybrid']){
 const positive=rows.filter(r=>r.topic!=='unrelated'),negative=rows.filter(r=>r.topic==='unrelated');
 summaries[channel]={ndcg10:positive.reduce((a,r)=>a+r[channel].ndcg10,0)/positive.length,precision5:positive.reduce((a,r)=>a+r[channel].precision5,0)/positive.length,no_result_false_recall:negative.filter(r=>r[channel].returned>0).length+'/'+negative.length};
}
console.log(JSON.stringify({fixture:path.basename(fixturePath),summaries,rows},null,2));
