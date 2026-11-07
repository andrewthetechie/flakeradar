// One command to serve the built dashboard (frontend/dist) for the `ui`
// Playwright project. Plain node: no dev server, no /api proxy — the ui tests
// answer every /api/* call with page.route, so nothing here knows the API.
// The dashboard's only routing is query strings, so index.html at / is all a
// navigation needs; every other path 404s exactly as the binary would.
import { createServer } from "node:http";
import { readFile, stat } from "node:fs/promises";
import { extname, join, normalize } from "node:path";

const ROOT = new URL("../../dist", import.meta.url).pathname;
const PORT = Number(process.env.PORT ?? 4173);
const HOST = process.env.HOST ?? "127.0.0.1";

const MIME = {
  ".html": "text/html; charset=utf-8",
  ".js": "text/javascript; charset=utf-8",
  ".mjs": "text/javascript; charset=utf-8",
  ".css": "text/css; charset=utf-8",
  ".json": "application/json; charset=utf-8",
  ".svg": "image/svg+xml",
  ".png": "image/png",
  ".woff": "font/woff",
  ".woff2": "font/woff2",
};

const server = createServer(async (req, res) => {
  try {
    const url = new URL(req.url ?? "/", `http://${HOST}:${PORT}`);
    let path = normalize(decodeURIComponent(url.pathname)).replace(/^\/+/, "");
    if (req.method !== "GET" && req.method !== "HEAD") {
      res.writeHead(405);
      res.end();
      return;
    }
    if (path === "" || path === ".") path = "index.html";
    // Refuse a path that escapes the dist root (issue-less static server).
    const filePath = join(ROOT, path);
    if (!filePath.startsWith(ROOT)) {
      res.writeHead(403);
      res.end();
      return;
    }
    const info = await stat(filePath);
    if (!info.isFile()) {
      res.writeHead(404);
      res.end();
      return;
    }
    const body = await readFile(filePath);
    res.writeHead(200, {
      "Content-Type": MIME[extname(filePath)] ?? "application/octet-stream",
      "Content-Length": body.length,
    });
    res.end(req.method === "HEAD" ? undefined : body);
  } catch {
    if (!res.headersSent) res.writeHead(404);
    res.end();
  }
});

server.listen(PORT, HOST, () => {
  process.stdout.write(`Serving frontend/dist at http://${HOST}:${PORT}\n`);
});
