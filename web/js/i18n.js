/* IotaPanel i18n —— 中/英双语 */
const I18N = {
  zh: {
    "nav.overview":"概览","nav.store":"插件商店","nav.settings":"设置",
    "sec-system":"系统","sec-plugins":"插件",
    "loading":"加载中…",
    // overview
    "ov.card.status":"运行状态","ov.card.stats":"概览","ov.card.plugins":"已安装插件",
    "ov.version":"版本","ov.listen":"监听地址","ov.uptime":"运行时长","ov.home":"数据目录",
    "ov.idle":"空闲退出","ov.plugins_installed":"已装插件","ov.plugins_running":"运行中",
    "ov.days":"天","ov.hours":"小时","ov.minutes":"分钟",
    "ov.plugin.status":"状态","ov.plugin.title":"名称","ov.plugin.version":"版本","ov.plugin.author":"作者",
    "ov.plugin.desc":"说明","ov.plugin.actions":"操作","ov.plugin.start":"启动","ov.plugin.stop":"停止",
    "ov.plugin.restart":"重启","ov.plugin.keepalive":"保活","ov.plugin.uninstall":"卸载","ov.plugin.log":"日志",
    "ov.no_plugins":"尚未安装插件，前往商店安装", 
    "plugin.running":"运行中","plugin.stopped":"已停止","plugin.error":"异常",
    // store
    "store.title":"插件商店","store.sub":"安装更多功能插件",
    "store.installed":"已安装","store.install":"安装","store.install_url":"通过 URL 安装",
    "store.url_placeholder":"https://…/plugin.tar.gz","store.sha_placeholder":"sha256 校验值（可选）",
    "store.no_store":"商店暂无可用插件","store.search":"搜索插件…",
    // settings
    "st.title":"设置","st.tabs.general":"常规","st.tabs.security":"安全","st.tabs.theme":"外观","st.tabs.about":"关于",
    "st.idle":"空闲无操作后自动停止插件（分钟）",
    "st.save":"保存设置","st.saved":"已保存","st.need_restart":"监听端口已更新，重启后生效",
    "st.listen_port":"监听端口",
    "st.theme":"主题","st.theme.sage":"松绿","st.theme.ocean":"海蓝","st.theme.rose":"玫粉","st.theme.lilac":"丁香",
    "st.lang":"界面语言","st.lang.zh":"简体中文","st.lang.en":"English",
    "st.fail_limit":"登录失败次数上限","st.lock_minutes":"失败后锁定（分钟）",
    "st.about":"关于","st.version":"版本","st.port_pool":"插件端口池",
    // account
    "ac.title":"账号设置","ac.username":"用户名","ac.change_username":"更换用户名",
    "ac.new_username":"新用户名","ac.password":"修改密码","ac.save":"保存",
    "ac.old_pass":"当前密码","ac.new_pass":"新密码","ac.sessions":"登录会话",
    "ac.current":"当前会话","ac.revoke":"下线","ac.revoke_all":"下线其它所有会话",
    "ac.created":"创建时间","ac.last_login":"最近登录","ac.no_sessions":"暂无会话记录",
    // log
    "log.title":"运行日志","log.refresh":"刷新","log.empty":"日志为空",
    // common
    "common.confirm":"确认","common.cancel":"取消","common.ok":"确定","common.error":"操作失败",
    "common.logout":"退出登录","common.nav.account":"账号设置","common.nav.sessions":"登录会话","common.nav.log":"运行日志",
    "common.logout_ok":"已退出登录","common.unauthorized":"登录状态已失效，请重新登录",
    "common.sec_ago":"刚刚","common.min_ago":" 分钟前","common.hr_ago":" 小时前",
    "format.bytes":"B"
  },
  en: {
    "nav.overview":"Overview","nav.store":"Store","nav.settings":"Settings",
    "sec-system":"System","sec-plugins":"Plugins",
    "loading":"Loading…",
    "ov.card.status":"Status","ov.card.stats":"Overview","ov.card.plugins":"Installed Plugins",
    "ov.version":"Version","ov.listen":"Listen address","ov.uptime":"Uptime","ov.home":"Data dir",
    "ov.idle":"Idle timeout","ov.plugins_installed":"Installed","ov.plugins_running":"Running",
    "ov.days":"d","ov.hours":"h","ov.minutes":"m",
    "ov.plugin.status":"Status","ov.plugin.title":"Name","ov.plugin.version":"Version","ov.plugin.author":"Author",
    "ov.plugin.desc":"Description","ov.plugin.actions":"Actions","ov.plugin.start":"Start","ov.plugin.stop":"Stop",
    "ov.plugin.restart":"Restart","ov.plugin.keepalive":"Keep-alive","ov.plugin.uninstall":"Uninstall","ov.plugin.log":"Log",
    "ov.no_plugins":"No plugins installed. Go to Store.",
    "plugin.running":"Running","plugin.stopped":"Stopped","plugin.error":"Error",
    "store.title":"Plugin Store","store.sub":"Install more feature plugins",
    "store.installed":"Installed","store.install":"Install","store.install_url":"Install from URL",
    "store.url_placeholder":"https://…/plugin.tar.gz","store.sha_placeholder":"sha256 (optional)",
    "store.no_store":"Store is empty","store.search":"Search…",
    "st.title":"Settings","st.tabs.general":"General","st.tabs.security":"Security","st.tabs.theme":"Appearance","st.tabs.about":"About",
    "st.idle":"Stop idle plugins after (minutes)",
    "st.save":"Save","st.saved":"Saved","st.need_restart":"Listen port updated, restart required to apply",
    "st.listen_port":"Listen port",
    "st.theme":"Theme","st.theme.sage":"Sage","st.theme.ocean":"Ocean","st.theme.rose":"Rose","st.theme.lilac":"Lilac",
    "st.lang":"Language","st.lang.zh":"简体中文","st.lang.en":"English",
    "st.fail_limit":"Max login failures","st.lock_minutes":"Lock for (minutes)",
    "st.about":"About","st.version":"Version","st.port_pool":"Plugin port pool",
    "ac.title":"Account","ac.username":"Username","ac.change_username":"Change username",
    "ac.new_username":"New username","ac.password":"Change password","ac.save":"Save",
    "ac.old_pass":"Current password","ac.new_pass":"New password","ac.sessions":"Sessions",
    "ac.current":"This session","ac.revoke":"Revoke","ac.revoke_all":"Revoke all other sessions",
    "ac.created":"Created","ac.last_login":"Last login","ac.no_sessions":"No sessions",
    "log.title":"Logs","log.refresh":"Refresh","log.empty":"Empty",
    "common.confirm":"Confirm","common.cancel":"Cancel","common.ok":"OK","common.error":"Failed",
    "common.logout":"Logout","common.nav.account":"Account","common.nav.sessions":"Sessions","common.nav.log":"Logs",
    "common.logout_ok":"Logged out","common.unauthorized":"Session expired. Please sign in.",
    "common.sec_ago":"just now","common.min_ago":" min ago","common.hr_ago":" hr ago",
    "format.bytes":"B"
  }
};
window.I18N = I18N;

function __t(key){
  const dict = I18N[window.IOTA_LANG] || I18N.zh;
  return dict[key] != null ? dict[key] : (I18N.zh[key] != null ? I18N.zh[key] : key);
}
window.__t = __t;
