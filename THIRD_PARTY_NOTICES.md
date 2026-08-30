# Third-party notices

DSHBox 自身采用 MIT License。仓库、构建工具、编译产物和运行时下载内容还涉及第三方组件；它们分别遵循各自许可证与商标政策。

## 编译与构建依赖

- Rust 依赖及其精确版本以 `src-tauri/Cargo.lock` 为准，许可证元数据可由 `cargo metadata --locked --format-version 1` 查看。
- npm 开发依赖及其精确版本以 `package-lock.json` 为准；`@tauri-apps/cli` 用于构建，`@resvg/resvg-js` 用于生成图标。
- WebView2、WebKit 与 WebKitGTK 由操作系统或系统运行时提供，不由本仓库重新许可。

锁文件中的许可证元数据包含 MIT、Apache-2.0、BSD、ISC、MPL-2.0、Unicode-3.0、Zlib、CDLA-Permissive-2.0 等标识。发布者在分发二进制前仍应使用依赖审计工具从当前锁文件生成并核对完整 notices；本摘要不能替代各包随附的许可证文本，也不判断具体分发方式下的许可证兼容性。

## 运行时下载

- [DeepSeek Harness](https://github.com/deepseek-ai/deepseek-harness) 与 Node.js 在用户设备上按需下载/安装，不以源码形式收录在本仓库。
- `src-tauri/resources/builtin-plugins.json` 中的插件只有在首次引导获得用户选择后，才通过官方 `dsh plugin` CLI 安装。插件许可证与条款以各自仓库和 npm 包为准。

## 品牌资源

`assets/brand/deepseek-mark.svg` 是生成应用与托盘图标的品牌源。DeepSeek 名称、标志及相关商标归其权利人所有；本项目的 MIT License 不授予第三方商标权。再分发或改名发行前，应自行确认品牌使用授权与政策。

## dsh-usage-stats

「用量与余额」功能衍生自此项目，按 MIT 条款保留版权声明。衍生范围：聚合模块 `src-tauri/src/usage/aggregate.rs`（逐函数移植）、`usage/pricing.rs`（DeepSeek 官方历史定价目录与峰谷判定移植）、`usage/export.rs`（导出投影：BOM/转义/公式防护/schema 版本化）、`usage/balance.rs` 与 `usage/subscriptions.rs`（适配器契约与解析逻辑参考，含 OrcaRouter / New API / Sub2API）、`usage/providers.rs`（供应商枚举口径）、控制中心用量页结构（视觉为本项目设计体系）。与上游的同步规程见 [docs/usage-sync.md](docs/usage-sync.md)。

- 项目：https://github.com/Ychris12138/dsh-usage-stats
- 许可证：MIT

```
MIT License

Copyright (c) 2026 dsh-usage-stats contributors

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```
