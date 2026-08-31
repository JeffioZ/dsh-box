// 开发模式 UI 静态服务器：端口 4321，服务 ui/ 目录（零依赖，node 内置模块）。
// 供开发版 exe 的加载页/标题栏实时读取 ui 文件：改文件后重启 exe（或刷新页面）即生效。
import http from 'node:http';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const root = path.resolve(__dirname, '..', 'ui');
const port = 4321;

const MIME = {
  '.html': 'text/html; charset=utf-8',
  '.js': 'text/javascript; charset=utf-8',
  '.css': 'text/css; charset=utf-8',
  '.svg': 'image/svg+xml',
  '.png': 'image/png',
  '.ico': 'image/x-icon',
  '.json': 'application/json',
};

const server = http.createServer((req, res) => {
    if (req.method !== 'GET' && req.method !== 'HEAD') {
      res.writeHead(405, { Allow: 'GET, HEAD' });
      res.end('method not allowed');
      return;
    }
    let urlPath;
    try {
      urlPath = decodeURIComponent(new URL(req.url, 'http://localhost').pathname);
    } catch {
      res.writeHead(400);
      res.end('bad request');
      return;
    }
    const relativePath = urlPath === '/' ? 'index.html' : urlPath.replace(/^[/\\]+/, '');
    const file = path.resolve(root, relativePath);
    const relativeToRoot = path.relative(root, file);
    if (relativeToRoot === '..' || relativeToRoot.startsWith('..' + path.sep) || path.isAbsolute(relativeToRoot)) {
      res.writeHead(403, { 'Content-Type': 'text/plain; charset=utf-8' });
      res.end('forbidden');
      return;
    }
    fs.readFile(file, (err, data) => {
      if (err) {
        res.writeHead(404, { 'Content-Type': 'text/plain; charset=utf-8' });
        res.end('not found');
        return;
      }
      res.writeHead(200, {
        'Content-Type': MIME[path.extname(file)] || 'application/octet-stream',
        'Cache-Control': 'no-store',
        'X-Content-Type-Options': 'nosniff',
      });
      res.end(req.method === 'HEAD' ? undefined : data);
    });
  });
server.listen(port, '127.0.0.1', () => {
  console.log(`dev UI server: http://127.0.0.1:${port} (root: ${root})`);
});
server.on('error', (err) => {
  // 端口竞态被占（EADDRINUSE）等启动失败要有明确输出，而不是未捕获异常栈
  console.error(`dev UI server failed to start on port ${port}: ${err.message}`);
  console.error('Port 4321 is fixed by convention (dev-run.ps1 / dev_ui.rs share it); stop the conflicting process and retry.');
  process.exit(1);
});
