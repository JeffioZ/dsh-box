// DSHBox 共享前端工具与轻量中英文本地化。
// 各窗口页面无打包器，共享脚本以普通 <script src="common.js"> 引用。

let DSHD_LANGUAGE = (() => {
  const preferred = String(
    // 与 dsh 的兜底一致：无注入语言、浏览器语言也不是 zh/en 时落产品默认 zh
    window.__DSHD_LANG || (navigator.languages && navigator.languages[0]) || navigator.language || 'zh-CN',
  ).toLowerCase();
  return preferred.startsWith('zh') ? 'zh-CN' : 'en';
})();

// —— 双保险：window.__TAURI__ 未注入时（如 app.withGlobalTauri 被误删），
//    页面不会在解构/访问处抛 TypeError 整页白屏——IPC 调用降级为明确失败
//    的 Promise，控制台打印一次醒目错误，且不覆盖已存在的正常注入。
//    曾因误删该配置导致标题栏/托盘菜单整页失效且无任何提示。
if (!window.__TAURI__) {
  console.error(
    '[DSHD] window.__TAURI__ 未注入：请检查 tauri.conf.json 的 app.withGlobalTauri 是否为 true（IPC 将全部失败）',
  );
  window.__TAURI__ = {
    core: {
      invoke: async () => {
        throw new Error('TAURI API not injected (app.withGlobalTauri missing)');
      },
    },
    event: { listen: async () => () => {} },
    window: {
      getCurrentWindow: () => ({
        close: () => {},
        hide: () => {},
        minimize: () => {},
        toggleMaximize: () => {},
        isMaximized: async () => false,
      }),
    },
  };
}

