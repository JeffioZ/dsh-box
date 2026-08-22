import fs from 'node:fs';
import path from 'node:path';
import vm from 'node:vm';
import { execFileSync, spawnSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const failures = [];
const fail = (message) => failures.push(message);
const read = (file) => fs.readFileSync(path.join(root, file), 'utf8');
const tracked = execFileSync(
  'git',
  ['ls-files', '--cached', '--others', '--exclude-standard'],
  { cwd: root, encoding: 'utf8' },
)
  .split(/\r?\n/)
  .filter((file) => file && fs.existsSync(path.join(root, file)));

const knownText = new Set([
  '', '.css', '.html', '.js', '.json', '.lock', '.md', '.mjs', '.ps1', '.rs',
  '.svg', '.toml', '.xml', '.yaml', '.yml',
]);
const knownBinary = new Set(['.icns', '.ico', '.png']);
const strictUtf8 = new TextDecoder('utf-8', { fatal: true });

for (const file of tracked) {
  const ext = path.extname(file).toLowerCase();
  if (!knownText.has(ext) && !knownBinary.has(ext)) fail(`未纳入项目检查的文件类型: ${file}`);
  const stat = fs.statSync(path.join(root, file));
  if (stat.size === 0) fail(`空文件: ${file}`);
  if (knownText.has(ext)) {
    const bytes = fs.readFileSync(path.join(root, file));
    let text = '';
    try {
      text = strictUtf8.decode(bytes);
    } catch {
      fail(`${file}: 不是有效的 UTF-8 文本`);
    }
    if (text.includes('\0')) fail(`${file}: 文本中包含 NUL 字节`);
  }
}

const normalizedNames = new Set();
for (const file of tracked) {
  const normalized = file.replaceAll('\\', '/').normalize('NFC').toLowerCase();
  if (normalizedNames.has(normalized)) fail(`文件名在大小写不敏感系统上冲突: ${file}`);
  normalizedNames.add(normalized);
}

for (const file of tracked.filter((file) => path.extname(file) === '.json')) {
  try {
    JSON.parse(read(file));
  } catch (error) {
    fail(`${file}: JSON 解析失败: ${error.message}`);
  }
}

for (const file of tracked.filter((file) => /\.(?:js|mjs)$/.test(file))) {
  const result = spawnSync(process.execPath, ['--check', file], { cwd: root, encoding: 'utf8' });
  if (result.status !== 0) fail(`${file}: JavaScript 语法失败\n${result.stderr.trim()}`);
}

for (const file of tracked.filter((file) => file.endsWith('.html'))) {
  const html = read(file);
  const withoutScripts = html
    .replace(/<script\b[\s\S]*?<\/script>/gi, '')
    .replace(/<style\b[\s\S]*?<\/style>/gi, '');
  const ids = [...withoutScripts.matchAll(/\bid=["']([^"']+)["']/g)].map((match) => match[1]);
  for (const id of new Set(ids)) {
    if (ids.filter((value) => value === id).length > 1) fail(`${file}: 重复静态 id=${id}`);
  }
  for (const match of html.matchAll(/<script(?![^>]*\bsrc=)[^>]*>([\s\S]*?)<\/script>/gi)) {
    try {
      new vm.Script(match[1], { filename: `${file}:inline-script` });
    } catch (error) {
      fail(`${file}: 内联 JavaScript 语法失败: ${error.message}`);
    }
  }
  for (const match of html.matchAll(/\b(?:src|href)=["']([^"']+)["']/gi)) {
    const target = match[1];
    if (!target || /^(?:#|[a-z]+:|\/\/)/i.test(target)) continue;
    const local = path.resolve(root, path.dirname(file), decodeURIComponent(target.split(/[?#]/)[0]));
    if (!fs.existsSync(local)) fail(`${file}: 本地引用不存在: ${target}`);
  }
}

const i18n = read('ui/i18n.js');
const messageMatch = i18n.match(/const DSHD_MESSAGES = (\{[\s\S]*?\n\});/);
if (!messageMatch) {
  fail('ui/i18n.js: 找不到 DSHD_MESSAGES');
} else {
  const keyLines = [...messageMatch[1].matchAll(/^\s{2}([A-Za-z_$][\w$]*):/gm)].map((match) => match[1]);
  for (const key of new Set(keyLines)) {
    if (keyLines.filter((value) => value === key).length > 1) fail(`ui/i18n.js: 重复 i18n key: ${key}`);
  }
  try {
    const messages = vm.runInNewContext(`(${messageMatch[1]})`);
    for (const [key, pair] of Object.entries(messages)) {
      if (!Array.isArray(pair) || pair.length !== 2 || pair.some((value) => typeof value !== 'string' || !value)) {
        fail(`ui/i18n.js: ${key} 必须包含非空中英双语`);
      }
    }
    const usages = new Set();
    for (const file of tracked.filter((file) => file.startsWith('ui/') && /\.(?:html|js)$/.test(file))) {
      const text = read(file);
      for (const match of text.matchAll(/\bdata-i18n(?:-placeholder|-title|-aria-label)?=["']([^"']+)["']/g)) usages.add(match[1]);
      for (const match of text.matchAll(/\bdshdT\(\s*["']([^"']+)["']/g)) usages.add(match[1]);
    }
    for (const key of usages) {
      if (!(key in messages)) fail(`UI 使用了未定义的 i18n key: ${key}`);
    }
  } catch (error) {
    fail(`ui/i18n.js: DSHD_MESSAGES 无法求值: ${error.message}`);
  }
}

for (const file of tracked.filter((file) => file.startsWith('ui/') && file.endsWith('.html'))) {
  const html = read(file);
  if (html.includes('common.js')) {
    const i18nAt = html.indexOf('i18n.js');
    const commonAt = html.indexOf('common.js');
    if (i18nAt < 0 || i18nAt > commonAt) fail(`${file}: i18n.js 必须在 common.js 之前加载`);
  }
}
for (const file of ['ui/index.html', 'ui/control-center.html']) {
  const html = read(file);
  if (/<(?:style|script)(?![^>]*\bsrc=)[^>]*>[\s\S]*?<\/(?:style|script)>/i.test(html)) {
    fail(`${file}: 不应保留内联 CSS/JavaScript`);
  }
}
const modelUi = read('ui/control-center-settings.js');
const renderStart = modelUi.indexOf('function miRenderResult(preview)');
const applyAction = modelUi.indexOf("applyBtn.textContent = dshdT('modelImportApply')", renderStart);
const renderEnd = modelUi.indexOf('box.hidden = false', renderStart);
if (renderStart < 0 || applyAction < renderStart || renderEnd < applyAction) {
  fail('模型导入预览必须在有无 API Key 两种情况下都渲染应用按钮');
}
try {
  new vm.Script([
    modelUi,
    read('ui/control-center-plugins.js'),
    read('ui/control-center.js'),
  ].join('\n'), { filename: 'control-center:combined-classic-scripts' });
} catch (error) {
  fail(`控制中心分拆脚本存在跨文件声明/语法冲突: ${error.message}`);
}

const sharedMenu = read('ui/menu.js');
try {
  const context = {};
  vm.runInNewContext(sharedMenu, context, { filename: 'ui/menu.js' });
  const normalized = context.dshdNormalizeMenuItems([
    { sep: true },
    { id: 'first' },
    { sep: true },
    { sep: true },
    { id: 'last' },
    { sep: true },
  ]);
  if (JSON.stringify(normalized) !== JSON.stringify([
    { id: 'first' },
    { sep: true },
    { id: 'last' },
  ])) {
    fail('ui/menu.js: 菜单分隔线只能保留在两个相邻条目组之间');
  }
} catch (error) {
  fail(`ui/menu.js: 共享菜单契约无法执行: ${error.message}`);
}

const statusbar = read('ui/statusbar.js');
if (statusbar.includes('.errorKind')) fail('ui/statusbar.js: BalancePayload 契约字段必须使用 error_kind');
const authBranch = statusbar.indexOf("b.error_kind === 'no_key'");
const genericFailure = statusbar.indexOf('if (!b.ok)', authBranch);
if (authBranch < 0 || genericFailure < authBranch) {
  fail('ui/statusbar.js: 凭据错误必须在通用余额失败分支之前处理');
}
if (!statusbar.includes('!payload.error_kind && lastBalance && lastBalance.ok')) {
  fail('ui/statusbar.js: 只有瞬时错误可保留 stale 余额，凭据错误必须立即替换');
}
const settingsCommands = read('src-tauri/src/commands/settings.rs');
const commandRegistry = read('src-tauri/src/commands/mod.rs');
if (!modelUi.includes("invoke('set_deepseek_api_key'")
    || !settingsCommands.includes('pub fn set_deepseek_api_key(')
    || !commandRegistry.includes('settings::set_deepseek_api_key,')) {
  fail('DeepSeek API Key 设置入口的前后端命令契约不完整');
}

const menuScript = read('src-tauri/resources/injections/context-menu.js');
const navigation = read('src-tauri/src/webview/navigation.rs');
if (!navigation.includes('include_str!("../../resources/injections/context-menu.js")')) {
  fail('src-tauri/src/webview/navigation.rs: 右键菜单资源未通过 include_str! 嵌入');
} else {
  try {
    new vm.Script(`(function(){${menuScript}\n})();`, { filename: 'context-menu.js' });
  } catch (error) {
    fail(`MENU_INJECT JavaScript 语法失败: ${error.message}`);
  }
  for (const contract of [
    "T('打开文件', 'Open file')",
    "T('复制路径', 'Copy path')",
    "T('复制文件内容', 'Copy file contents')",
    'contextSequence',
    "dshdUrl('normalize'",
    'prefers-reduced-motion',
    "document.addEventListener('contextmenu', onCtx)",
  ]) {
    if (!menuScript.includes(contract)) fail(`MENU_INJECT 缺少兼容契约: ${contract}`);
  }
  if (!navigation.includes('delete window.__dshdProtocolToken')) fail('注入后未清除全局协议令牌');
}

const pkg = JSON.parse(read('package.json'));
const tauri = JSON.parse(read('src-tauri/tauri.conf.json'));
const cargo = read('src-tauri/Cargo.toml').match(/^version\s*=\s*"([^"]+)"/m)?.[1];
const lockVersion = read('src-tauri/Cargo.lock').match(/\[\[package\]\]\s*\nname = "dsh-box"\s*\nversion = "([^"]+)"/m)?.[1];
if (!cargo || pkg.version !== cargo || tauri.version !== cargo || lockVersion !== cargo) {
  fail(`版本不一致: Cargo=${cargo} package=${pkg.version} tauri=${tauri.version} lock=${lockVersion}`);
}

const presets = JSON.parse(read('src-tauri/resources/builtin-plugins.json'));
const presetIds = new Set();
for (const preset of presets) {
  if (!preset.id || !preset.spec || !preset.name || !preset.description || !preset.repoUrl) {
    fail(`builtin-plugins.json: 条目字段不完整: ${JSON.stringify(preset)}`);
  }
  if (presetIds.has(preset.id)) fail(`builtin-plugins.json: 重复 id=${preset.id}`);
  presetIds.add(preset.id);
  if (!/^https:\/\/github\.com\//.test(preset.repoUrl || '')) fail(`builtin-plugins.json: 非 GitHub HTTPS 地址: ${preset.repoUrl}`);
}

for (const file of tracked.filter((file) => file.endsWith('.ps1'))) {
  const bytes = fs.readFileSync(path.join(root, file));
  if (!(bytes[0] === 0xef && bytes[1] === 0xbb && bytes[2] === 0xbf)) fail(`${file}: 缺少 UTF-8 BOM`);
}

for (const file of tracked.filter((file) => file.endsWith('.md'))) {
  const text = read(file);
  for (const match of text.matchAll(/!?\[[^\]]*\]\(([^)]+)\)/g)) {
    let target = match[1].trim().replace(/^<|>$/g, '');
    if (!target || /^(?:#|[a-z]+:|\/\/)/i.test(target)) continue;
    target = decodeURIComponent(target.split('#')[0]);
    if (!fs.existsSync(path.resolve(root, path.dirname(file), target))) fail(`${file}: 本地链接不存在: ${target}`);
  }
}

const pngExpectations = new Map([
  ['src-tauri/icons/32x32.png', [32, 32]],
  ['src-tauri/icons/40x40.png', [40, 40]],
  ['src-tauri/icons/48x48.png', [48, 48]],
  ['src-tauri/icons/64x64.png', [64, 64]],
  ['src-tauri/icons/128x128.png', [128, 128]],
  ['src-tauri/icons/128x128@2x.png', [256, 256]],
  ['src-tauri/icons/256x256.png', [256, 256]],
  ['src-tauri/icons/tray-16.png', [16, 16]],
  ['src-tauri/icons/tray-20.png', [20, 20]],
  ['src-tauri/icons/tray-24.png', [24, 24]],
  ['src-tauri/icons/tray-32.png', [32, 32]],
]);
for (const [file, [expectedWidth, expectedHeight]] of pngExpectations) {
  const bytes = fs.readFileSync(path.join(root, file));
  const signature = bytes.subarray(0, 8).toString('hex');
  const width = bytes.readUInt32BE(16);
  const height = bytes.readUInt32BE(20);
  if (signature !== '89504e470d0a1a0a' || width !== expectedWidth || height !== expectedHeight) {
    fail(`${file}: PNG 规格异常 (${width}x${height})`);
  }
}
if (!read('scripts/gen-icons.mjs').includes("assets', 'brand', 'deepseek-mark.svg")) {
  fail('图标生成脚本未使用统一品牌源 assets/brand/deepseek-mark.svg');
}
const ico = fs.readFileSync(path.join(root, 'src-tauri/icons/icon.ico'));
if (ico.subarray(0, 4).toString('hex') !== '00000100') fail('src-tauri/icons/icon.ico: ICO 文件头异常');
const icns = fs.readFileSync(path.join(root, 'src-tauri/icons/icon.icns'));
if (icns.subarray(0, 4).toString('ascii') !== 'icns') fail('src-tauri/icons/icon.icns: ICNS 文件头异常');

if (failures.length) {
  console.error(`项目检查失败（${failures.length} 项）：`);
  for (const failure of failures) console.error(`- ${failure}`);
  process.exit(1);
}

console.log(`项目检查通过：${tracked.length} 个受控文件，JS/HTML/i18n/版本/配置/文档链接/图标/右键契约均有效。`);
