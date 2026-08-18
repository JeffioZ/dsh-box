// 把 release 产物复制到 dist/ 并报告体积
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const root = path.resolve(__dirname, '..');
const dist = path.join(root, 'dist');
fs.mkdirSync(dist, { recursive: true });

const src = path.join(root, 'src-tauri', 'target', 'release', 'DSHBox.exe');
if (!fs.existsSync(src)) throw new Error('未找到编译产物: ' + src);

const dst = path.join(dist, 'DSHBox.exe');
fs.copyFileSync(src, dst);
const size = fs.statSync(dst).size;
console.log(`dist/DSHBox.exe (${(size / 1024 / 1024).toFixed(2)} MB)`);
