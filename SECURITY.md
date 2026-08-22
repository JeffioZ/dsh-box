# Security Policy

## Supported versions

安全修复只面向最新发布版本。报告前请先确认问题仍能在最新 Release 复现；不要为了验证而在真实凭据、生产账户或他人设备上进行破坏性测试。

## Private reporting

优先使用 GitHub 仓库的 **Security → Report a vulnerability** 私密报告入口。报告中请包含：

- 受影响版本与操作系统；
- 最小复现步骤和预期/实际结果；
- 影响范围与攻击前提；
- 已脱敏的日志、截图或概念验证。

若私密入口不可用，可创建不含利用细节和敏感数据的普通 issue，请维护者提供私密沟通方式。不要公开 API Key、`.credentials.yaml`、完整日志、会话内容或可直接利用的漏洞细节。

维护者会确认收到报告、评估影响，并在修复可用后协调披露。该项目由个人维护，暂不承诺固定响应时限。

架构信任边界、剩余风险和更新校验范围见 [docs/security.md](docs/security.md)。
