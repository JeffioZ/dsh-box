// 生成 DeepSeek 品牌图标：应用图标（蓝底白鲸圆角方块）、窗口/托盘图标、真彩 ICO。
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

// 官方 DeepSeek 鲸鱼标志源文件
const brandSvg = fs.readFileSync(path.join(root, 'assets', 'brand', 'deepseek-mark.svg'), 'utf8');
// 注意：只正则提取第一个 <path> 的 d 属性——品牌源当前是单 path；
// 若未来改为多 path 图形，这里会取错路径，必须重写提取逻辑
const m = brandSvg.match(/<path[^>]*d="([^"]+)"[^>]*>/);
if (!m) throw new Error('brand mark path not found');
const logoPath = m[1];

// 深蓝底 + 白色 logo（应用图标）。
// 鲸鱼占比随尺寸自适应：小图标放大鲸鱼保证可辨识度，大图标保持官方比例。
function appIconSvg(size) {
  const ratio = size <= 24 ? 0.74 : size <= 48 ? 0.7 : 0.62;
  const logoSize = size * ratio;
  const ox = (size - logoSize) / 2;
  const oy = ox;
  return `<svg xmlns="http://www.w3.org/2000/svg" width="${size}" height="${size}" viewBox="0 0 ${size} ${size}">
  <rect x="0" y="0" width="${size}" height="${size}" rx="${size * 0.22}" fill="#4D6BFE"/>
  <g transform="translate(${ox} ${oy}) scale(${logoSize / 50})">
    <path d="${logoPath}" fill="#FFFFFF"/>
  </g>
</svg>`;
}

function render(svg, size) {
  const resvg = new Resvg(svg, { fitTo: { mode: 'width', value: size } });
  return resvg.render().asPng();
}

fs.mkdirSync(iconDir, { recursive: true });

// 1) 各尺寸应用图标（蓝底白鲸）
const appSizes = [16, 20, 24, 32, 40, 48, 64, 128, 256];
const appFileSizes = [32, 40, 48, 64, 128, 256];
const appPngs = {};
for (const s of appSizes) appPngs[s] = render(appIconSvg(s), s);
for (const s of appFileSizes) fs.writeFileSync(path.join(iconDir, `${s}x${s}.png`), appPngs[s]);

// 2) 托盘专用图标：与应用图标同风格（蓝底圆角方块+白鲸），
//    按物理尺寸精确渲染（100%/125%/150%/200% DPI 各一张）
const traySizes = [16, 20, 24, 32];
for (const s of traySizes) {
  fs.writeFileSync(path.join(iconDir, `tray-${s}.png`), render(appIconSvg(s), s));
}

// 3) ICO：自编码 32bpp 真彩，覆盖标题栏/任务栏/资源管理器常用尺寸
const icoSizes = [16, 20, 24, 32, 40, 48, 64, 128, 256];
const icoRgba = icoSizes.map((s) => ({ size: s, rgba: decodePng(appPngs[s]).rgba }));
fs.writeFileSync(path.join(iconDir, 'icon.ico'), encodeIco(icoRgba));

// 4) ICNS（macOS .app 打包用）：容器内直接嵌各尺寸 PNG（现代 macOS 全支持）。
//    512/1024 现场渲染不落盘，避免仓库冗余。
const icnsPngs = {};
for (const [, size] of icnsTypes) {
  icnsPngs[size] = appPngs[size] ?? render(appIconSvg(size), size);
}
fs.writeFileSync(path.join(iconDir, 'icon.icns'), encodeIcns(icnsPngs));

// 5) 128@2x（256 的拷贝，供 bundle.icon 引用）
fs.copyFileSync(path.join(iconDir, '256x256.png'), path.join(iconDir, '128x128@2x.png'));

// 6) 标题栏品牌图标（内联 SVG，任意 DPI 缩放清晰）
const uiAssets = path.join(root, 'ui', 'assets');
fs.mkdirSync(uiAssets, { recursive: true });
fs.writeFileSync(path.join(uiAssets, 'app-icon.svg'), appIconSvg(64));

console.log('icons generated:', fs.readdirSync(iconDir).join(', '));
