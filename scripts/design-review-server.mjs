// Local-only visual review of the shipped UI with synthetic data and no media capture.
import http from 'node:http';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { readFile } from 'node:fs/promises';

const directory = path.dirname(fileURLToPath(import.meta.url));
const ui = path.resolve(directory, '../crates/desktop/ui');
const types = { '.html': 'text/html', '.js': 'text/javascript', '.css': 'text/css', '.svg': 'image/svg+xml', '.png': 'image/png', '.woff2': 'font/woff2' };
const server = http.createServer(async (request, response) => {
  try {
    const pathname = decodeURIComponent(new URL(request.url, 'http://localhost').pathname);
    let file;
    if (pathname === '/__preview/adapter.js') file = path.join(directory, 'design-review-adapter.js');
    else {
      file = path.resolve(ui, '.' + (pathname === '/' ? '/index.html' : pathname));
      if (!file.startsWith(ui + path.sep) || !types[path.extname(file)]) {
        response.writeHead(404).end(); return;
      }
    }
    let content = await readFile(file);
    if (file === path.join(ui, 'index.html')) content = Buffer.from(content.toString('utf8').replace('<script src="main.js"></script>', '<script src="/__preview/adapter.js"></script><script src="main.js"></script>'));
    response.writeHead(200, {
      'Content-Type': (types[path.extname(file)] || 'application/octet-stream') + '; charset=utf-8',
      'Cache-Control': 'no-store',
      'Content-Security-Policy': "default-src 'self'; img-src 'self' data: blob:; style-src 'self' 'unsafe-inline'; script-src 'self'; connect-src 'self'; media-src 'self' blob:"
    }).end(content);
  } catch { response.writeHead(404).end(); }
});
server.listen(4177, '127.0.0.1', () => console.log('GhostChat design review: http://127.0.0.1:4177/?view=call-dock&lang=tr'));
