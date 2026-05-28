(function(){
  const APP_ID='api-dashboard'; const KEY=APP_ID+':history'; let history=[]; const $=(id)=>document.getElementById(id);
  async function call(method,params){
    if(window.AppRuntime&&typeof window.AppRuntime.call==='function') return window.AppRuntime.call(method,params);
    window.__mockStorage=window.__mockStorage||new Map();
    if(method==='storage.get') return {value:window.__mockStorage.has(params.key)?window.__mockStorage.get(params.key):params.defaultValue};
    if(method==='storage.set'){window.__mockStorage.set(params.key,params.value);return {ok:true};}
    if(method==='notification.toast'||method==='app.log') return {ok:true};
    if(method==='network.request') return {status:200,headers:{'content-type':'application/json'},bodyText:JSON.stringify({mock:true,url:params.url,time:new Date().toISOString()},null,2)};
    throw new Error('Unknown mock method '+method);
  }
  async function load(){const r=await call('storage.get',{key:KEY,defaultValue:[]}); history=Array.isArray(r.value)?r.value:[]; renderHistory();}
  async function send(){
    const url=$('url').value.trim(); const method=$('method').value; const start=performance.now(); $('response').textContent='Loading…';
    try{ const res=await call('network.request',{url,method,headers:{},body:null,timeoutMs:10000}); const ms=Math.round(performance.now()-start); const body=String(res.bodyText||''); $('status').textContent=String(res.status); $('bytes').textContent=String(body.length); $('duration').textContent=ms+' ms'; $('response').textContent=body.slice(0,5000); history.unshift({url,method,status:res.status,bytes:body.length,ms,at:new Date().toISOString()}); history=history.slice(0,10); await call('storage.set',{key:KEY,value:history}); await call('notification.toast',{message:'Request complete',level:'success'}); renderHistory(); }
    catch(e){ $('response').textContent='Request failed: '+e.message; await call('notification.toast',{message:'Request failed',level:'error'}); }
  }
  function renderHistory(){ const box=$('history'); box.innerHTML=''; if(!history.length){box.textContent='No requests yet.'; return;} for(const h of history){ const row=document.createElement('div'); row.className='row'; const s=document.createElement('b'); s.textContent=String(h.status); const u=document.createElement('span'); u.textContent=h.url; const m=document.createElement('span'); m.textContent=h.ms+' ms'; row.append(s,u,m); box.append(row);} }
  $('send').addEventListener('click',send); load();
})();
