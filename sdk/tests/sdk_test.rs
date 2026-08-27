use iotapanel_sdk::http;
use iotapanel_sdk::ws;

#[test]
fn urlcodec_roundtrip() {
    for s in ["hello", "中文测试", "a b&c=d", "100%"] {
        assert_eq!(http::urldecode(&http::urlencode(s)), s);
    }
    assert_eq!(http::urldecode("a+b"), "a b");
}

#[test]
fn query_parse() {
    let q = http::parse_query("path=/tmp/x&name=%E4%B8%AD%E6%96%87");
    assert_eq!(q.get("path").unwrap(), "/tmp/x");
    assert_eq!(q.get("name").unwrap(), "中文");
}

#[test]
fn yaml_manifest() {
    let y = iotapanel_sdk::util::parse_yaml(
        r#"
name: terminal
title: 终端
version: 0.1.0
keepalive: true
auth: none
bind: 127.0.0.1
menus:
  - title: 终端
    icon: 💻
    path: /
    section: system
"#,
    );
    assert_eq!(y.str_or("name", ""), "terminal");
    assert_eq!(y.str_or("title", ""), "终端");
    assert!(y.bool_or("keepalive", false));
    assert_eq!(y.str_or("auth", ""), "none");
    let menus = y.list_map("menus");
    assert_eq!(menus.len(), 1);
    assert_eq!(menus[0].iter().find(|(k, _)| k == "icon").unwrap().1.as_str().unwrap(), "💻");
    // section 键
    assert_eq!(
        menus[0]
            .iter()
            .find(|(k, _)| k == "section")
            .map(|(_, v)| v.as_str().unwrap().to_string())
            .unwrap(),
        "system"
    );
}

#[test]
fn ws_accept_key_rfc6455_example() {
    // RFC 6455 §1.3 的示例
    assert_eq!(
        ws::accept_key("dGhlIHNhbXBsZSBub25jZQ=="),
        "s3pPLMBiTxaQ9kYGzzhZRbK+xOo="
    );
}

#[test]
fn multipart_parse() {
    let body = b"--XX\r\nContent-Disposition: form-data; name=\"file\"; filename=\"a.txt\"\r\nContent-Type: text/plain\r\n\r\nhello\r\nworld\r\n--XX--\r\n";
    let parts = http::parse_multipart("multipart/form-data; boundary=XX", body).unwrap();
    assert_eq!(parts.len(), 1);
    assert_eq!(parts[0].name, "file");
    assert_eq!(parts[0].filename.as_deref(), Some("a.txt"));
    assert_eq!(parts[0].data, b"hello\r\nworld");
}

#[test]
fn civil_time() {
    assert_eq!(iotapanel_sdk::util::rfc3339(0), "1970-01-01T00:00:00Z");
    assert_eq!(iotapanel_sdk::util::rfc3339(1735689600), "2025-01-01T00:00:00Z");
}
