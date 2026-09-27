// 生成 DSHBox 品牌图标（方案 D · 蓝—靛蓝，源 SVG 为设计交付件，不描摹、不改形）：
// 应用图标（自带底板/留白/渐变，按尺寸直出）、黑白托盘单色图标、真彩 ICO/ICNS。
// 用法：npm run icons
// 依赖：@resvg/resvg-js（SVG 渲染）；ICO/ICNS/PNG 编解码在 icon-codecs.mjs
import { Resvg } from '@resvg/resvg-js';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { decodePng, encodeIcns, encodeIco, icnsTypes } from './icon-codecs.mjs';

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const root = path.resolve(__dirname, '..');
const iconDir = path.join(root, 'src-tauri', 'icons');

// 品牌源（assets/brand/）。源 SVG 已含底板与留白，坐标空间各归各
// （应用 0 0 512 512，托盘 56 46 400 400）——只按目标尺寸光栅化，
// 不叠加底板、不套用历史鲸鱼标的 translate/scale 参数。
const appSvg = fs.readFileSync(path.join(root, 'assets', 'brand', 'dshbox-app-icon.svg'), 'utf8');
const trayBlackSvg = fs.readFileSync(path.join(root, 'assets', 'brand', 'dshbox-tray-black.svg'), 'utf8');
const trayWhiteSvg = fs.readFileSync(path.join(root, 'assets', 'brand', 'dshbox-tray-white.svg'), 'utf8');

function render(svg, size) {
  return new Resvg(svg, { fitTo: { mode: 'width', value: size } }).render().asPng();
}

// ui/assets/app-icon.svg：给 <img> 引用与跨源文档内联（boot 遮罩/错误页，
// 见 webview/navigation.rs 的 boot_continue_inject）共用的副本。三处加工：
// ① 剥 XML 声明——内联进 innerHTML 时会解析成 bogus comment，虽无害但脏；
// ② 全部 id 加 dshd-ic- 前缀并同步 url(#…) 引用——该 SVG 会被内联进 dsh
//    远程文档，裸 id（background/loop 等通用名）可能与宿主 DOM 冲突使渐变
//    失效，前缀方案为交付包 README「接入方式」所指定，图形不变；
// ③ 内联落点是 JS 单引号字符串（boot-continue.js 的卡片模板），源若含
//    单引号会毁掉整段注入（2026-09-19 教训同类），此处硬校验拦截。
function uiAssetSvg(svg) {
  if (svg.includes("'")) {
    throw new Error('app icon svg 含单引号，会破坏 boot 遮罩的内联注入');
  }
  const ids = [...svg.matchAll(/\bid="([^"]+)"/g)].map((m) => m[1]);
  let out = svg.replace(/^<\?xml[^>]*\?>\s*/, '');
  for (const id of ids) {
    out = out.replaceAll(`id="${id}"`, `id="dshd-ic-${id}"`).replaceAll(`url(#${id})`, `url(#dshd-ic-${id})`);
  }
  // ④ 根 svg 内在尺寸改写为 64：boot 遮罩的 logo 容器是 64px 定宽 div、
  //    内联 SVG 没有 CSS 约束，会按内在尺寸渲染——源图的 512 会让遮罩
  //    logo 暴涨成 512px（2026-09-27 接入方案 D 的现场回归）。<img> 引用
  //    处（启动页/标题栏/关于）均有 CSS 或属性约束，改小无影响；viewBox
  //    保持 512 坐标系，矢量缩放不受内在尺寸影响。
  const sized = out.replace(/(<svg\b[^>]*\bwidth=")512("[^>]*\bheight=")512(")/, '$164$264$3');
  if (sized === out) {
    throw new Error('app icon svg 根节点尺寸改写失败（width/height 非 512，需复核 uiAssetSvg）');
  }
  return sized;
}

fs.mkdirSync(iconDir, { recursive: true });

// 应用图标各尺寸：ICO/ICNS 与仓库 PNG 全部由同一 SVG 按各自目标尺寸独立渲染
const appSizes = [16, 20, 24, 30, 32, 40, 48, 64, 128, 256, 512, 1024];
const appPngs = {};
for (const s of appSizes) appPngs[s] = render(appSvg, s);
for (const s of [32, 40, 48, 64, 128, 256]) {
  fs.writeFileSync(path.join(iconDir, `${s}x${s}.png`), appPngs[s]);
}

// 托盘图标：浅底黑版 / 深底白版（透明底单色，Rust 侧按系统任务栏明暗选用），
// 按物理尺寸精确渲染（100%/125%/150%/200% DPI 各一张）
for (const s of [16, 20, 24, 32]) {
  fs.writeFileSync(path.join(iconDir, `tray-black-${s}.png`), render(trayBlackSvg, s));
  fs.writeFileSync(path.join(iconDir, `tray-white-${s}.png`), render(trayWhiteSvg, s));
  // 旧版单套托盘图（蓝底方块时代）遗留名，清掉避免双份并存被误引用
  fs.rmSync(path.join(iconDir, `tray-${s}.png`), { force: true });
}

// ICO：自编码 32bpp 真彩；帧尺寸为交付包 manifest 的 icoPngFrameSizes
// （较旧版增 30px 档，覆盖 150% DPI 开始菜单）
const icoSizes = [16, 20, 24, 30, 32, 40, 48, 64, 128, 256];
const icoRgba = icoSizes.map((s) => ({ size: s, rgba: decodePng(appPngs[s]).rgba }));
fs.writeFileSync(path.join(iconDir, 'icon.ico'), encodeIco(icoRgba));

// ICNS（macOS .app 打包用）：容器内直接嵌各尺寸 PNG（现代 macOS 全支持）。
// 512/1024 现场渲染不落盘，避免仓库冗余。
const icnsPngs = {};
for (const [, size] of icnsTypes) icnsPngs[size] = appPngs[size];
fs.writeFileSync(path.join(iconDir, 'icon.icns'), encodeIcns(icnsPngs));

// 128@2x（256 的拷贝，供 bundle.icon 引用）
fs.copyFileSync(path.join(iconDir, '256x256.png'), path.join(iconDir, '128x128@2x.png'));

// UI 侧 SVG 副本（启动页/标题栏/关于弹窗 <img> + 遮罩/错误页内联同源）
const uiAssets = path.join(root, 'ui', 'assets');
fs.mkdirSync(uiAssets, { recursive: true });
fs.writeFileSync(path.join(uiAssets, 'app-icon.svg'), uiAssetSvg(appSvg));

console.log('icons generated:', fs.readdirSync(iconDir).join(', '));
