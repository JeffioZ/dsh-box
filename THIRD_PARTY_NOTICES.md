# Third-party notices

DSHBox 自身采用 MIT License。仓库、构建工具、编译产物和运行时下载内容还涉及第三方组件；它们分别遵循各自许可证与商标政策。

## 编译与构建依赖

- Rust 依赖及其精确版本以 `src-tauri/Cargo.lock` 为准，许可证元数据可由 `cargo metadata --locked --format-version 1` 查看。
- npm 开发依赖及其精确版本以 `package-lock.json` 为准；`@tauri-apps/cli` 用于构建，`@resvg/resvg-js` 用于生成图标。
- WebView2、WebKit 与 WebKitGTK 由操作系统或系统运行时提供，不由本仓库重新许可。

锁文件中的许可证元数据包含 MIT、Apache-2.0、BSD、ISC、MPL-2.0、Unicode-3.0、Zlib、CDLA-Permissive-2.0 等兼容组合。发布者在分发二进制前仍应使用依赖审计工具从当前锁文件生成并核对完整 notices；本摘要不能替代各包随附的许可证文本。

## 运行时下载

- [DeepSeek Harness](https://github.com/deepseek-ai/deepseek-harness) 与 Node.js 在用户设备上按需下载/安装，不以源码形式收录在本仓库。
- `src-tauri/resources/builtin-plugins.json` 中的插件只有在首次引导获得用户选择后，才通过官方 `dsh plugin` CLI 安装。插件许可证与条款以各自仓库和 npm 包为准。

## 品牌资源

`assets/brand/deepseek-mark.svg` 是生成应用与托盘图标的品牌源。DeepSeek 名称、标志及相关商标归其权利人所有；本项目的 MIT License 不授予第三方商标权。再分发或改名发行前，应自行确认品牌使用授权与政策。
