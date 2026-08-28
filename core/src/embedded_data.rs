// 核心内嵌资源表（由 scripts/gen-embedded.py 自动生成，勿手改；改资源后重跑 build.sh）。
#![allow(dead_code)]
pub struct EmbeddedFile {
    pub path: &'static str,
    pub data: &'static [u8],
}

macro_rules! asset {
    ($rel:expr, $file:expr) => {
        EmbeddedFile { path: $rel, data: include_bytes!(concat!("../../web/", $file)) }
    };
}

pub static FILES: &[EmbeddedFile] = &[
    asset!("favicon.svg", "favicon.svg"),
    asset!("index.html", "index.html"),
    asset!("login.html", "login.html"),
    asset!("setup.html", "setup.html"),
    asset!("css/app.css", "css/app.css"),
    asset!("js/app.js", "js/app.js"),
    asset!("js/i18n.js", "js/i18n.js"),
    asset!("plugins/hello/manifest.yaml", "../plugins/hello/manifest.yaml"),
    asset!("plugins/hello/bin/hello.gz", "../plugins/hello/bin/hello.gz"),
    asset!("plugins/hello/web/index.html", "../plugins/hello/web/index.html"),
    asset!("plugins/https-front/manifest.yaml", "../plugins/https-front/manifest.yaml"),
    asset!("plugins/https-front/bin/https-front.gz", "../plugins/https-front/bin/https-front.gz"),
    asset!("plugins/https-front/web/index.html", "../plugins/https-front/web/index.html"),
    asset!("plugins/mcp-agent/manifest.yaml", "../plugins/mcp-agent/manifest.yaml"),
    asset!("plugins/mcp-agent/bin/mcp-agent.gz", "../plugins/mcp-agent/bin/mcp-agent.gz"),
    asset!("plugins/mcp-agent/web/index.html", "../plugins/mcp-agent/web/index.html"),
    asset!("plugins/resource-monitor/manifest.yaml", "../plugins/resource-monitor/manifest.yaml"),
    asset!("plugins/resource-monitor/bin/resource-monitor.gz", "../plugins/resource-monitor/bin/resource-monitor.gz"),
    asset!("plugins/resource-monitor/web/index.html", "../plugins/resource-monitor/web/index.html"),
    asset!("plugins/terminal/manifest.yaml", "../plugins/terminal/manifest.yaml"),
    asset!("plugins/terminal/bin/terminal.gz", "../plugins/terminal/bin/terminal.gz"),
    asset!("plugins/terminal/web/index.html", "../plugins/terminal/web/index.html"),
    asset!("plugins/terminal/web/lib/fit.js", "../plugins/terminal/web/lib/fit.js"),
    asset!("plugins/terminal/web/lib/xterm.css", "../plugins/terminal/web/lib/xterm.css"),
    asset!("plugins/terminal/web/lib/xterm.js", "../plugins/terminal/web/lib/xterm.js"),
];
