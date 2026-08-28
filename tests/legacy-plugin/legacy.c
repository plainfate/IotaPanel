/*
 * SPDX-License-Identifier: Apache-2.0
 *
 * legacy-demo.c —— 旧生态兼容性测试插件
 *
 * 目的：模拟"旧 Go 生态"里的预编译插件二进制（任意语言编译的可执行文件）。
 * 旧插件契约只有两条：
 *   1) 核心注入环境变量 PLUGIN_PORT / PLUGIN_BIND / PLUGIN_NAME / PANEL_HOME
 *   2) 监听 PLUGIN_BIND:PLUGIN_PORT，提供 HTTP 服务，经面板网关 /p/<name>/ 反代
 *
 * 本文件用纯 C + libc 实现一个最小 HTTP 服务（无任何第三方依赖），
 * 编译方式见同目录 build.sh（gcc -static）。面板核心不关心插件是什么语言写的，
 * 只按 manifest.yaml 的 command 拉起进程 —— 这就是对旧生态的兼容性证明。
 *
 * 编译:
 *   gcc -static -O2 -o legacy-demo legacy.c
 */
#include <arpa/inet.h>
#include <errno.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/socket.h>
#include <unistd.h>

static char *env_or(const char *key, const char *def) {
    const char *v = getenv(key);
    return (v && *v) ? (char *)v : (char *)def;
}

static void send_all(int fd, const char *data, size_t len) {
    size_t off = 0;
    while (off < len) {
        ssize_t n = write(fd, data + off, len - off);
        if (n <= 0)
            return;
        off += (size_t)n;
    }
}

static void respond(int fd, const char *status, const char *ctype,
                    const char *body, size_t body_len) {
    char head[512];
    int hl = snprintf(head, sizeof(head),
                      "HTTP/1.1 %s\r\n"
                      "Content-Type: %s\r\n"
                      "Content-Length: %zu\r\n"
                      "Connection: close\r\n"
                      "X-Panel-Plugin: legacy-demo\r\n"
                      "\r\n",
                      status, ctype, body_len);
    send_all(fd, head, (size_t)hl);
    send_all(fd, body, body_len);
}

static void handle_client(int fd) {
    char buf[4096];
    ssize_t n = read(fd, buf, sizeof(buf) - 1);
    if (n <= 0) {
        close(fd);
        return;
    }
    buf[n] = '\0';

    /* 取请求行: "GET /api/info HTTP/1.1" */
    char method[16] = {0}, path[512] = {0};
    if (sscanf(buf, "%15s %511s", method, path) != 2) {
        close(fd);
        return;
    }

    if (strcmp(path, "/api/info") == 0) {
        /* 回显核心注入的环境变量 —— 证明旧插件能拿到与 Go 版一致的注入 */
        char body[1024];
        int bl = snprintf(body, sizeof(body),
                          "{\"name\":\"legacy-demo\",\"language\":\"c\","
                          "\"plugin_port\":\"%s\",\"plugin_bind\":\"%s\","
                          "\"plugin_name\":\"%s\",\"panel_home\":\"%s\"}",
                          env_or("PLUGIN_PORT", ""), env_or("PLUGIN_BIND", ""),
                          env_or("PLUGIN_NAME", ""), env_or("PANEL_HOME", ""));
        respond(fd, "200 OK", "application/json; charset=utf-8", body, (size_t)bl);
    } else {
        const char *html =
            "<!DOCTYPE html><html><head><meta charset=\"utf-8\">"
            "<title>legacy-demo</title></head><body>"
            "<h1>Legacy Plugin OK</h1>"
            "<p>This is a legacy-style plugin (precompiled C binary) "
            "running under the Rust IotaPanel core.</p>"
            "</body></html>";
        respond(fd, "200 OK", "text/html; charset=utf-8", html, strlen(html));
    }
    close(fd);
}

int main(void) {
    const char *bind_addr = env_or("PLUGIN_BIND", "127.0.0.1");
    int port = atoi(env_or("PLUGIN_PORT", "19000"));
    if (port <= 0)
        port = 19000;

    int srv = socket(AF_INET, SOCK_STREAM, 0);
    if (srv < 0) {
        perror("socket");
        return 1;
    }
    int one = 1;
    setsockopt(srv, SOL_SOCKET, SO_REUSEADDR, &one, sizeof(one));

    struct sockaddr_in addr;
    memset(&addr, 0, sizeof(addr));
    addr.sin_family = AF_INET;
    addr.sin_port = htons((uint16_t)port);
    if (inet_pton(AF_INET, bind_addr, &addr.sin_addr) != 1) {
        addr.sin_addr.s_addr = htonl(INADDR_LOOPBACK);
    }
    if (bind(srv, (struct sockaddr *)&addr, sizeof(addr)) < 0) {
        perror("bind");
        return 1;
    }
    if (listen(srv, 16) < 0) {
        perror("listen");
        return 1;
    }

    fprintf(stderr, "[legacy-demo] listening on %s:%d\n", bind_addr, port);

    for (;;) {
        int fd = accept(srv, NULL, NULL);
        if (fd < 0)
            continue;
        handle_client(fd);
    }
    return 0;
}