const DSHD_MESSAGES = {
  aboutTagline: ['DeepSeek Harness 桌面外壳 · 开箱即用', 'Desktop shell for DeepSeek Harness, ready out of the box'],
  about: ['关于', 'About'],
  apiKey: ['API Key', 'API Key'],
  apiKeyFormatHint: ['API Key 应以 sk- 开头', 'API key should start with sk-'],
  apiKeyHint: ['留空则之后在 dsh 设置页配置', 'Leave empty to configure later in dsh settings'],
  autostart: ['开机自启动', 'Launch at startup'],
  accountAvailable: ['账户可用', 'Account available'],
  accountStatusAvailable: ['账户状态：可用', 'Account status: Available'],
  accountStatusUnavailable: ['账户状态：不可用', 'Account status: Unavailable'],
  accountUnavailable: ['账户不可用', 'Account unavailable'],
  appMenu: ['应用菜单', 'Application menu'],
  appVersion: ['应用版本 {version}', 'App version {version}'],
  balanceChipHint: ['查看余额详情', 'View balance details'],
  balanceDetailsAria: ['API 余额详情', 'API balance details'],
  balanceQueryFailed: ['余额查询失败', 'Balance query failed'],
  balanceTitle: ['API 余额', 'API balance'],
  checkFailed: ['检查更新失败', 'Update check failed'],
  checkFailedRetry: ['检查失败，请稍后重试', 'The update check failed. Please try again later.'],
  checkingUpdates: ['正在检查更新…', 'Checking for updates…'],
  updatingDsh: ['正在更新…', 'Updating…'],
  checkUpdates: ['检查更新', 'Check for updates'],
  close: ['关闭', 'Close'],
  closeToTray: ['关闭到托盘', 'Close to tray'],
  continue: ['继续', 'Continue'],
  pwshUacNotice: ['安装需要管理员权限，稍后将弹出 UAC 授权提示，请选择“是”。', 'Administrator permission is required. Accept the upcoming UAC prompt.'],
  updateInProgress: ['更新正在进行…', 'Update in progress…'],
  completed: ['已完成', 'Completed'],
  currentVersion: ['当前 {version}', 'Current {version}'],
  dshUpdateAvailable: ['dsh 有新版本：{latest}（当前 {current}）', 'dsh {latest} is available (current: {current})'],
  dshUpToDate: ['dsh 已是最新（{version}）', 'dsh is up to date ({version})'],
  grantedBalance: ['赠送余额', 'Granted balance'],
  install: ['安装', 'Install'],
  installingDsh: ['正在安装 dsh（需要联网）…', 'Installing dsh (internet required)…'],
  installingNode: ['正在准备 Node.js 运行时…', 'Preparing the Node.js runtime…'],
  invalidApiKey: ['API Key 无效', 'Invalid API Key'],
  language: ['语言', 'Language'],
  latestLts: ['最新 LTS {version}', 'Latest LTS {version}'],
  latestVersion: ['最新版', 'latest version'],
  maximize: ['最大化', 'Maximize'],
  mainMenu: ['主菜单', 'Main menu'],
  marketRow: ['插件市场 dsh-market · MIT', 'Plugin market dsh-market · MIT'],
  preferences: ['偏好设置', 'Preferences'],
  menuLoadFailed: ['菜单加载失败', 'Menu failed to load'],
  minimize: ['最小化', 'Minimize'],
  noApiKey: ['未配置 API Key', 'API Key not configured'],
  noBalance: ['暂无余额', 'No balance available'],
  noBalanceInfo: ['暂无余额信息', 'No balance information available'],
  noUpdates: ['未发现可用更新。', 'No updates are available.'],
  notCompleted: ['未完成：{message}', 'Not completed: {message}'],
  notInstalled: ['未安装', 'Not installed'],
  navAbout: ['关于', 'About'],
  navBalance: ['API 余额', 'API balance'],
  navCheck: ['检查更新', 'Updates'],
  navLabel: ['功能导航', 'Feature navigation'],
  navPlugins: ['插件管理', 'Plugins'],
  navSettings: ['桌面端设置', 'Settings'],
  onboardingSub: ['配置 API Key、语言与主题，之后随时可改。', 'Set up API key, language and theme. Change anytime.'],
  onboardingTitle: ['首次使用配置', 'First-run setup'],
  openLogs: ['打开日志', 'Open logs'],
  operationNotStarted: ['未能开始：{message}', 'Could not start: {message}'],
  pluginCatAll: ['全部', 'All'],
  pluginCatMemory: ['记忆/智能', 'Memory/AI'],
  pluginCatNetwork: ['网络/集成', 'Network/Integration'],
  pluginCatSkin: ['皮肤/主题', 'Skins/Themes'],
  pluginCatTool: ['工具', 'Tools'],
  pluginCatWorkflow: ['工作流', 'Workflows'],
  pluginFailed: ['操作失败：{message}', 'Operation failed: {message}'],
  pluginInstall: ['安装', 'Install'],
  pluginInstalled: ['已安装 {name}，服务重启中…', 'Installed {name}. Restarting the service…'],
  pluginInstalledTag: ['已安装', 'Installed'],
  pluginInstalledTitle: ['已安装', 'Installed'],
  pluginInstalling: ['正在安装 {name}…', 'Installing {name}…'],
  pluginNone: ['尚未安装插件。可搜索 npm 安装，或使用内置的 dsh-market 插件市场', 'No plugins installed yet. Search npm to install, or use the built-in dsh-market plugin market.'],
  pluginNoResult: ['没有找到匹配的插件，试试更短的关键词', 'No matching plugins found. Try a shorter keyword.'],
  pluginRemoved: ['已卸载 {name}，服务重启中…', 'Removed {name}. Restarting the service…'],
  pluginRemoving: ['正在卸载 {name}…', 'Removing {name}…'],
  pluginResults: ['搜索结果', 'Search results'],
  pluginSearch: ['搜索', 'Search'],
  pluginSearchDone: ['找到 {count} 个结果', 'Results: {count}'],
  pluginSearchHint: ['搜索 npm 上的 dsh 插件（如 dsh-plugin）…', 'Search dsh plugins on npm (e.g. dsh-plugin)…'],
  pluginSearching: ['正在搜索…', 'Searching…'],
  pluginsTitle: ['插件管理', 'Plugin manager'],
  pluginBuiltin: ['内置', 'Built-in'],
  pluginCheckUpdates: ['检查更新', 'Check for updates'],
  pluginNotInstalled: ['未安装', 'Not installed'],
  pluginCooldown: ['新版本 {version}（发布冷却中）', 'New version {version} (publish cooldown)'],
  pluginUpdatesChecked: ['检查完成，均为最新版本', 'Check complete — all up to date'],
  pluginUpdatesFound: ['发现 {count} 个可更新插件', '{count} update(s) available'],
  pluginUpdateAvailable: ['有新版本', 'Update available'],
  pluginUpdating: ['正在更新…', 'Updating…'],
  pluginUpdated: ['更新完成，服务重启中', 'Updated. Restarting service…'],
  pluginUpdateFailed: ['更新失败：{message}', 'Update failed: {message}'],
  pluginUpdate: ['更新', 'Update'],
  pluginInstall: ['安装', 'Install'],
  pluginInstalling: ['正在安装 {name}…', 'Installing {name}…'],
  pluginInstalled: ['{name} 已安装，服务重启中', '{name} installed. Restarting service…'],
  pluginUninstall: ['卸载', 'Uninstall'],
  port: ['端口 {port}', 'Port {port}'],
  processing: ['处理中…', 'Working…'],
  queryingBalance: ['正在查询余额…', 'Checking balance…'],
  refreshBalance: ['刷新余额', 'Refresh balance'],
  refreshingBalance: ['正在刷新', 'Refreshing'],
  quit: ['退出', 'Quit'],
  ready: ['已就绪，正在进入界面…', 'Ready. Opening the interface…'],
  restore: ['还原', 'Restore'],
  restoreWindow: ['还原窗口', 'Restore window'],
  retry: ['重试', 'Retry'],
  saveFailed: ['保存失败', 'Failed to save'],
  settingsTitle: ['桌面端设置', 'Desktop settings'],
  settingsAutostartDesc: ['开机后静默启动并驻留托盘', 'Start silently in the tray on login'],
  settingsFailed: ['设置失败：{message}', 'Could not apply the setting: {message}'],
  settingsHideStats: ['隐藏会话统计', 'Hide session stats'],
  settingsHideStatsDesc: ['隐藏输入框下方的统计信息；启用窗口状态栏时仍可查看完整统计', 'Hide the stats line below the input; full stats remain available when the window status bar is enabled'],
  settingsHideStatusbar: ['隐藏状态栏', 'Hide the status bar'],
  settingsHideStatusbarDesc: ['隐藏窗口底部的状态栏（会话统计与余额）', 'Hide the status bar at the bottom of the window (session stats and balance)'],
  settingsHideTools: ['隐藏工具调用', 'Hide tool calls'],
  settingsHideToolsDesc: ['隐藏对话中的工具调用卡片，仅保留文本消息与最终输出', 'Hide tool call cards, keeping only text messages and final output'],
  settingsAutoUpdatePlugins: ['自动更新内置插件', 'Auto-update built-in plugins'],
  settingsAutoUpdatePluginsDesc: ['后台每日检查内置插件的新版本并自动升级', 'Check daily for new versions of built-in plugins and install them automatically'],
  statsRegion: ['会话统计', 'Session stats'],
  statsCountsHint: ['本会话的对话轮次与执行步数', 'Conversation turns and execution steps in this session'],
  statsDurationsHint: ['模型推理与工具执行的累计耗时', 'Cumulative time on model inference and tool execution'],
  statsSpeedsHint: ['首次响应的等待时间与生成速率', 'Time to first token and generation rate'],
  statsCacheHint: ['提示词前缀缓存命中率，下方为读/写/未命中拆分', 'Prompt prefix cache hit rate; read/write/uncached split below'],
  statsTokensHint: ['本会话累计输入与输出的 token 数', 'Cumulative input and output tokens for this session'],
  staleBalance: ['刷新失败，显示上次余额', 'Refresh failed; showing the last balance'],
  skip: ['跳过', 'Skip'],
  startUsing: ['开始使用', 'Get started'],
  stepOf: ['第 {n} 步 / 共 {total} 步', 'Step {n} of {total}'],
  tagline: ['本地智能体 · 一键启航', 'Local agent, one-click launch'],
  theme: ['外观', 'Appearance'],
  themeDark: ['深色', 'Dark'],
  themeLight: ['浅色', 'Light'],
  themeSystem: ['跟随系统', 'System'],
  startProgress: ['启动进度', 'Startup progress'],
  starting: ['正在启动…', 'Starting…'],
  startingServer: ['正在启动 dsh 服务…', 'Starting the dsh service…'],
  startupFailed: ['启动失败', 'Startup failed'],
  startupFailedHint: ['请重试；若持续失败，可打开日志查看详情。', 'Please retry. If it keeps failing, open the logs for details.'],
  systemManaged: ['系统 Node.js', 'System Node.js'],
  systemManagedLatest: ['系统 Node.js · 最新 LTS {version}', 'System Node.js · Latest LTS {version}'],
  systemManagedUnavailable: ['系统 Node.js · 暂无法获取版本信息', 'System Node.js · Version information unavailable'],
  toppedUpBalance: ['充值余额', 'Topped-up balance'],
  unknownError: ['未知错误', 'Unknown error'],
  upToDate: ['已是最新', 'Up to date'],
  upToDateLts: ['已是最新 LTS', 'Latest LTS installed'],
  update: ['更新', 'Update'],
  updatedAt: ['更新于 {time}', 'Updated {time}'],
  updateApp: ['更新应用', 'Update app'],
  updateDsh: ['更新 dsh', 'Update dsh'],
  updateDshWait: ['正在更新 dsh，请稍候…', 'Updating dsh. Please wait…'],
  updateFailed: ['更新失败：{message}', 'Update failed: {message}'],
  versionServiceUnavailable: ['暂无法获取版本信息', 'Version information unavailable'],
};

