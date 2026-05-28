(function(){
  const APP_ID='task-workbench';
  const KEY=APP_ID+':tasks';
  let tasks=[];
  let filter='all';
  const $=(id)=>document.getElementById(id);
  async function call(method, params){
    if(window.AppRuntime&&typeof window.AppRuntime.call==='function') return window.AppRuntime.call(method, params);
    window.__mockStorage=window.__mockStorage||new Map();
    if(method==='storage.get') return {value:window.__mockStorage.has(params.key)?window.__mockStorage.get(params.key):params.defaultValue};
    if(method==='storage.set'){window.__mockStorage.set(params.key,params.value);return {ok:true};}
    if(method==='notification.toast'||method==='app.log') return {ok:true};
    if(method==='core.step') return {ok:true,stateVersion:Date.now(),actions:[{type:'Toast',message:'Mock core accepted '+params.event.type}]};
    throw new Error('Unknown mock method '+method);
  }
  async function load(){ const r=await call('storage.get',{key:KEY,defaultValue:[]}); tasks=Array.isArray(r.value)?r.value:[]; render(); }
  async function persist(){ await call('storage.set',{key:KEY,value:tasks}); }
  async function add(){
    const title=$('title').value.trim();
    if(!title){ $('title').focus(); return; }
    const priority=$('priority').value;
    const core=await call('core.step',{app:APP_ID,event:{type:'CreateTask',payload:{title,priority}}});
    $('core-output').textContent=JSON.stringify(core,null,2);
    tasks.unshift({id:'task_'+Date.now(),title,priority,done:false,createdAt:Date.now()});
    $('title').value=''; await persist(); await call('notification.toast',{message:'Task added',level:'success'}); render();
  }
  async function toggle(id){ tasks=tasks.map(t=>t.id===id?{...t,done:!t.done}:t); await call('core.step',{app:APP_ID,event:{type:'ToggleTask',payload:{id}}}).then(r=>{$('core-output').textContent=JSON.stringify(r,null,2)}); await persist(); render(); }
  async function remove(id){ tasks=tasks.filter(t=>t.id!==id); await persist(); render(); }
  function visible(t){ return filter==='all'||(filter==='open'&&!t.done)||(filter==='done'&&t.done)||(filter==='high'&&t.priority==='high'); }
  function render(){
    const list=$('tasks'); list.innerHTML=''; const filtered=tasks.filter(visible); $('empty').hidden=filtered.length!==0;
    for(const task of filtered){
      const li=document.createElement('li'); li.className='task';
      const check=document.createElement('input'); check.type='checkbox'; check.checked=task.done; check.addEventListener('change',()=>toggle(task.id));
      const title=document.createElement('div'); title.className='title'+(task.done?' done':''); title.textContent=task.title;
      const meta=document.createElement('div'); const pill=document.createElement('span'); pill.className='pill '+task.priority; pill.textContent=task.priority; const del=document.createElement('button'); del.textContent='Delete'; del.addEventListener('click',()=>remove(task.id)); meta.append(pill,document.createTextNode(' '),del);
      li.append(check,title,meta); list.append(li);
    }
  }
  $('add').addEventListener('click',add); $('title').addEventListener('keydown',e=>{if(e.key==='Enter')add();});
  for(const b of document.querySelectorAll('[data-filter]')) b.addEventListener('click',()=>{filter=b.dataset.filter; document.querySelectorAll('[data-filter]').forEach(x=>x.classList.toggle('active',x===b)); render();});
  load().catch(e=>{$('empty').textContent='Failed to load: '+e.message;});
})();
