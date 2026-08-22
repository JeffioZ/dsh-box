# Contributing

感谢参与 DSHBox。开始前请先阅读 [项目边界](README.md#项目边界)、[架构](docs/architecture.md)与[开发指南](docs/development.md)。

## 提交改动

1. 先搜索已有 issue，较大的功能或会改变用户习惯的交互先讨论。
2. 保持薄外壳边界：不 fork、patch 或复制 dsh 内核与 Web UI。
3. 使用 Conventional Commits，例如 `fix: ...`、`feat: ...`、`docs: ...`。
4. 新逻辑补测试；新文案补齐中英文；修改 UI 检查深浅色和减弱动态效果。
5. 提交前运行：

```powershell
npm run check
cargo fmt --manifest-path src-tauri/Cargo.toml -- --check
cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings
cargo test --manifest-path src-tauri/Cargo.toml --all-targets
```

## Pull request

PR 请说明用户影响、设计取舍、测试结果和已知负面影响。涉及右键菜单、更新、凭据、插件自动安装或文件替换时，请单列回归与恢复方案。不要提交构建产物、日志、凭据或私人会话数据。

安全问题不要提交公开 issue 或 PR，按 [SECURITY.md](SECURITY.md) 私密报告。
