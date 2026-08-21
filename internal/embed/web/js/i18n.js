// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 plainfate <https://github.com/plainfate>
/* ============================================================
   MicroPanel 国际化（中英双语）
   用法：
     setLang(code)  切换语言（自动翻译 [data-i18n] 元素）
     t('key')       取当前语言的字符串（app.js 动态内容用）
   语言持久化：localStorage('mp-lang') + 服务端 settings.lang
   ============================================================ */
const I18N_LANGS = [
  { code: 'zh-CN', name: '简体中文' },
  { code: 'en', name: 'English' },
];

const I18N = {
'zh-CN': {
  overview:'概览', plugins:'插件', settings:'设置', about:'关于', system:'系统', logout:'退出',
  running:'运行中', stopped:'已停止', keepalive:'保活', manage:'管理', open:'打开',
  ovTitle:'概览', ovSub:'面板核心状态与已安装插件一览。', ovVersion:'版本', ovUptime:'运行时长',
  ovPlugins:'插件', ovHome:'安装目录', ovListen:'监听地址', ovIdle:'空闲超时', ovInstalled:'已安装插件', ovNone:'（未安装插件）',
  plTitle:'插件', plSub:'从 URL / GitHub Release 安装插件包（.tar.gz，自动 SHA256 校验）；也可手动放入 plugins/ 目录后重启面板自动登记。',
  plUrlInstall:'从 URL / GitHub 安装', plUrlPh:'https://github.com/xxx/plugin/releases/download/v1/plugin.tar.gz',
  plShaPh:'SHA256（可选）', plDownload:'下载并安装', plManualHint:'GitHub Release 的 .tar.gz 直链可直接粘贴；手动安装：把插件目录复制到 PANEL_HOME/plugins/<name>/ 后 panel restart。',
  plNone:'（未安装插件，可从上方的 URL 安装，或手动放入 plugins/ 目录）',
  stTitle:'设置', stSub:'外观、账户安全、面板服务与插件策略。', thTitle:'外观主题（莫奈配色预设）', lang:'语言',
  acTitle:'账户与安全', acUsername:'用户名', acCreated:'创建时间', acLastLogin:'最近登录',
  acChangePw:'修改密码（改密后其他登录会话将强制下线）', acOldPw:'原密码', acNewPw:'新密码（至少 6 位）', acConfirmPw:'确认新密码',
  acChangePwBtn:'修改密码', acPolicy:'登录安全策略', acFailLimit:'密码错误次数上限', acLockMinutes:'锁定时间（分钟）',
  acSavePolicy:'保存策略', acSessions:'登录会话（可强制下线）', acRevokeAll:'下线其他全部会话', acRevoke:'强制下线',
  acTime:'登录时间', acIp:'IP', acBrowser:'浏览器', acAction:'操作',
  svTitle:'面板服务', svListenPort:'监听端口（保存后重启生效）', svSavePort:'保存端口', svInstallDir:'安装目录（PANEL_HOME）', svCurrentListen:'当前监听地址',
  soTitle:'系统操作', soRestart:'重启面板', soRestartHint:'等价于命令行 panel restart（或 systemctl restart micropanel）',
  soCoreLog:'核心日志（logs/panel.log）', soRefreshLog:'刷新日志',
  ppTitle:'插件策略', ppIdleTimeout:'空闲退出时间（分钟）', ppSave:'保存', ppPortMapFile:'端口映射文件', ppPortPool:'端口池',
  ppPortMapTable:'端口映射表（port-map.json）', ppPlugin:'插件', ppPort:'端口', ppPid:'PID', ppStartedAt:'启动时间',
  drKeepalive:'后台保活', drStart:'启动', drStop:'停止', drRestart:'重启', drLog:'插件日志', drUninstall:'卸载插件', drDanger:'危险操作',
  abTitle:'关于 MicroPanel', abSub:'极简微内核 · 进程级隔离 · 按需启动 —— 服务器领域的「Chrome 浏览器」。',
  lgTitle:'登录', lgUsername:'用户名', lgPassword:'密码', lgRemember:'记住我（30 天内免登录）', lgBtn:'登 录',
  spWelcome:'首次使用，请创建你自己的管理员账号（系统无默认账号）', spStep1:'创建你的管理员账号',
  spUsernamePh:'设置你的用户名（至少 3 个字符）', spPasswordPh:'至少 6 位', spConfirmPw:'确认密码',
  spNext:'创建账号并继续', spStep2:'选择基础功能插件', spStep2Sub:'可自由勾选，或跳过进入极简模式（随时可在「插件」页安装）',
  spInstall:'开始安装', spSkip:'跳过，进入极简模式', spStep3:'正在安装…', spPreparing:'准备中…', spDone:'完成',
},

'en': {
  overview:'Overview', plugins:'Plugins', settings:'Settings', about:'About', system:'System', logout:'Logout',
  running:'Running', stopped:'Stopped', keepalive:'Keep-alive', manage:'Manage', open:'Open',
  ovTitle:'Overview', ovSub:'Panel core status and installed plugins.', ovVersion:'Version', ovUptime:'Uptime',
  ovPlugins:'Plugins', ovHome:'Install dir', ovListen:'Listen addr', ovIdle:'Idle timeout', ovInstalled:'Installed plugins', ovNone:'(no plugins installed)',
  plTitle:'Plugins', plSub:'Install a plugin package (.tar.gz) from a URL / GitHub Release with optional SHA-256 check, or drop a folder into plugins/ and restart.',
  plUrlInstall:'Install from URL / GitHub', plUrlPh:'https://github.com/xxx/plugin/releases/download/v1/plugin.tar.gz',
  plShaPh:'SHA256 (optional)', plDownload:'Download & install', plManualHint:'Paste a .tar.gz direct link (GitHub Releases work). Manual: copy the plugin folder to PANEL_HOME/plugins/<name>/ then run panel restart.',
  plNone:'(no plugins installed — use the URL above or drop a folder into plugins/)',
  stTitle:'Settings', stSub:'Appearance, account security, panel service and plugin policy.', thTitle:'Theme (Monet presets)', lang:'Language',
  acTitle:'Account & Security', acUsername:'Username', acCreated:'Created', acLastLogin:'Last login',
  acChangePw:'Change password (other sessions will be signed out)', acOldPw:'Current password', acNewPw:'New password (min 6 chars)', acConfirmPw:'Confirm new password',
  acChangePwBtn:'Change password', acPolicy:'Login security policy', acFailLimit:'Max failed attempts', acLockMinutes:'Lockout (minutes)',
  acSavePolicy:'Save policy', acSessions:'Sessions (revocable)', acRevokeAll:'Sign out all other sessions', acRevoke:'Revoke',
  acTime:'Login time', acIp:'IP', acBrowser:'Browser', acAction:'Action',
  svTitle:'Panel Service', svListenPort:'Listen port (restart to apply)', svSavePort:'Save port', svInstallDir:'Install dir (PANEL_HOME)', svCurrentListen:'Current listen addr',
  soTitle:'System Actions', soRestart:'Restart panel', soRestartHint:'Same as panel restart (or systemctl restart micropanel)',
  soCoreLog:'Core log (logs/panel.log)', soRefreshLog:'Refresh log',
  ppTitle:'Plugin Policy', ppIdleTimeout:'Idle exit time (minutes)', ppSave:'Save', ppPortMapFile:'Port map file', ppPortPool:'Port pool',
  ppPortMapTable:'Port map (port-map.json)', ppPlugin:'Plugin', ppPort:'Port', ppPid:'PID', ppStartedAt:'Started at',
  drKeepalive:'Keep alive', drStart:'Start', drStop:'Stop', drRestart:'Restart', drLog:'Plugin log', drUninstall:'Uninstall', drDanger:'Danger zone',
  abTitle:'About MicroPanel', abSub:'Microkernel · process isolation · lazy start — the "Chrome browser" for servers.',
  lgTitle:'Sign in', lgUsername:'Username', lgPassword:'Password', lgRemember:'Remember me (30 days)', lgBtn:'Sign in',
  spWelcome:'Create your own admin account (no default account)', spStep1:'Create your admin account',
  spUsernamePh:'Pick a username (min 3 chars)', spPasswordPh:'min 6 chars', spConfirmPw:'Confirm password',
  spNext:'Create & continue', spStep2:'Choose basic plugins', spStep2Sub:'Pick freely, or skip for minimal mode (install later in Plugins page)',
  spInstall:'Install', spSkip:'Skip, minimal mode', spStep3:'Installing…', spPreparing:'Preparing…', spDone:'Done',
},
};

