/* IotaPanel 主应用逻辑 —— hash 路由单页 */
(function(){
  'use strict';
  const $ = (s,r=document)=>r.querySelector(s);
  const $$ = (s,r=document)=>[...r.querySelectorAll(s)];

  window.IOTA_LANG = localStorage.getItem('iota_lang') || 'zh';
  const state = { theme:'sage', username:'', plugins:[], settings:null, store:[], security:null };

  /* ---------- 工具 ---------- */
  async function api(path, opts={}){
    const r = await fetch(path, Object.assign({
      headers:{'Content-Type':'application/json'},
      credentials:'same-origin'
    }, opts));
    if(r.status===401 && !path.startsWith('/api/login')){
      try{ const j=await r.json(); window.location.href='/login'; throw new Error(j.error||'unauthorized'); }catch(e){ if(e.message==='unauthorized') throw e; }
      window.location.href='/login'; throw new Error('unauthorized');
    }
    let j=null; try{ j=await r.json(); }catch(e){}
    if(!r.ok){ const m=(j&&(j.error||j.msg))||('HTTP '+r.status); throw new Error(m); }
    return j;
  }
  const esc = s => String(s==null?'':s).replace(/[&<>"']/g, c=>({'&':'&amp;','<':'&lt;','>':'&gt;','"':'&quot;',"'":'&#39;'}[c]));
  function toast(msg,type=''){
    const c=$('#toast-container'),el=document.createElement('div');
    el.className='toast '+type; el.textContent=msg; c.appendChild(el);
    setTimeout(()=>{el.style.opacity='0';el.style.transition='opacity .3s';setTimeout(()=>el.remove(),300);},2800);
  }
  function confirmBox(title,msg){
    return new Promise(res=>{
      const box=$('#confirm-box'); $('#confirm-title').textContent=title; $('#confirm-msg').textContent=msg;
      box.hidden=false;
      const close=(v)=>{box.hidden=true; $('#confirm-cancel').onclick=$('#confirm-ok').onclick=null; res(v);};
      $('#confirm-cancel').onclick=()=>close(false);
      $('#confirm-ok').onclick=()=>close(true);
    });
  }
  function fmtUptime(sec){
    const d=Math.floor(sec/86400),h=Math.floor(sec%86400/3600),m=Math.floor(sec%3600/60);
    if(d>0)return d+' '+__t('ov.days')+' '+h+' '+__t('ov.hours');
    if(h>0)return h+' '+__t('ov.hours')+' '+m+' '+__t('ov.minutes');
    return m+' '+__t('ov.minutes');
  }
  function applyLang(){
    document.documentElement.lang = window.IOTA_LANG==='en'?'en':'zh-CN';
    $$('[data-i18n]').forEach(el=>{ const k=el.dataset.i18n; const t=__t(k); if(el.dataset.i18nOrg==null)el.dataset.i18nOrg=el.textContent; if(t!==k)el.textContent=t; });
  }

  /* ---------- 登录守卫 ---------- */
  async function ensureAuth(){
    try { const m=await api('/api/me'); state.username=m.username; return true; }
    catch(e){ window.location.href='/login'; return false; }
  }

  /* ---------- 主题 ---------- */
  function applyTheme(t){
    state.theme=t; document.documentElement.setAttribute('data-theme',t);
    localStorage.setItem('iota_theme',t);
  }
  function cycleTheme(){
    const order=['sage','ocean','rose','lilac'];
    const i=order.indexOf(state.theme);
    applyTheme(order[(i+1)%order.length]);
    saveSettingPick('theme',state.theme);
    applyLang();
  }

  /* ---------- 顶部栏 ---------- */
  function bindTopbar(){
    $('#sidebar-toggle').onclick=()=>$('.body').classList.toggle('nav-open');
    $('#theme-toggle').onclick=cycleTheme;
    $('#logout-btn').onclick=async (e)=>{ e.preventDefault(); try{ await api('/api/logout',{method:'POST'});}catch(_){} window.location.href='/login'; };
    $('#user-chip').onclick=(e)=>{ e.stopPropagation(); const m=$('#user-menu'); m.hidden=!m.hidden; };
    document.addEventListener('click',()=>{ $('#user-menu').hidden=true; });
  }

  /* ---------- 侧栏渲染 ---------- */
  function renderSidebar(){
    const box=$('#plugin-menu-items'); box.innerHTML='';
    const nav=$('#plugin-nav');
    (state.plugins||[]).forEach(p=>{
      const a=document.createElement('a');
      a.className='nav-item'; a.dataset.href='#/plugin/'+p.name; a.dataset.name=p.name;
      a.innerHTML=`<span class="nav-ico">${esc(p.icon||'▸')}</span><span class="nav-txt" data-i18n-keep>${esc(p.title||p.name)}</span>`;
      box.appendChild(a);
    });
    nav.hidden = !(state.plugins&&state.plugins.length);
    $$('.nav-item[data-href]').forEach(a=>a.onclick=()=>{ location.hash=a.dataset.href; $('.body').classList.remove('nav-open'); });
    highlightNav();
  }
  function highlightNav(){
    const h=location.hash||'#/overview';
    $$('.nav-item').forEach(a=>{
      a.classList.toggle('active', (a.dataset.href===h) || (a.dataset.name && h.startsWith('#/plugin/'+a.dataset.name)));
    });
  }

  /* ---------- 主内容路由 ---------- */
  async function router(){
    const hash=location.hash||'#/overview';
    const content=$('#content');
    if(hash.startsWith('#/plugin/')){
      const name=hash.slice('#/plugin/'.length).split('/')[0];
      const p=(state.plugins||[]).find(x=>x.name===name);
      if(!p){ content.innerHTML=`<div class="empty">Plugin not found: ${esc(name)}</div>`; return; }
      $('#plugin-chip').hidden=false; $('#plugin-chip').textContent=p.title||p.name;
      content.innerHTML=`
        <div class="plugin-frame-wrap">
          <div class="plugin-frame-top flex">
            <a class="btn ghost sm" href="#/store"><span>←</span> ${__t('nav.store')}</a>
            <span class="spacer"></span>
            <span class="badge ${p.status&&p.status.running?'running':'stopped'}">${p.status&&p.status.running?'●':'○'} ${__t(p.status&&p.status.running?'plugin.running':'plugin.stopped')}</span>
          </div>
          <iframe class="plugin-frame" src="/p/${encodeURIComponent(name)}/" data-name="${esc(name)}"></iframe>
        </div>`;
      highlightNav(); return;
    }
    $('#plugin-chip').hidden=true;
    const routes={
      '#/overview':renderOverview,
      '#/store':renderStore,
      '#/settings':renderSettings,
      '#/account':renderAccount,
      '#/sessions':renderSessions,
      '#/log':renderLog,
    };
    const fn=routes[hash];
    if(fn){ content.innerHTML='<div class="route-loading"></div>'; await fn(content); }
    else content.innerHTML=`<div class="empty">404 · ${esc(hash)}</div>`;
    highlightNav(); applyLang();
  }

  /* ================= 概览 ================= */
  async function renderOverview(content){
    const [status,plugins]=await Promise.all([api('/api/status'),api('/api/plugins')]);
    state.plugins=plugins.plugins||[];
    const ps=state.plugins, running=ps.filter(p=>p.status&&p.status.running).length;
    content.innerHTML=`
      <div class="grid cols-4">
        ${statCard(status.version,'ov.version')}
        ${statCard(fmtUptime(status.uptime_seconds||0),'ov.uptime')}
        ${statCard(status.listen_addr||'','ov.listen')}
        ${statCard(status.home||'','ov.home')}
      </div>
      <div class="grid cols-2" style="margin-top:16px">
        <div class="card">
          <div class="card-title"><h3>${__t('ov.card.stats')}</h3><span class="spacer"></span>
            <span class="badge running"><span class="pulse-dot dot-running"></span> Panel</span></div>
          <div class="stat-row"><span class="k">${__t('ov.version')}</span><span class="v">v${esc(status.version)}</span></div>
          <div class="stat-row"><span class="k">${__t('ov.listen')}</span><span class="v mono">${esc(status.listen_addr)}</span></div>
          <div class="stat-row"><span class="k">${__t('ov.uptime')}</span><span class="v">${fmtUptime(status.uptime_seconds||0)}</span></div>
          <div class="stat-row"><span class="k">${__t('ov.idle')}</span><span class="v">${esc(status.idle_timeout_minutes)} min</span></div>
        </div>
        <div class="card">
          <div class="card-title"><h3>${__t('ov.card.plugins')}</h3><span class="spacer"></span>
            <span class="v">${running} / ${ps.length}</span></div>
          <div class="stat-row"><span class="k">${__t('ov.plugins_installed')}</span><span class="v">${ps.length}</span></div>
          <div class="stat-row"><span class="k">${__t('ov.plugins_running')}</span><span class="v">${running}</span></div>
        </div>
      </div>
      <div class="card" style="margin-top:16px">
        <div class="card-title">
          <h3>${__t('ov.card.plugins')}</h3><span class="spacer"></span>
          <a class="btn sm" href="#/store">${__t('nav.store')}</a>
        </div>
        ${ps.length===0 ? `<div class="empty">${__t('ov.no_plugins')}</div>` : `
        <table class="table">
          <thead><tr>
            <th>${__t('ov.plugin.title')}</th><th>${__t('ov.plugin.version')}</th>
            <th>${__t('ov.plugin.status')}</th><th>${__t('ov.plugin.desc')}</th><th style="width:200px">${__t('ov.plugin.actions')}</th>
          </tr></thead>
          <tbody>${ps.map(pluginRow).join('')}</tbody>
        </table>`}
      </div>`;
    bindPluginActions();
  }
  function statCard(val,label){
    return `<div class="card"><div class="stat-num" title="${esc(val)}">${esc(String(val).slice(0,26))}</div><div class="stat-label">${__t(label)}</div></div>`;
  }
  function pluginRow(p){
    const run=p.status&&p.status.running;
    const st=p.status&&p.status.state||(run?'running':'stopped');
    const stB = st==='running'?'running':(st==='error'?'err':'stopped');
    return `<tr data-name="${esc(p.name)}">
      <td><strong>${esc(p.title||p.name)}</strong><div class="mono" style="color:var(--tx3);font-size:11px">${esc(p.name)}</div></td>
      <td>v${esc(p.version)}</td>
      <td><span class="badge ${stB}">${st==='running'?'<span class="pulse-dot dot-running"></span>':''}${__t('plugin.'+(st==='error'?'error':st))}</span></td>
      <td style="color:var(--tx2);font-size:12.5px;max-width:280px">${esc(p.description||'')}</td>
      <td><div class="flex">
        <button class="btn sm act-start" data-name="${esc(p.name)}" ${run?'disabled':''}>${__t('ov.plugin.start')}</button>
        <button class="btn sm act-stop" data-name="${esc(p.name)}" ${!run?'disabled':''}>${__t('ov.plugin.stop')}</button>
        <button class="btn sm act-restart" data-name="${esc(p.name)}">${__t('ov.plugin.restart')}</button>
        <button class="btn sm act-uninstall" data-name="${esc(p.name)}">${__t('ov.plugin.uninstall')}</button>
      </div></td>
    </tr>`;
  }
  function bindPluginActions(){
    $$('.act-start').forEach(b=>b.onclick=async()=>{
      try{ const j=await api('/api/plugins/'+b.dataset.name+'/start',{method:'POST'}); toast(__t('plugin.running'),'success'); await router(); }
      catch(e){ toast(e.message,'error'); }
    });
    $$('.act-stop').forEach(b=>b.onclick=async()=>{
      try{ await api('/api/plugins/'+b.dataset.name+'/stop',{method:'POST'}); toast(__t('plugin.stopped'),'success'); await router(); }
      catch(e){ toast(e.message,'error'); }
    });
    $$('.act-restart').forEach(b=>b.onclick=async()=>{
      try{ await api('/api/plugins/'+b.dataset.name+'/restart',{method:'POST'}); toast('ok','success'); await router(); }
      catch(e){ toast(e.message,'error'); }
    });
    $$('.act-uninstall').forEach(b=>b.onclick=async()=>{
      if(!await confirmBox(__t('common.confirm'),`${__t('ov.plugin.uninstall')}: ${b.dataset.name}?`))return;
      try{ await api('/api/plugins/'+b.dataset.name,{method:'DELETE'}); toast('ok','success'); await router(); }
      catch(e){ toast(e.message,'error'); }
    });
  }

  /* ================= 插件商店 ================= */
  async function renderStore(content){
    let store=[]; try{ store=(await api('/api/store')).store||[]; }catch(e){}
    state.store=store;
    content.innerHTML=`
      <div class="card">
        <div class="card-title"><h3>${__t('store.title')}</h3><small class="sub" style="margin-left:8px">${__t('store.sub')}</small>
          <span class="spacer"></span>
          <button class="btn sm" id="url-toggle">${__t('store.install_url')}</button>
        </div>
        <div id="url-form" class="flex" style="margin:10px 0 16px" hidden>
          <input style="flex:2" id="url-input" placeholder="${__t('store.url_placeholder')}">
          <input style="flex:1.2" id="sha-input" placeholder="${__t('store.sha_placeholder')}">
          <button class="btn primary sm" id="url-install">${__t('store.install')}</button>
        </div>
        ${store.length===0 ? `<div class="empty">${__t('store.no_store')}</div>` : `
        <div class="grid cols-3">
        ${store.map(p=>`
          <div class="plugin-card" style="flex-direction:column">
            <div class="pc-title">${esc(p.title||p.name)}</div>
            <div class="pc-desc">${esc(p.description||'')}</div>
            <div class="pc-meta mono">v${esc(p.version)} · ${esc(p.language)} · ${esc(p.author)}</div>
            <div class="flex"><span class="spacer"></span>
              ${p.installed
                ? `<span class="badge running">✓ ${__t('store.installed')}</span>`
                : `<button class="btn primary sm act-install" data-name="${esc(p.name)}">${__t('store.install')}</button>`}
            </div>
          </div>`).join('')}
        </div>`}
      </div>`;
    $('#url-toggle').onclick=()=>{ const f=$('#url-form'); f.hidden=!f.hidden; };
    $('#url-install').onclick=async()=>{
      try{ await api('/api/store/install-url',{method:'POST',body:JSON.stringify({url:$('#url-input').value.trim(),sha256:$('#sha-input').value.trim()})}); toast('ok','success'); await renderStore(content); }
      catch(e){ toast(e.message,'error'); }
    };
    $$('.act-install').forEach(b=>b.onclick=async()=>{
      try{ await api('/api/store/'+b.dataset.name+'/install',{method:'POST'}); toast('ok','success'); await renderStore(content); }
      catch(e){ toast(e.message,'error'); }
    });
  }

  /* ================= 设置 ================= */
  async function renderSettings(content){
    const [s,sec]=await Promise.all([api('/api/settings'),api('/api/security')]);
    state.settings=s;
    content.innerHTML=`
      <div class="card"><div class="card-title"><h3>${__t('st.title')}</h3></div>
        <div class="tab-bar">
          <button class="tab active" data-tab="general">${__t('st.tabs.general')}</button>
          <button class="tab" data-tab="appearance">${__t('st.tabs.theme')}</button>
          <button class="tab" data-tab="security">${__t('st.tabs.security')}</button>
          <button class="tab" data-tab="about">${__t('st.tabs.about')}</button>
        </div>
        <div id="tab-general">
          <label class="field"><span class="field-label">${__t('st.idle')}</span>
            <input type="number" min="1" max="1440" id="set-idle" value="${esc(s.idle_timeout_minutes)}"></label>
          <label class="field"><span class="field-label">${__t('st.listen_port')}</span>
            <input type="number" min="1" max="65535" id="set-port" value="${esc(s.listen_addr.split(':').pop())}"></label>
        </div>
        <div id="tab-appearance" hidden>
          <label class="field"><span class="field-label">${__t('st.theme')}</span>
            <select id="set-theme">
              <option value="sage" ${s.theme==='sage'?'selected':''}>${__t('st.theme.sage')}</option>
              <option value="ocean" ${s.theme==='ocean'?'selected':''}>${__t('st.theme.ocean')}</option>
              <option value="rose" ${s.theme==='rose'?'selected':''}>${__t('st.theme.rose')}</option>
              <option value="lilac" ${s.theme==='lilac'?'selected':''}>${__t('st.theme.lilac')}</option>
            </select></label>
          <label class="field"><span class="field-label">${__t('st.lang')}</span>
            <select id="set-lang">
              <option value="zh" ${(s.lang||'zh').startsWith('zh')?'selected':''}>${__t('st.lang.zh')}</option>
              <option value="en" ${(s.lang||'').startsWith('en')?'selected':''}>${__t('st.lang.en')}</option>
            </select></label>
        </div>
        <div id="tab-security" hidden>
          <label class="field"><span class="field-label">${__t('st.fail_limit')}</span>
            <input type="number" min="1" max="100" id="set-fail" value="${esc(sec.fail_limit)}"></label>
          <label class="field"><span class="field-label">${__t('st.lock_minutes')}</span>
            <input type="number" min="1" max="1440" id="set-lock" value="${esc(sec.lock_minutes)}"></label>
        </div>
        <div id="tab-about" hidden>
          <div class="stat-row"><span class="k">${__t('st.version')}</span><span class="v">v${esc(s.version)}</span></div>
          <div class="stat-row"><span class="k">${__t('st.port_pool')}</span><span class="v mono">${esc(s.port_pool)}</span></div>
          <div class="stat-row"><span class="k">Home</span><span class="v mono">${esc(s.home)}</span></div>
        </div>
        <div class="flex" style="margin-top:18px"><span class="spacer"></span>
          <button class="btn primary" id="set-save">${__t('st.save')}</button></div>
      </div>`;
    $$('.tab').forEach(t=>t.onclick=()=>{
      $$('.tab').forEach(x=>x.classList.toggle('active',x===t));
      ['general','appearance','security','about'].forEach(n=>$('#tab-'+n).hidden=(n!==t.dataset.tab));
    });
    $('#set-theme').onchange=e=>applyTheme(e.target.value);
    $('#set-lang').onchange=e=>{ window.IOTA_LANG=e.target.value; localStorage.setItem('iota_lang',e.target.value); applyLang(); };
    $('#set-save').onclick=async()=>{
      try{
        const body={
          idle_timeout_minutes:+$('#set-idle').value||0,
          theme:state.theme,
          lang:window.IOTA_LANG,
          listen_port:+$('#set-port').value||0
        };
        const j=await api('/api/settings',{method:'PUT',body:JSON.stringify(body)});
        toast(j.need_restart?__t('st.need_restart'):__t('st.saved'),'success');
        // 安全设置单独保存
        const fb=+$('#set-fail').value||0, lb=+$('#set-lock').value||0;
        if(fb>0 && lb>0){ await api('/api/security',{method:'PUT',body:JSON.stringify({fail_limit:fb,lock_minutes:lb})}); }
      }catch(e){ toast(e.message,'error'); }
    };
  }
  function saveSettingPick(k,v){
    // 主题/语言即时保存（供 theme 切换）
    if(k==='theme'||k==='lang'){
      fetch('/api/settings',{method:'PUT',headers:{'Content-Type':'application/json'},body:JSON.stringify({[k]:v})}).catch(()=>{});
    }
  }

  /* ================= 账号 ================= */
  async function renderAccount(content){
    const ac=await api('/api/account');
    content.innerHTML=`
      <div class="grid cols-2">
        <div class="card">
          <div class="card-title"><h3>${__t('ac.username')}</h3></div>
          <div class="stat-row"><span class="k">${__t('ac.username')}</span><span class="v">${esc(ac.username)}</span></div>
          <div class="stat-row"><span class="k">${__t('ac.created')}</span><span class="v">${esc(ac.created_at||'')}</span></div>
          <div class="stat-row"><span class="k">${__t('ac.last_login')}</span><span class="v">${esc(ac.last_login_at||'-')}</span></div>
          <div class="flex" style="margin-top:14px">
            <input id="new-username" placeholder="${__t('ac.new_username')}" style="flex:1;padding:9px 12px;border:1px solid var(--line2);border-radius:9px;background:var(--panel2)">
            <button class="btn sm" id="ac-username-save">${__t('ac.change_username')}</button>
          </div>
        </div>
        <div class="card">
          <div class="card-title"><h3>${__t('ac.password')}</h3></div>
          <label class="field"><span class="field-label">${__t('ac.old_pass')}</span><input type="password" id="old-pass"></label>
          <label class="field"><span class="field-label">${__t('ac.new_pass')}</span><input type="password" id="new-pass" minlength="6"></label>
          <div class="flex"><span class="spacer"></span>
            <a class="btn sm" href="#/sessions">${__t('ac.sessions')}</a>
            <button class="btn primary sm" id="ac-pass-save">${__t('ac.save')}</button></div>
        </div>
      </div>`;
    $('#ac-username-save').onclick=async()=>{
      const v=$('#new-username').value.trim(); if(!v){toast('username required','error');return;}
      try{ await api('/api/account/username',{method:'POST',body:JSON.stringify({new_username:v})}); toast('ok','success'); location.reload(); }
      catch(e){ toast(e.message,'error'); }
    };
    $('#ac-pass-save').onclick=async()=>{
      try{ await api('/api/account/password',{method:'POST',body:JSON.stringify({old_password:$('#old-pass').value,new_password:$('#new-pass').value})});
        toast('ok','success'); $('#old-pass').value='';$('#new-pass').value=''; }
      catch(e){ toast(e.message,'error'); }
    };
  }

  /* ================= 会话 ================= */
  async function renderSessions(content){
    const j=await api('/api/account/sessions'); const list=j.sessions||[];
    content.innerHTML=`
      <div class="card"><div class="card-title"><h3>${__t('ac.sessions')}</h3><span class="spacer"></span>
        <button class="btn danger sm" id="revoke-all">${__t('ac.revoke_all')}</button></div>
      ${list.length===0?`<div class="empty">${__t('ac.no_sessions')}</div>`:`
      <table class="table"><thead><tr><th>IP</th><th>User-Agent</th><th>${__t('ac.created')}</th><th>过期时间</th><th></th></tr></thead><tbody>
      ${list.map(s=>`<tr>
        <td class="mono">${esc(s.ip||'-')}</td>
        <td class="mono" style="color:var(--tx2);font-size:11.5px;max-width:200px;overflow:hidden;text-overflow:ellipsis;white-space:nowrap">${esc(s.user_agent||'')}</td>
        <td>${esc(s.created_at||'')}</td>
        <td class="mono" style="font-size:11.5px">${esc(s.expires_at||'')}</td>
        <td>${s.current?`<span class="badge running">${__t('ac.current')}</span>`:`<button class="btn sm danger revoke-s" data-j="${esc(s.jti)}">${__t('ac.revoke')}</button>`}</td>
      </tr>`).join('')}
      </tbody></table>`}
      </div>`;
    $$('.revoke-s').forEach(b=>b.onclick=async()=>{
      try{ await api('/api/account/sessions/revoke',{method:'POST',body:JSON.stringify({jti:b.dataset.j})}); toast('ok','success'); await renderSessions(content); }
      catch(e){ toast(e.message,'error'); }
    });
    $('#revoke-all').onclick=async()=>{
      if(!await confirmBox(__t('common.confirm'),__t('ac.revoke_all')+'?'))return;
      try{ await api('/api/account/sessions/revoke-all',{method:'POST'}); toast('ok','success'); await renderSessions(content); }
      catch(e){ toast(e.message,'error'); }
    };
  }

  /* ================= 日志 ================= */
  async function renderLog(content){
    await refreshLog(content);
  }
  async function refreshLog(content){
    let j={}; try{ j=await api('/api/log'); }catch(e){}
    content.innerHTML=`
      <div class="card"><div class="card-title"><h3>${__t('log.title')}</h3><span class="spacer"></span>
        <button class="btn sm" id="log-refresh">${__t('log.refresh')}</button></div>
      ${j.log?`<pre class="log-view">${esc(j.log)}</pre>`:`<div class="empty">${__t('log.empty')}</div>`}
      </div>`;
    $('#log-refresh').onclick=()=>refreshLog(content);
  }

  /* ---------- 启动 ---------- */
  async function init(){
    document.documentElement.setAttribute('data-theme', localStorage.getItem('iota_theme')||'sage');
    applyLang();
    bindTopbar();
    if(!await ensureAuth())return;
    // 语言/主题从设置载入
    try{ const s=await api('/api/settings'); if(s.theme)applyTheme(s.theme); if(s.lang){window.IOTA_LANG=s.lang.startsWith('en')?'en':'zh';localStorage.setItem('iota_lang',window.IOTA_LANG);} }catch(e){}
    applyLang();
    // 加载插件用于侧栏
    try{ state.plugins=(await api('/api/plugins')).plugins||[]; }catch(e){}
    renderSidebar();
    window.addEventListener('hashchange',router);
    await router();
  }
  document.addEventListener('DOMContentLoaded',init);
})();