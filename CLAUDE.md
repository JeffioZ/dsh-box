# CLAUDE.md

项目说明见 [AGENTS.md](AGENTS.md)（每次会话自动加载）。要点：本仓库是 DeepSeek Harness (dsh) 的 Tauri v2 桌面外壳——薄外壳定位，不改 dsh 内核；与 dsh 交互只走注入 JS / 读写 `$DSH_HOME` 配置 / 调用 dsh CLI 三条通道。构建用 `build.ps1`，开发用 `dev-build.ps1` + `dev-run.ps1`。
