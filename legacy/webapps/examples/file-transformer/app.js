(function(){
  const APP_ID='file-transformer';
  const LAST_KEY=APP_ID+':last-output';
  const $=(id)=>document.getElementById(id);
  async function call(method,params){
    if(window.AppRuntime&&typeof window.AppRuntime.call==='function') return window.AppRuntime.call(method,params);
    if(method==='dialog.openFile') return {files:[{name:'mock.txt',mime:'text/plain',size:11,text:'hello world'}]};
    if(method==='dialog.saveFile'||method==='notification.toast'||method==='storage.set'||method==='app.log') return {ok:true};
    if(method==='core.step') return {ok:true,actions:[{type:'TransformText',text:localTransform(params.event.payload.text,params.event.payload.mode)}]};
    throw new Error('Unknown mock method '+method);
  }
  function localTransform(text,mode){ if(mode==='uppercase')return text.toUpperCase(); if(mode==='lowercase')return text.toLowerCase(); if(mode==='reverse-lines')return text.split(/\r?\n/).reverse().join('\n'); if(mode==='word-count'){const words=text.trim()?text.trim().split(/\s+/).length:0;return 'Characters: '+text.length+'\nWords: '+words+'\nLines: '+text.split(/\r?\n/).length;} return text; }
  function status(obj){ $('status').textContent=typeof obj==='string'?obj:JSON.stringify(obj,null,2); }
  async function openFile(){ try{ const res=await call('dialog.openFile',{accept:['text/plain','application/json'],multiple:false,maxBytes:5242880}); const file=res.files&&res.files[0]; if(file){$('input').value=String(file.text||''); status({opened:file.name,size:file.size});} }catch(e){status('Open failed: '+e.message);} }
  async function transform(){ const text=$('input').value; const mode=$('mode').value; const res=await call('core.step',{app:APP_ID,event:{type:'TransformText',payload:{text,mode}}}); const action=res.actions&&res.actions.find(a=>a.text||a.type==='TransformText'); const output=(action&&action.text)||localTransform(text,mode); $('output').value=output; await call('storage.set',{key:LAST_KEY,value:{mode,output,at:Date.now()}}); await call('notification.toast',{message:'Transform complete',level:'success'}); status(res); }
  async function save(){ const text=$('output').value; if(!text){status('Nothing to save');return;} await call('dialog.saveFile',{suggestedName:'transformed.txt',mime:'text/plain',text}); await call('notification.toast',{message:'Saved output',level:'success'}); }
  $('open').addEventListener('click',openFile); $('transform').addEventListener('click',transform); $('save').addEventListener('click',save);
})();