/* ---------- 运行时 ---------- */
let _curLang = 'zh-CN';

function detectLang() {
  const saved = localStorage.getItem('mp-lang');
  if (saved && I18N[saved]) return saved;
  const nav = (navigator.language || 'en').toLowerCase();
  return nav.startsWith('zh') ? 'zh-CN' : 'en';
}

function t(key) {
  const dict = I18N[_curLang] || I18N['zh-CN'];
  return dict[key] || I18N['zh-CN'][key] || key;
}

function setLang(code) {
  _curLang = I18N[code] ? code : 'zh-CN';
  localStorage.setItem('mp-lang', _curLang);
  document.documentElement.lang = _curLang;
  applyStaticI18n();
  document.dispatchEvent(new CustomEvent('langchange', { detail: _curLang }));
}

/* 翻译静态 [data-i18n] 元素（支持 input 用 data-i18n-ph 翻译占位符） */
function applyStaticI18n() {
  document.querySelectorAll('[data-i18n]').forEach(el => {
    const key = el.dataset.i18n;
    const txt = t(key);
    if (el.dataset.i18nPh !== undefined) el.setAttribute('placeholder', txt);
    else el.textContent = txt;
  });
  document.querySelectorAll('[data-lang]').forEach(el => {
    el.classList.toggle('active', el.dataset.lang === _curLang);
  });
}

/* 自动初始化 */
(function () {
  const lang = localStorage.getItem('mp-lang') || detectLang();
  setLang(lang);
})();
