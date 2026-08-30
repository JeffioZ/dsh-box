# morphicons（vendored）

图标弹簧变形动画库，MIT 许可。本目录内嵌其 npm 构建产物原样副本：

- `dom.js` —— DOM 绑定（`createMorph`），本仓库唯一入口
- `spring-CFHloqPP.js`、`normalize-CYnN3Npw.js` —— 其内部 chunk（相对导入，勿改名）
- `LICENSE` —— 上游许可证全文

- 上游：<https://github.com/guillermolg00/morphicons>
- 版本：1.7.1（npm 包 `morphicons`）
- 接入方式：经典脚本内动态 `import('vendor/morphicons/dom.js')`（相对页面解析，
  页面都在 ui/ 根）；加载失败回退为直接替换 path 的 `d`，交互不降级。
- 当前使用点：主菜单开合箭头（下⇄上，`ui/titlebar.js`）、密码可见性眼睛
  （`ui/common.js` 的 `dshdBindPasswordToggle`）。
- 使用约束：仅限描边式、同 24×24 网格的图标配对；`reducedMotion: 'user'`
  跟随系统减弱动效偏好（本仓库强制约定）。

升级：替换上述三个 dist 文件（文件名含内容哈希，随版本变化），更新本文件版本号
与 `THIRD_PARTY_NOTICES.md` 中的条目。
