// Package embed 内嵌静态资源：
//   - web/     面板前端（纯 HTML/CSS/JS）
//   - plugins/ 官方插件包（由 build.sh 在编译前生成）
package embed

import "embed"

//go:embed all:web
var Web embed.FS

//go:embed all:plugins
var Plugins embed.FS