function dshdLocale() {
  return DSHD_LANGUAGE;
}

function dshdT(key, values) {
  const pair = DSHD_MESSAGES[key];
  let text = pair ? pair[DSHD_LANGUAGE === 'zh-CN' ? 0 : 1] : key;
  for (const [name, value] of Object.entries(values || {})) {
    text = text.replaceAll('{' + name + '}', String(value));
  }
  return text;
}

function dshdApplyI18n(root) {
  document.documentElement.lang = DSHD_LANGUAGE;
  const scope = root || document;
  scope.querySelectorAll('[data-i18n]').forEach((el) => {
    el.textContent = dshdT(el.dataset.i18n);
  });
  scope.querySelectorAll('[data-i18n-title]').forEach((el) => {
    el.title = dshdT(el.dataset.i18nTitle);
  });
  scope.querySelectorAll('[data-i18n-aria-label]').forEach((el) => {
    el.setAttribute('aria-label', dshdT(el.dataset.i18nAriaLabel));
  });
}

function dshdSetLanguage(language) {
  DSHD_LANGUAGE = String(language || '').toLowerCase().startsWith('zh') ? 'zh-CN' : 'en';
  window.__DSHD_LANG = DSHD_LANGUAGE;
  dshdApplyI18n();
  window.dispatchEvent(new CustomEvent('dshd-language-changed', {
    detail: { language: DSHD_LANGUAGE },
  }));
}

/** HTML 转义（文本插入 innerHTML 前调用）。 */
function dshdEsc(s) {
  return String(s).replace(/[&<>"']/g, (c) => ({
    '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;',
  }[c]));
}

/** 币种显示符号（CNY → ¥，其余原样）。 */
function dshdCurrency(c) {
  return c === 'CNY' ? '¥' : c;
}

/** 余额字段格式化：空值兜底为 0.00，纯数字加千分位，其他原样。 */
function dshdBalanceValue(v) {
  const s = v != null && v !== '' ? String(v) : '0.00';
  if (!/^\d+(\.\d+)?$/.test(s)) return s;
  const [int, dec] = s.split('.');
  const grouped = int.replace(/\B(?=(\d{3})+(?!\d))/g, ',');
  // 无小数部分补 .00，与空值兜底格式一致（如 0 → 0.00、110 → 110.00）
  return grouped + '.' + (dec !== undefined ? dec : '00');
}
