// 核心前端静态资源表（由 embed.rs 通过 include_str! 提供，编译期直接嵌入）。
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
    asset!("index.html", "index.html"),
    asset!("login.html", "login.html"),
    asset!("setup.html", "setup.html"),
    asset!("favicon.svg", "favicon.svg"),
    asset!("css/app.css", "css/app.css"),
    asset!("js/i18n.js", "js/i18n.js"),
    asset!("js/app.js", "js/app.js"),
    asset!("plugins/hello/manifest.yaml", "../plugins/hello/manifest.yaml"),
    asset!("plugins/file-manager/manifest.yaml", "../plugins/file-manager/manifest.yaml"),
    asset!("plugins/resource-monitor/manifest.yaml", "../plugins/resource-monitor/manifest.yaml"),
    asset!("plugins/terminal/manifest.yaml", "../plugins/terminal/manifest.yaml"),
    asset!("plugins/https-front/manifest.yaml", "../plugins/https-front/manifest.yaml"),
    asset!("plugins/mcp-agent/manifest.yaml", "../plugins/mcp-agent/manifest.yaml"),
];
