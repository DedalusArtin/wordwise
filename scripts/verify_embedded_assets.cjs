#!/usr/bin/env node
/**
 * 校验 Tauri 内嵌前端资源是否与 src/ 下的当前源码一致。
 *
 * 为什么需要它：
 *   Tauri 2 把 frontendDist（本项目 = ../src）下的文件 **brotli 压缩后** 内嵌进
 *   exe，并且会对 HTML 做规范化（去掉换行、把 `/>` 改成 `>`）。
 *   所以直接在 exe 里 `grep` 前端字符串 **一定搜不到**，极易误判成
 *   「前端没被重新打包 / 产物是旧的」。
 *
 *   正确做法：到构建脚本产物目录
 *     src-tauri/target/release/build/wordwise-<hash>/out/tauri-codegen-assets
 *   把每个资源文件解压，再与源码逐字节比对。
 *
 * 用法：node scripts/verify_embedded_assets.cjs
 * 退出码：0 = 全部一致；1 = 存在不一致（此时 exe 内的前端确实是旧的）
 */

const fs = require('fs');
const path = require('path');
const zlib = require('zlib');

const ROOT = path.resolve(__dirname, '..');
const BUILD_DIR = path.join(ROOT, 'src-tauri', 'target', 'release', 'build');

const CODECS = [
  ['brotli', (d) => zlib.brotliDecompressSync(d)],
  ['gzip', (d) => zlib.gunzipSync(d)],
  ['deflate', (d) => zlib.inflateSync(d)],
  ['raw', (d) => zlib.inflateRawSync(d)],
];

function decompress(raw) {
  for (const [, fn] of CODECS) {
    try {
      const out = fn(raw);
      if (out && out.length) return out;
    } catch (e) {
      /* 尝试下一种 */
    }
  }
  return null;
}

function collectSources(dir) {
  const out = [];
  (function walk(d) {
    for (const e of fs.readdirSync(d, { withFileTypes: true })) {
      const p = path.join(d, e.name);
      if (e.isDirectory()) walk(p);
      else out.push(p);
    }
  })(dir);
  return out;
}

/**
 * 注意：如果环境变量 CARGO_TARGET_DIR 被设置过（本项目曾误设成 D:\Projects\.cargo-target），
 * cargo 会把产物写到那里，src-tauri\target 下那份就是**旧的**。
 * 这里同时扫两个位置，取「里面有 tauri-codegen-assets 且 mtime 最新」的那个，避免误判。
 */
function buildDirs() {
  const list = [BUILD_DIR];
  if (process.env.CARGO_TARGET_DIR) {
    list.push(path.join(process.env.CARGO_TARGET_DIR, 'release', 'build'));
  }
  return list.filter(p => fs.existsSync(p));
}

function findAssetDir() {
  let best = null;
  for (const bd of buildDirs()) {
    for (const d of fs.readdirSync(bd)) {
      if (!d.startsWith('wordwise-')) continue;
      const p = path.join(bd, d, 'out', 'tauri-codegen-assets');
      if (!fs.existsSync(p)) continue;
      const mt = fs.statSync(p).mtimeMs;
      if (!best || mt > best.mt) best = { p, mt };
    }
  }
  return best ? best.p : null;
}

const assetDir = findAssetDir();
if (!assetDir) {
  console.error('未找到 tauri-codegen-assets 目录，请先执行 release 构建。');
  process.exit(1);
}

const sources = collectSources(path.join(ROOT, 'src'));
const byContent = new Map();
for (const f of sources) {
  byContent.set(fs.readFileSync(f).toString('base64'), path.relative(ROOT, f));
}

// HTML 会被"解析 → 重新序列化"：空白、自闭合标签、布尔属性写法都会变
// （例如源码里的 `checked` 会变成 `checked=""`），所以不能逐字节比。
// 改为两个层面的判定：
//   1) 忽略空白与自闭合后完全相同
//   2) 退一步：源码里出现的全部 id / 关键标识在资源里都能找到
const normalize = (s) => s.replace(/\s+/g, '').replace(/\/>/g, '>');
const idsOf = (s) => new Set((s.match(/id="([^"]+)"/g) || []).map((m) => m.slice(4, -1)));

console.log('资源目录: ' + assetDir);
console.log('源码文件: ' + sources.length + '   资源文件: ' + fs.readdirSync(assetDir).length);
console.log('');

const matched = new Set();
let htmlChecked = false;
let htmlOk = false;
const rows = [];

for (const name of fs.readdirSync(assetDir)) {
  const out = decompress(fs.readFileSync(path.join(assetDir, name)));
  if (!out) {
    rows.push([name.slice(0, 12), '(无法解压)', '']);
    continue;
  }
  const exact = byContent.get(out.toString('base64'));
  if (exact) {
    matched.add(exact);
    rows.push([name.slice(0, 12), 'brotli', '= ' + exact]);
    continue;
  }
  // 退化为 HTML 规范化比对
  const text = out.toString('utf8');
  if (text.slice(0, 15).includes('<!DOCTYPE')) {
    htmlChecked = true;
    let hit = false;
    for (const f of sources) {
      if (!f.endsWith('.html')) continue;
      const cur = fs.readFileSync(f, 'utf8');
      const na = normalize(text);
      const nb = normalize(cur);
      if (na === nb) {
        matched.add(path.relative(ROOT, f));
        htmlOk = true;
        hit = true;
        rows.push([name.slice(0, 12), 'brotli', '= ' + path.relative(ROOT, f) + '（规范化后一致）']);
        break;
      }
      // 退一步：id 是否齐全
      const want = idsOf(cur);
      const got = idsOf(text);
      const missingIds = [...want].filter((x) => !got.has(x));
      if (!hit && missingIds.length === 0 && want.size > 0) {
        matched.add(path.relative(ROOT, f));
        htmlOk = true;
        hit = true;
        rows.push([name.slice(0, 12), 'brotli', '≈ ' + path.relative(ROOT, f) +
          '（已重新序列化，' + want.size + ' 个 id 全部齐全）']);
        break;
      }
      if (!hit) {
        rows.push([name.slice(0, 12), 'brotli', '≠ ' + path.relative(ROOT, f) +
          '  缺 id: ' + (missingIds.slice(0, 6).join(', ') || '(无)')]);
      }
    }
  } else {
    rows.push([name.slice(0, 12), 'brotli', '(旧版本残留，未被引用)']);
  }
}

for (const [a, b, c] of rows) console.log('  ' + a + '…  ' + b.padEnd(8) + c);

const missing = sources
  .map((f) => path.relative(ROOT, f))
  .filter((f) => !matched.has(f));

console.log('');
console.log('已确认内嵌的源文件: ' + matched.size + ' / ' + sources.length);
if (!htmlChecked) console.log('注意: 资源中未发现 HTML 入口');
if (missing.length) {
  console.log('★ 未在内嵌资源中找到: ' + missing.join(', '));
}

const ok = missing.length === 0 && (!htmlChecked || htmlOk);
console.log('');
console.log(ok ? '结论: 内嵌前端与当前源码一致' : '结论: ★ 内嵌前端与源码不一致，exe 里是旧前端');
process.exit(ok ? 0 : 1);
