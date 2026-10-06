#!/usr/bin/env node
/**
 * i18n 覆盖率检查。
 *
 * 为什么需要它：界面语言是按「中文原文做字典键」迁移的，漏一条不会报错、
 * 只会安静地继续显示中文。肉眼扫一遍 ~29k 字根本不现实，所以要机器列出
 * 「界面上会出现、但字典里没有」的串，把迁移变成可以逐条清空的清单。
 *
 * 抽取范围：
 *   - index.html：标签之间的文本、title / placeholder / aria-label 属性
 *   - src/js/*.js：模板字符串与引号字符串里的字面中文串
 * （实际字符串再多也有噪声，所以 key 提取走「保守去重」：只关心去 pkg 后的
 *   独立字符串，不求 100% 精确，求不漏。）
 *
 * 用法：node scripts/check_i18n.cjs [--all]
 *   --all  连「已翻译」的也一并列出（默认只列缺失项）
 */
const fs = require('fs');
const path = require('path');

const ROOT = path.resolve(__dirname, '..');

/** 字符串里有汉字才算待翻译文本。 */
const HAS_HAN = /[\u3400-\u4dbf\u4e00-\u9fff\uf900-\ufaff]/;

/** 把 src/js/i18n.js 里的 EN 表抠出来（不加载脚本，避免依赖 DOM）。 */
function loadDict() {
  const src = fs.readFileSync(path.join(ROOT, 'src/js/i18n.js'), 'utf8');
  const start = src.indexOf('const EN = {');
  if (start < 0) throw new Error('没找到 EN 表');
  const open = src.indexOf('{', start);
  let depth = 0, end = -1;
  for (let i = open; i < src.length; i += 1) {
    const c = src[i];
    if (c === '{') depth += 1;
    else if (c === '}') { depth -= 1; if (depth === 0) { end = i; break; } }
  }
  const body = src.slice(open + 1, end);
  // ★ 直接求值对象字面量，而不是逐行正则：一行里写多条（`'天': 'd', '对': '✓'`）
  //   是常见写法，逐行匹配会把后面的条目全漏掉，覆盖率就永远是错的。
  const dict = new Function('return {' + body + '}')();

  // 重复键保护：JS 对象里同一个 key 写两次不会报错，后者静默覆盖前者，
  // 于是「明明写了却没生效」，而且本脚本还统计不出来。这里显式查一遍。
  const keys = Object.keys(dict);
  const seenCount = new Map();
  for (const m of body.matchAll(/(?:^|[,{\n])\s*(['"])(.+?)\1\s*:/g)) {
    const k = m[2];
    seenCount.set(k, (seenCount.get(k) || 0) + 1);
  }
  const dups = [...seenCount.entries()].filter(([, n]) => n > 1);
  if (dups.length) {
    console.error('× EN 表里有重复键（后者会覆盖前者）：');
    for (const [k, n] of dups) console.error('   ' + k + ' × ' + n);
    process.exit(1);
  }
  if (keys.length !== seenCount.size) {
    console.error('× 键数与字面量数不一致，字典可能没从 EN 表正确切出');
    process.exit(1);
  }
  return dict;
}

/** 从 JS 源码里抽中文字符串字面量。 */
function extractFromJs(file, out) {
  const src = fs.readFileSync(file, 'utf8');
  const re = /(['"`])((?:[^\\`'"]|\\.)*?)\1/g;
  let m;
  while ((m = re.exec(src)) !== null) {
    const s = m[2];
    if (!HAS_HAN.test(s)) continue;
    if (s.length > 80) continue;      // 超长多半是整页 HTML，交给 index.html 那份去重
    const trimmed = s.trim();
    if (!trimmed) continue;
    out.add(trimmed);
  }
}

/** 从 index.html 抽标签间文本与可翻译属性。 */
function extractFromHtml(file, out) {
  const src = fs.readFileSync(file, 'utf8');
  const no = src.replace(/<!--[\s\S]*?-->/g, ' ').replace(/<script[\s\S]*?<\/script>/g, ' ')
                .replace(/<style[\s\S]*?<\/style>/g, ' ');

  // 可翻译属性
  for (const attr of ['title', 'placeholder', 'aria-label']) {
    const re = new RegExp(attr + '="([^"]*)"', 'g');
    let m;
    while ((m = re.exec(no)) !== null) {
      const v = m[1].trim();
      if (HAS_HAN.test(v)) out.add(v);
    }
  }

  // 标签间的可见文本（剥掉内联标签后取）
  const textRe = />([^<>]*)</g;
  let m;
  while ((m = textRe.exec(no)) !== null) {
    const v = m[1].replace(/\s+/g, ' ').trim();
    if (v && HAS_HAN.test(v)) out.add(v);
  }
}

const all = process.argv.includes('--all');
const dict = loadDict();
const found = new Set();

const files = [path.join(ROOT, 'src/index.html')];
const jsDir = path.join(ROOT, 'src/js');
for (const f of fs.readdirSync(jsDir).sort()) {
  if (!f.endsWith('.js') || f === 'i18n.js') continue;
  files.push(path.join(jsDir, f));
}

for (const f of files) {
  if (f.endsWith('.html')) extractFromHtml(f, found);
  else extractFromJs(f, found);
}

const missing = [];
let done = 0;
for (const s of found) {
  if (Object.prototype.hasOwnProperty.call(dict, s)) done += 1;
  else missing.push(s);
}
missing.sort((a, b) => a.length - b.length);

const total = found.size;
const pct = total ? ((done / total) * 100).toFixed(1) : '0.0';
console.log(`抽到界面字符串 ${total} 条，已翻译 ${done} 条，覆盖率 ${pct}%`);
console.log(`未覆盖 ${missing.length} 条${all ? '（含已翻译的完整清单见上方统计）' : '，按长度升序：'}\n`);
if (!all) {
  for (const s of missing.slice(0, 400)) console.log('  ' + JSON.stringify(s));
  if (missing.length > 400) console.log(`  …… 另有 ${missing.length - 400} 条`);
}
