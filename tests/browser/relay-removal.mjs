import fs from 'node:fs';
import path from 'node:path';
import http from 'node:http';
import assert from 'node:assert/strict';
import { chromium } from 'playwright-core';

const root=path.resolve('dist/mobile'), output=path.resolve(process.argv[2]);
let desktops, fail, removes;
const results=[], errors=[];
const server=http.createServer((request,response)=>{
  const url=new URL(request.url,'http://localhost');
  const json=(value,status=200)=>{response.writeHead(status,{'content-type':'application/json','cache-control':'no-store'});response.end(JSON.stringify(value));};
  if(url.pathname==='/relay/account')return json({id:'synthetic-account',name:'验收账号',hasPasskey:false,desktops});
  if(url.pathname==='/relay/health')return json({ok:true});
  if(request.method==='DELETE'&&url.pathname.startsWith('/relay/account/desktops/')){
    removes++;const id=url.pathname.split('/').at(-1);
    return setTimeout(()=>{if(fail)return json({error:'temporary'},503);desktops=desktops.filter(row=>row.id!==id);response.writeHead(204);response.end();},150);
  }
  const file=path.join(root,url.pathname==='/'?'index.html':url.pathname);
  if(!fs.existsSync(file)){response.writeHead(404);return response.end();}
  response.setHeader('content-type',({'.js':'text/javascript','.css':'text/css','.html':'text/html','.svg':'image/svg+xml','.png':'image/png'})[path.extname(file)]||'application/octet-stream');
  let bytes=fs.readFileSync(file);if(file.endsWith('index.html'))bytes=Buffer.from(bytes.toString().replace('<head>','<head><meta name="codex-mobile-relay">'));
  response.end(bytes);
});
await new Promise(resolve=>server.listen(0,'127.0.0.1',resolve));
const browser=await chromium.launch({headless:true,args:['--no-sandbox']});
try {
  for(const viewport of [{width:390,height:844},{width:844,height:390},{width:1280,height:900}]){
    desktops=[{id:'a'.repeat(64),name:'待配对电脑',online:false},{id:'b'.repeat(64),name:'另一台电脑',online:true}];fail=true;removes=0;
    const context=await browser.newContext({viewport}),page=await context.newPage();page.on('pageerror',e=>errors.push(String(e)));
    await page.goto(`http://127.0.0.1:${server.address().port}/`);
    const otherContext=await browser.newContext({viewport}),otherPage=await otherContext.newPage();
    otherPage.on('pageerror',e=>errors.push(String(e)));
    await otherPage.goto(`http://127.0.0.1:${server.address().port}/`);
    await otherPage.getByRole('button',{name:'移除 待配对电脑',exact:true}).waitFor();
    await page.getByRole('button',{name:'移除 待配对电脑',exact:true}).click();
    assert.equal(await page.getByText('仅移除此手机配对',{exact:true}).count(),0);
    await page.getByRole('button',{name:'从云账号移除',exact:true}).click();
    await page.getByText('无法从云账号移除这台电脑，请重试。',{exact:true}).waitFor();
    assert.equal(removes,1);assert.equal(await page.locator('.relay-device-entry').count(),2);
    fail=false;await page.getByRole('button',{name:'从云账号移除',exact:true}).click();
    await page.getByRole('button',{name:'移除 待配对电脑',exact:true}).waitFor({state:'detached'});
    assert.equal(removes,2);assert.equal(await page.locator('.relay-device-entry').count(),1);
    await otherPage.getByRole('button',{name:'移除 待配对电脑',exact:true}).waitFor({state:'detached',timeout:15000});
    assert.equal(await otherPage.locator('.relay-device-entry').count(),1);
    await otherContext.close();
    await page.reload();await page.getByRole('button',{name:'移除 另一台电脑',exact:true}).waitFor();
    assert.equal(await page.getByRole('button',{name:'移除 待配对电脑',exact:true}).count(),0);
    await page.getByRole('button',{name:'移除 另一台电脑',exact:true}).click();
    assert.equal(await page.locator('.relay-brand img').evaluate(image=>image.complete&&image.naturalWidth>0),true);
    await page.screenshot({path:path.join(output,`relay-removal-${viewport.width}.png`)});
    results.push({viewport,passed:true,removes});await context.close();
  }
} finally {
  fs.writeFileSync(path.join(output,'relay-removal-report.json'),JSON.stringify({results,errors},null,2));
  await browser.close();server.closeAllConnections();await new Promise(resolve=>server.close(resolve));
}
assert.deepEqual(errors,[]);console.log(`Relay removal: ${results.length} passed`);
