(function(){
  const APP_ID='core-replay-lab'; const KEY=APP_ID+':events'; let events=[]; const $=(id)=>document.getElementById(id);
  async function call(method,params){
    if(window.AppRuntime&&typeof window.AppRuntime.call==='function') return window.AppRuntime.call(method,params);
    window.__mockStorage=window.__mockStorage||new Map();
    if(method==='storage.get') return {value:window.__mockStorage.has(params.key)?window.__mockStorage.get(params.key):params.defaultValue};
    if(method==='storage.set'){window.__mockStorage.set(params.key,params.value);return {ok:true};}
    if(method==='dialog.saveFile'||method==='notification.toast'||method==='app.log') return {ok:true};
    if(method==='core.step') return {ok:true,stateVersion:events.length+1,actions:[{type:'Log',message:'Mock handled '+params.event.type}]};
    throw new Error('Unknown mock method '+method);
  }
  async function load(){ const r=await call('storage.get',{key:KEY,defaultValue:[]}); events=Array.isArray(r.value)?r.value:[]; renderLog(); }
  async function persist(){ await call('storage.set',{key:KEY,value:events}); }
  function parsePayload(){ try{return JSON.parse($('payload').value||'{}');}catch(e){throw new Error('Payload must be valid JSON: '+e.message);} }
  async function send(){ try{ const event={type:$('event-type').value,payload:parsePayload(),at:new Date().toISOString()}; const res=await call('core.step',{app:APP_ID,event}); $('output').textContent=JSON.stringify(res,null,2); events.push(event); await persist(); await call('notification.toast',{message:'Event sent',level:'success'}); renderLog(); }catch(e){$('output').textContent=e.message;} }
  async function replay(){ const outputs=[]; for(const event of events){ outputs.push(await call('core.step',{app:APP_ID,event})); } $('output').textContent=JSON.stringify({replayed:events.length,outputs},null,2); }
  async function clear(){ events=[]; await persist(); renderLog(); $('output').textContent='Cleared.'; }
  async function exportLog(){ const text=JSON.stringify({app:APP_ID,events},null,2); await call('dialog.saveFile',{suggestedName:'core-replay-fixture.json',mime:'application/json',text}); await call('notification.toast',{message:'Fixture exported',level:'success'}); }
  function renderLog(){ const log=$('log'); log.innerHTML=''; if(!events.length){ const li=document.createElement('li'); li.textContent='No events yet.'; log.append(li); return; } events.forEach((e,i)=>{ const li=document.createElement('li'); li.className='event'; li.textContent=(i+1)+'. '+e.type+' '+JSON.stringify(e.payload); log.append(li); }); }
  $('send').addEventListener('click',send); $('replay').addEventListener('click',replay); $('clear').addEventListener('click',clear); $('export').addEventListener('click',exportLog); load();
})();
