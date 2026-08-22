// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 plainfate <https://github.com/plainfate>
/* ============================================================
   IotaPanel 主界面逻辑（纯原生 JavaScript）
   ============================================================ */

/* ---------- 通用工具 ---------- */
const $ = (sel) => document.querySelector(sel);

/** 封装 fetch：统一 JSON、错误处理与未登录跳转 */
async function api(path, opts = {}) {
  const res = await fetch(path, {
    headers: { 'Content-Type': 'application/json' },
    ...opts,
  });
  let data = null;
  try { data = await res.json(); } catch { /* 非 JSON 响应忽略 */ }
  if (res.status === 401) { location.href = '/login'; throw new Error('未登录'); }
  if (!res.ok) throw new Error((data && data.error) || ('HTTP ' + res.status));
  return data;
}

/** 顶部轻提示 */
function toast(msg, type = 'ok') {
  const el = $('#toast');
  el.textContent = msg;
  el.className = type === 'err' ? 'err' : '';
  clearTimeout(toast._t);
  toast._t = setTimeout(() => el.classList.add('hidden'), 2600);
}

/** HTML 转义，防止插件元信息注入 */
function esc(s) {
  return String(s == null ? '' : s)
    .replace(/&/g, '&amp;').replace(/</g, '&lt;')
    .replace(/>/g, '&gt;').replace(/"/g, '&quot;');
}

/* ---------- 全局状态 ---------- */
let plugins = [];       // 已安装插件列表（含菜单与运行状态）
let curPlugin = null;   // 抽屉当前展示的插件名

/* ============================================================
   主题（莫奈配色预设）
   ============================================================ */
const THEMES = [
  { id: 'sage',  label: '鼠尾草绿' },
  { id: 'ocean', label: '睡莲蓝' },
  { id: 'rose',  label: '玫瑰园' },
  { id: 'lilac', label: '丁香紫' },
];

/** 应用主题：写入 body[data-theme] 并记住到 localStorage */
function applyTheme(id) {
  document.body.dataset.theme = THEMES.some(t => t.id === id) ? id : 'sage';
  localStorage.setItem('mp-theme', document.body.dataset.theme);
}

/* ============================================================
   初始化：拉取状态与插件列表，渲染侧边栏
   ============================================================ */
async function init() {
  // 先应用本地记住的主题，避免闪烁
  applyTheme(localStorage.getItem('mp-theme') || 'sage');
  try {
    const [status, me, pl] = await Promise.all([
      api('/api/status'), api('/api/me'), api('/api/plugins'),
    ]);
    window.__status = status;
    $('#user-name').textContent = me.username;
    plugins = pl.plugins || [];
    renderNav();
    bindGlobalEvents();
    route(); // 处理当前 hash
    setInterval(refreshStatusDots, 15000); // 定期刷新运行状态小圆点
  } catch (e) {
    toast(e.message, 'err');
  }
}

/* ============================================================
   侧边栏渲染：由每个插件的 manifest.yaml 菜单动态注入
   ============================================================ */
function renderNav() {
  const nav = $('#plugin-nav');
  nav.innerHTML = '';
  plugins.forEach((p) => {
    if (!p.menus || !p.menus.length) return; // 无菜单的插件不进侧边栏
    const group = document.createElement('div');
    group.className = 'nav-group';
    // 组头：插件名 + 运行状态圆点 + 「管理」入口（打开详情抽屉）
    group.innerHTML = `
      <div class="nav-group-head" data-plugin="${esc(p.name)}">
        <span class="dot ${p.status.running ? 'on' : ''}"></span>
        <span>${esc(p.title)}</span>
        <span class="manage" title="插件详情">⚙</span>
      </div>`;
    // 子菜单：点击后 iframe 嵌入对应插件页面
    p.menus.forEach((m) => {
      const path = (m.path || '/').startsWith('/') ? m.path : '/' + m.path;
      const item = document.createElement('a');
      item.className = 'nav-item';
      item.href = '#/p/' + encodeURIComponent(p.name) + path;
      item.textContent = (m.icon ? m.icon + ' ' : '') + m.title;
      group.appendChild(item);
    });
    nav.appendChild(group);
  });
}

/* 侧边栏点击：插件组头 → 打开抽屉 */
function bindGlobalEvents() {
  document.addEventListener('click', (e) => {
    const head = e.target.closest('.nav-group-head');
    if (head) { openDrawer(head.dataset.plugin); return; }
    const item = e.target.closest('#sys-nav .nav-item');
    if (item) setActive(item);
  });
  $('#btn-logout').addEventListener('click', async () => {
    try { await api('/api/logout', { method: 'POST' }); } catch { /* 忽略 */ }
    location.href = '/login';
  });
  // 修改用户名
  $('#btn-change-username').addEventListener('click', async () => {
    const cur = $('#acc-username').textContent;
    const name = prompt('输入新的用户名（3-32 个字符，修改后无需重新登录）：', cur);
    if (!name || name === cur) return;
    try {
      await api('/api/account/username', {
        method: 'POST',
        body: JSON.stringify({ new_username: name }),
      });
      toast('用户名已修改为 ' + name);
      renderAccount();
    } catch (e) { toast(e.message, 'err'); }
  });
  // 重启面板（异步触发，页面会短暂断开）
  $('#btn-restart').addEventListener('click', async () => {
    if (!confirm('确定重启面板？保活插件不受影响，页面将短暂断开后恢复。')) return;
    try {
      const d = await api('/api/system/restart', { method: 'POST' });
      toast(d.msg || '重启已触发');
    } catch (e) { toast(e.message, 'err'); }
  });
  // 核心日志
  $('#btn-core-log').addEventListener('click', renderCoreLog);
  // 主题切换（莫奈预设）
  $('#theme-picker').addEventListener('click', (e) => {
    const item = e.target.closest('.theme-item');
    if (!item) return;
    applyTheme(item.dataset.theme);
    api('/api/settings', { method: 'PUT', body: JSON.stringify({ theme: item.dataset.theme }) }).catch(() => {});
    renderSettings(); // 刷新选中高亮
  });
  // 语言切换（18 种）
  $('#lang-picker').addEventListener('click', (e) => {
    const item = e.target.closest('.theme-item');
    if (!item) return;
    setLang(item.dataset.lang);
    api('/api/settings', { method: 'PUT', body: JSON.stringify({ lang: item.dataset.lang }) }).catch(() => {});
    renderSettings();
  });
  // 语言变化后重绘当前页
  document.addEventListener('langchange', () => route());
  // 保存监听端口（重启生效）
  $('#btn-save-port').addEventListener('click', async () => {
    const p = parseInt($('#listen-port').value, 10);
    if (isNaN(p) || p < 1 || p > 65535) return toast('端口需在 1-65535 之间', 'err');
    try {
      const d = await api('/api/settings', { method: 'PUT', body: JSON.stringify({ listen_port: p }) });
      toast(d.need_restart ? '端口已保存，重启面板后生效（systemctl restart iotapanel）' : '已保存');
    } catch (e) { toast(e.message, 'err'); }
  });
  // 远程 URL 安装插件
  $('#btn-url-install').addEventListener('click', async () => {
    const url = $('#url-input').value.trim();
    const sha = $('#url-sha256').value.trim();
    if (!url) return toast('请输入插件包下载地址', 'err');
    const btn = $('#btn-url-install');
    btn.disabled = true; btn.textContent = '下载中…';
    try {
      const d = await api('/api/store/install-url', { method: 'POST', body: JSON.stringify({ url, sha256: sha }) });
      toast('「' + d.plugin + '」安装成功 v' + d.version);
      $('#url-input').value = ''; $('#url-sha256').value = '';
      await refreshPlugins();
      renderPlugins();
    } catch (e) { toast(e.message, 'err'); }
    btn.disabled = false; btn.textContent = '下载并安装';
  });
  // 修改密码
  $('#btn-change-pw').addEventListener('click', async () => {
    const oldPw = $('#pw-old').value, newPw = $('#pw-new').value, newPw2 = $('#pw-new2').value;
    if (!oldPw) return toast('请输入原密码', 'err');
    if (newPw.length < 6) return toast('新密码至少 6 位', 'err');
    if (newPw !== newPw2) return toast('两次输入的新密码不一致', 'err');
    try {
      const d = await api('/api/account/password', {
        method: 'POST',
        body: JSON.stringify({ old_password: oldPw, new_password: newPw }),
      });
      $('#pw-old').value = $('#pw-new').value = $('#pw-new2').value = '';
      toast('密码已修改' + (d.revoked_sessions ? '，其他 ' + d.revoked_sessions + ' 个会话已下线' : ''));
    } catch (e) { toast(e.message, 'err'); }
  });
  // 保存登录安全策略
  $('#btn-save-sec').addEventListener('click', async () => {
    try {
      await api('/api/security', {
        method: 'PUT',
        body: JSON.stringify({
          fail_limit: parseInt($('#sec-fail').value, 10),
          lock_minutes: parseInt($('#sec-lock').value, 10),
        }),
      });
      toast('登录安全策略已保存');
    } catch (e) { toast(e.message, 'err'); }
  });
  // 下线其他全部会话
  $('#btn-revoke-all').addEventListener('click', async () => {
    if (!confirm('确定下线除当前会话外的所有登录会话？')) return;
    try {
      const d = await api('/api/account/sessions/revoke-all', { method: 'POST' });
      toast('已下线 ' + (d.revoked || 0) + ' 个会话');
      renderAccount();
    } catch (e) { toast(e.message, 'err'); }
  });
}

/** 高亮当前侧边栏项 */
function setActive(el) {
  document.querySelectorAll('.nav-item').forEach(n => n.classList.remove('active'));
  if (el) el.classList.add('active');
}

/* 只刷新运行状态小圆点（不重建 DOM） */
async function refreshStatusDots() {
  try {
    const pl = await api('/api/plugins');
    const map = {};
    (pl.plugins || []).forEach(p => map[p.name] = p.status.running);
    document.querySelectorAll('.nav-group-head').forEach(h => {
      const on = map[h.dataset.plugin];
      const dot = h.querySelector('.dot');
      if (dot) dot.classList.toggle('on', !!on);
    });
  } catch { /* 忽略 */ }
}

/* ============================================================
   路由：#/overview  #/plugins  #/settings  #/about  #/p/<插件>/<路径>
   ============================================================ */
function route() {
  const hash = location.hash || '';
  const pages = ['overview', 'plugins', 'settings', 'about'];
  const pageEls = {
    overview: $('#page-overview'), plugins: $('#page-plugins'),
    settings: $('#page-settings'), about: $('#page-about'),
  };
  const pluginView = $('#plugin-view');

  // 插件页面路由：#/p/<name>/<plugin-path>
  const m = hash.match(/^#\/p\/([^/]+)(\/.*)?$/);
  if (m) {
    pages.forEach(p => pageEls[p].classList.add('hidden'));
    pluginView.classList.remove('hidden');
    const name = decodeURIComponent(m[1]);
    const pluginPath = m[2] || '/';
    // 高亮侧边栏中对应的菜单项
    document.querySelectorAll('#plugin-nav .nav-item').forEach(n => n.classList.remove('active'));
    const anchor = document.querySelector(`#plugin-nav a[href="${CSS.escape(hash)}"]`);
    if (anchor) anchor.classList.add('active');
    openPlugin(name, pluginPath);
    return;
  }
  pluginView.classList.add('hidden');

  // 系统页面路由（默认概览）
  let page = pages.find(p => hash === '#/' + p) || 'overview';
  pages.forEach(p => pageEls[p].classList.toggle('hidden', p !== page));
  setActive(document.querySelector(`a[href="#/${page}"]`));
  if (page === 'overview') renderOverview();
  if (page === 'plugins') renderPlugins();
  if (page === 'settings') renderSettings();
  if (page === 'about') renderAbout();
}

window.addEventListener('hashchange', route);

/* ============================================================
   插件页面嵌入（iframe + 冷启动遮罩）
   ============================================================ */
let frameTimer = null;

function openPlugin(name, pluginPath) {
  const frame = $('#plugin-frame');
  const overlay = $('#loading-overlay');
  const txt = $('#loading-text');
  // 显示「冷启动」遮罩，等待反向代理拉起插件进程
  txt.textContent = '正在冷启动插件「' + name + '」…（首次约 1-2 秒）';
  overlay.classList.remove('hidden');
  clearTimeout(frameTimer);
  // 若超过 12 秒仍未就绪，提示错误（插件可能启动失败）
  frameTimer = setTimeout(() => {
    txt.textContent = '插件启动超时，请查看插件日志';
  }, 12000);
  frame.onload = () => overlay.classList.add('hidden'); // iframe 加载完成即隐藏遮罩
  frame.src = '/p/' + encodeURIComponent(name) + (pluginPath.startsWith('/') ? pluginPath : '/' + pluginPath);
}

/* ============================================================
   插件详情抽屉
   ============================================================ */
function openDrawer(name) {
  const p = plugins.find(x => x.name === name);
  if (!p) return;
  curPlugin = name;
  $('#drawer-mask').classList.remove('hidden');
  $('#drawer').classList.remove('hidden');
  $('#dr-title').textContent = p.title + '（' + p.name + '）';
  $('#dr-meta').textContent = 'v' + p.version + ' · ' + (p.author || '匿名') + ' · ' + (p.description || '');
  refreshDrawer(p);
  refreshLog();
}

function refreshDrawer(p) {
  const st = p.status || {};
  $('#dr-status').textContent = st.running ? '● 运行中' : '○ 已停止';
  $('#dr-status').style.color = st.running ? 'var(--ok)' : 'var(--muted)';
  $('#dr-port').textContent = st.running ? (st.port + ' / ' + st.pid) : '-';
  $('#dr-keepalive').checked = !!p.keepalive;
  $('#dr-start').disabled = st.running;
  $('#dr-stop').disabled = !st.running;
  $('#dr-restart').disabled = !st.running;
}

function closeDrawer() {
  curPlugin = null;
  $('#drawer-mask').classList.add('hidden');
  $('#drawer').classList.add('hidden');
}
$('#drawer-mask').addEventListener('click', closeDrawer);
$('#drawer-close').addEventListener('click', closeDrawer);

/* 保活开关：开启后进程常驻，面板核心重启也不受影响 */
$('#dr-keepalive').addEventListener('change', async (e) => {
  if (!curPlugin) return;
  try {
    await api('/api/plugins/' + encodeURIComponent(curPlugin) + '/keepalive', {
      method: 'POST',
      body: JSON.stringify({ enabled: e.target.checked }),
    });
    toast(e.target.checked ? '已开启后台保活' : '已关闭后台保活');
    await refreshPlugins();
  } catch (err) { toast(err.message, 'err'); }
});

/* 启停 / 重启 */
async function pluginAction(action) {
  if (!curPlugin) return;
  try {
    await api('/api/plugins/' + encodeURIComponent(curPlugin) + '/' + action, { method: 'POST' });
    await refreshPlugins();
  } catch (err) { toast(err.message, 'err'); }
}
$('#dr-start').addEventListener('click', () => pluginAction('start'));
$('#dr-stop').addEventListener('click', () => pluginAction('stop'));
$('#dr-restart').addEventListener('click', () => pluginAction('restart'));

/* 日志查看 */
async function refreshLog() {
  if (!curPlugin) return;
  try {
    const d = await api('/api/plugins/' + encodeURIComponent(curPlugin) + '/log');
    $('#dr-log').textContent = d.log || '（暂无日志）';
    $('#dr-log').scrollTop = $('#dr-log').scrollHeight;
  } catch { /* 忽略 */ }
}
$('#dr-log-refresh').addEventListener('click', refreshLog);

/* 卸载：停止进程 → 删除插件目录 → 删除数据库记录 → 刷新界面 */
$('#dr-uninstall').addEventListener('click', async () => {
  if (!curPlugin) return;
  if (!confirm('确定卸载插件「' + curPlugin + '」？将删除其目录与配置。')) return;
  try {
    await api('/api/plugins/' + encodeURIComponent(curPlugin), { method: 'DELETE' });
    toast('插件已卸载');
    closeDrawer();
    await refreshPlugins();
    route(); // 若正在使用该插件页面则切回默认视图
  } catch (err) { toast(err.message, 'err'); }
});

/* 重新拉取插件列表并重绘（操作后调用） */
async function refreshPlugins() {
  const pl = await api('/api/plugins');
  plugins = pl.plugins || [];
  renderNav();
  const p = plugins.find(x => x.name === curPlugin);
  if (p) refreshDrawer(p);
}

/* ============================================================
   概览页
   ============================================================ */
async function renderOverview() {
  try {
    const [st, pl] = await Promise.all([api('/api/status'), api('/api/plugins')]);
    $('#ov-version').textContent = 'v' + st.version;
    const up = st.uptime_seconds || 0;
    $('#ov-uptime').textContent = Math.floor(up / 3600) + ' 小时 ' + Math.floor(up % 3600 / 60) + ' 分';
    $('#ov-plugins').textContent = st.plugins_installed + ' 个（' + st.plugins_running + ' 运行中）';
    $('#ov-home').textContent = st.home;
    $('#ov-listen').textContent = st.listen_addr;
    $('#ov-idle').textContent = st.idle_timeout_minutes + ' 分钟';
    // 插件快捷入口
    const box = $('#ov-plugins-list');
    box.innerHTML = '';
    const list = pl.plugins || [];
    if (!list.length) { box.innerHTML = t('ovNone'); return; }
    list.forEach(p => {
      const row = document.createElement('div');
      row.className = 'kv';
      const firstMenu = p.menus && p.menus.length ? esc((p.menus[0].icon || '🧩') + ' ' + p.title) : esc('🧩 ' + p.title);
      const run = p.status.running;
      row.innerHTML = `
        <span class="k">${firstMenu} <span class="muted">(${esc(p.name)})</span></span>
        <span>${run ? '<span class="badge badge-ok">' + t('running') + '</span>' : '<span class="badge">' + t('stopped') + '</span>'}
        ${p.menus && p.menus.length ? `<button class="btn btn-ghost btn-sm" data-open="${esc(p.name)}">${t('open')}</button>` : ''}
        <button class="btn btn-ghost btn-sm" data-manage="${esc(p.name)}">${t('manage')}</button></span>`;
      box.appendChild(row);
    });
    box.querySelectorAll('[data-open]').forEach(btn => {
      btn.addEventListener('click', () => {
        const p = list.find(x => x.name === btn.dataset.open);
        if (p && p.menus.length) location.hash = '#/p/' + encodeURIComponent(p.name) + (p.menus[0].path || '/');
      });
    });
    box.querySelectorAll('[data-manage]').forEach(btn => {
      btn.addEventListener('click', () => openDrawer(btn.dataset.manage));
    });
  } catch (err) { toast(err.message, 'err'); }
}

/* ============================================================
   插件页（安装管理）
   ============================================================ */
async function renderPlugins() {
  const box = $('#plugins-manage');
  try {
    const pl = await api('/api/plugins');
    const list = pl.plugins || [];
    if (!list.length) {
      box.innerHTML = '<div class="muted" style="font-size:13px">' + t('plNone') + '</div>';
      return;
    }
    box.innerHTML = '';
    list.forEach(p => {
      const row = document.createElement('div');
      row.className = 'kv';
      row.innerHTML = `
        <span class="k">${esc(p.title)} <span class="muted">v${esc(p.version)} · ${esc(p.name)}</span></span>
        <span>${p.status.running ? '<span class="badge badge-ok">' + t('running') + '</span>' : '<span class="badge">' + t('stopped') + '</span>'}
        ${p.keepalive ? '<span class="badge badge-ok">' + t('keepalive') + '</span>' : ''}
        <button class="btn btn-ghost btn-sm" data-manage="${esc(p.name)}">${t('manage')}</button></span>`;
      box.appendChild(row);
    });
    box.querySelectorAll('[data-manage]').forEach(btn => {
      btn.addEventListener('click', () => openDrawer(btn.dataset.manage));
    });
  } catch (err) { box.innerHTML = '<div class="error-text">' + esc(err.message) + '</div>'; }
}

/* ============================================================
   设置页
   ============================================================ */
async function renderSettings() {
  try {
    const d = await api('/api/settings');
    $('#idle-min').value = d.idle_timeout_minutes;
    $('#port-pool').textContent = d.port_pool || '19000 - 19999';
    // 主题选择器
    const picker = $('#theme-picker');
    picker.innerHTML = '';
    THEMES.forEach(t => {
      const el = document.createElement('div');
      el.className = 'theme-item' + (t.id === (d.theme || 'sage') ? ' active' : '');
      el.dataset.theme = t.id;
      el.innerHTML = `<span class="sw" data-c="${t.id}"></span>${t.label}`;
      picker.appendChild(el);
    });
    // 语言选择器（18 种）
    const langPicker = $('#lang-picker');
    langPicker.innerHTML = '';
    I18N_LANGS.forEach(l => {
      const el = document.createElement('div');
      el.className = 'theme-item' + (l.code === (document.documentElement.lang || 'zh-CN') ? ' active' : '');
      el.dataset.lang = l.code;
      el.textContent = l.name;
      langPicker.appendChild(el);
    });
    // 同步服务端语言
    if (d.lang && I18N[d.lang] && d.lang !== document.documentElement.lang) setLang(d.lang);
    // 面板服务：端口 + 路径
    const m = (d.listen_addr || '').match(/:(\d+)$/);
    $('#listen-port').value = m ? m[1] : '8787';
    $('#set-home').textContent = d.home || '-';
    $('#set-listen').textContent = d.listen_addr || '-';
    // 端口映射表
    const tbody = $('#portmap-body');
    tbody.innerHTML = '';
    const entries = Object.entries(d.port_map || {});
    if (!entries.length) {
      tbody.innerHTML = '<tr><td colspan="4" style="color:var(--muted)">（暂无运行中的插件进程）</td></tr>';
    }
    entries.forEach(([name, v]) => {
      const tr = document.createElement('tr');
      tr.innerHTML = `<td>${esc(name)}</td><td>${esc(v.port)}</td><td>${esc(v.pid)}</td><td>${esc(v.started_at || '-')}</td>`;
      tbody.appendChild(tr);
    });
  } catch (err) { toast(err.message, 'err'); }
  renderAccount();
}

/* 账户与安全：资料、密码策略、会话列表 */
async function renderAccount() {
  try {
    const [acc, sec] = await Promise.all([api('/api/account'), api('/api/security')]);
    $('#acc-username').textContent = acc.username;
    $('#acc-created').textContent = acc.created_at || '-';
    $('#acc-last-login').textContent = acc.last_login_at || '（首次登录）';
    $('#sec-fail').value = sec.fail_limit;
    $('#sec-lock').value = sec.lock_minutes;
    // 会话列表
    const d = await api('/api/account/sessions');
    const tbody = $('#sessions-body');
    tbody.innerHTML = '';
    const active = (d.sessions || []).filter(s => !s.revoked);
    if (!active.length) {
      tbody.innerHTML = '<tr><td colspan="4" style="color:var(--muted)">（暂无活跃会话）</td></tr>';
      return;
    }
    active.forEach(s => {
      const tr = document.createElement('tr');
      tr.innerHTML = `
        <td>${esc(s.created_at)}${s.current ? ' <span class="badge badge-ok">当前</span>' : ''}</td>
        <td class="mono" style="font-family:ui-monospace,monospace">${esc(s.ip || '-')}</td>
        <td class="muted" style="max-width:220px;overflow:hidden;text-overflow:ellipsis;white-space:nowrap">${esc(s.user_agent || '-')}</td>
        <td style="text-align:right">${s.current ? '<span class="muted">-</span>'
          : `<button class="btn btn-danger btn-sm" data-revoke="${esc(s.jti)}">强制下线</button>`}</td>`;
      tbody.appendChild(tr);
    });
    tbody.querySelectorAll('[data-revoke]').forEach(btn => {
      btn.addEventListener('click', async () => {
        if (!confirm('确定强制下线该登录会话？')) return;
        try {
          await api('/api/account/sessions/revoke', {
            method: 'POST',
            body: JSON.stringify({ jti: btn.dataset.revoke }),
          });
          toast('已强制下线该会话');
          renderAccount();
        } catch (e) { toast(e.message, 'err'); }
      });
    });
  } catch (err) { toast(err.message, 'err'); }
}

/* 核心日志查看 */
async function renderCoreLog() {
  try {
    const d = await api('/api/log');
    $('#core-log').textContent = d.log || '（暂无日志）';
    $('#core-log').scrollTop = $('#core-log').scrollHeight;
  } catch (e) { toast(e.message, 'err'); }
}

/* 保存空闲退出时间：立刻生效并持久化 */
$('#btn-save-idle').addEventListener('click', async () => {
  const min = parseInt($('#idle-min').value, 10);
  if (isNaN(min) || min < 1 || min > 1440) { toast('请输入 1-1440 之间的分钟数', 'err'); return; }
  try {
    await api('/api/settings', {
      method: 'PUT',
      body: JSON.stringify({ idle_timeout_minutes: min }),
    });
    toast('已保存：插件空闲 ' + min + ' 分钟后自动退出');
  } catch (err) { toast(err.message, 'err'); }
});

/* ============================================================
   关于页
   ============================================================ */
async function renderAbout() {
  try {
    const st = window.__status || await api('/api/status');
    $('#about-version').textContent = 'v' + st.version;
    $('#about-home').textContent = st.home;
    $('#about-listen').textContent = st.listen_addr;
    const up = st.uptime_seconds || 0;
    const h = Math.floor(up / 3600), m = Math.floor(up % 3600 / 60);
    $('#about-uptime').textContent = h + ' 小时 ' + m + ' 分钟';
  } catch { /* 忽略 */ }
}

/* ---------- 启动 ---------- */
init();
