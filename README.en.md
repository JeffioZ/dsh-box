# DSHBox

[中文](README.md) · [Architecture](docs/architecture.md) · [Development](docs/development.md) · [Security model](docs/security.md)

DSHBox (`dsh-box`) is a cross-platform desktop shell for [DeepSeek Harness](https://github.com/deepseek-ai/deepseek-harness), built with Tauri v2, Rust, and plain HTML/CSS/JavaScript. It loads the official `dsh web` UI and adds runtime management, windows, tray integration, updates, and local desktop actions without forking or patching dsh.

> Release artifacts are currently unsigned. Windows SmartScreen, macOS Gatekeeper, or Linux security policies may require manual approval.

## Highlights

- Detects or installs Node.js and dsh; guides Windows users through WebView2 setup when needed.
- Manages a single dsh service with port fallback, child-process cleanup, watchdog recovery, and page heartbeat recovery.
- Provides custom title/status bars, tray, notifications, window-state persistence, light/dark themes, and Chinese/English UI.
- Uses transactional dsh/Node updates; Windows app updates require an exact release tag and SHA-256 digest.
- Keeps the full local-file context menu: default open, editor open, reveal, copy path, and copy UTF-8 contents.
- Manages plugins only through the official `dsh plugin` CLI. Recommended first-run plugins are disclosed and optional.
- Validates `llm-pi-ai` imports with a typed YAML parser and stores credentials separately from settings.

## Platforms

| Platform | Architecture | Minimum environment | Artifact |
|---|---|---|---|
| Windows | x64 | Windows 10 and WebView2 Runtime | Single `DSHBox.exe` |
| macOS | arm64 / x64 | macOS 13.5+ | Unsigned dmg |
| Linux | x64 / arm64 | WebKitGTK 4.1 environment such as Ubuntu 22.04 or Debian 12 | zip |

Download the appropriate artifact from [Releases](https://github.com/JeffioZ/dsh-box/releases). Windows is the primary locally tested platform; GitHub Actions builds all five targets.

## First run and data

The first-run page is optional. API keys are written to dsh's `$DSH_HOME/.credentials.yaml`, while language and theme preferences are merged into `$DSH_HOME/settings.yaml`. The recommended-plugin switch is enabled by default but can be turned off; skipping setup does not consent to automatic plugin installation.

The key can later be replaced or cleared under **Desktop settings → DeepSeek credential**. When an environment variable supplies the key, that section is read-only and identifies the external owner.

DSHBox stores user-editable shell settings in `config.json` and internal window/onboarding/maintenance state in `state.json`. Those files have strict ownership boundaries and are not used as fallbacks for each other. API keys are never read from or written to `config.json`.

Default app-data roots are `%LOCALAPPDATA%\DSHBox` on Windows, `~/Library/Application Support/com.deepseek.dsh-box` on macOS, and `$XDG_DATA_HOME/com.deepseek.dsh-box` on Linux. dsh data remains in the official `$DSH_HOME` location, normally `~/.dsh`.

Environment overrides: `DSH_BOX_ROOT`, `DSH_BOX_PORT`, `DSH_BOX_API_KEY`, `DSH_BOX_API_BASE`, `DSH_HOME`, and `DSHD_LANG`. API key precedence is `DSH_BOX_API_KEY`, `DEEPSEEK_API_KEY`, then dsh credentials.

The settings page also controls whether closing hides to the tray or quits and whether a normal launch opens the window or stays in the tray. On the first-install screen, downloads can use automatic fallback, official sources only, or mirrors only; the source can be switched and installation cancelled without terminating the app.

## Development

Install Rust 1.85+, Node.js, and the platform-specific Tauri dependencies, then run:

```powershell
npm install
npm run check
cargo fmt --manifest-path src-tauri/Cargo.toml -- --check
cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings
cargo test --manifest-path src-tauri/Cargo.toml --all-targets
```

On Windows:

```powershell
pwsh -NoLogo -NoProfile -File .\build.ps1
pwsh -NoLogo -NoProfile -File .\dev-build.ps1
pwsh -NoLogo -NoProfile -File .\dev-run.ps1
```

The project deliberately has no frontend bundler. `ui/` contains page-specific HTML/CSS/JS, shared design tokens and utilities, and a single bilingual message table. See [the development guide](docs/development.md) for release and validation details.

## Boundary

DSHBox interacts with dsh through only three channels: scripts injected into the trusted local dsh page, narrowly scoped reads/writes under `$DSH_HOME`, and official `dsh` CLI commands. It does not reimplement the dsh conversation UI, patch its package, or change its session format.

See [CONTRIBUTING.md](CONTRIBUTING.md), [SECURITY.md](SECURITY.md), and [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md). DSHBox source code is licensed under the [MIT License](LICENSE).
