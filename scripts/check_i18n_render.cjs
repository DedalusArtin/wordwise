#!/usr/bin/env node
/**
 * i18n「渲染后文本」检查 —— 补 `check_i18n.cjs` 够不到的两件事。
 *
 * ## 为什么需要第二个 i18n 脚本
 *
 * `check_i18n.cjs` 回答的是「源码里出现了哪些中文字面量、字典里有没有」。
 * 但界面语言实际是**按文本节点**翻译的，于是它两种错都漏：
 *
 *   1. **死键**：字典里写了整段 HTML（`共 <b>${n}</b> 个词`）——
 *      整段 HTML 从来不会成为文本节点，这条键永远命不中，
 *      可它照样被算作「已翻译」。覆盖率因此虚高。
 *   2. **没翻**：模板串被标签拆散后，真正要建的是碎片键
 *      （`共` / `个词 /` / `条关系`），而这些碎片在源码里根本不以
 *      字面量形式出现，字面量扫描看不见它们。
 *
 * 所以这里直接跑翻译：加载 i18n.js、切成英文、把**渲染后真正会出现的
 * 文本**一条条喂进去。附带一条硬约束：严格用例的译文里不许剩汉字 ——
 * 一次抓出所有「翻了一半」。
 *
 * ## 两种模式
 *
 *   node scripts/check_i18n_render.cjs
 *       死键检查 + 92 条渲染用例。退出码即结论（构建闸门用这个）。
 *
 *   node scripts/check_i18n_render.cjs --gap [文件…]
 *       按**文件**列出「界面上会出现、字典里没有」的串。排查新功能漏翻用，
 *       不参与退出码（结果里混着示例数据等噪声，需要人看）。
 */
const fs = require('fs');
const path = require('path');
const vm = require('vm');

const ROOT = path.resolve(__dirname, '..');
const HAS_HAN = /[\u3400-\u4dbf\u4e00-\u9fff\uf900-\ufaff]/;

function loadDict() {
  const src = fs.readFileSync(path.join(ROOT, 'src/js/i18n.js'), 'utf8');
  const start = src.indexOf('const EN = {');
  const open = src.indexOf('{', start);
  let depth = 0, end = -1;
  for (let i = open; i < src.length; i += 1) {
    const c = src[i];
    if (c === '{') depth += 1;
    else if (c === '}') { depth -= 1; if (!depth) { end = i; break; } }
  }
  return new Function('return {' + src.slice(open + 1, end) + '}')();
}

/* ============================================================
   模式二：按文件列出缺失文案
   ============================================================ */

function extractJs(file) {
  const src = fs.readFileSync(file, 'utf8');
  const out = new Set();
  const re = /'((?:[^'\\]|\\.)*)'|"((?:[^"\\]|\\.)*)"|`((?:[^`\\]|\\.)*)`/g;
  let m;
  while ((m = re.exec(src)) !== null) {
    const v = (m[1] ?? m[2] ?? m[3] ?? '').replace(/\s+/g, ' ').trim();
    if (v && HAS_HAN.test(v)) out.add(v);
  }
  return out;
}

function extractHtml(file) {
  const src = fs.readFileSync(file, 'utf8');
  const no = src.replace(/<!--[\s\S]*?-->/g, '');
  const out = new Set();
  for (const m of no.matchAll(/>([^<>]*)</g)) {
    const v = m[1].replace(/\s+/g, ' ').trim();
    if (v && HAS_HAN.test(v)) out.add(v);
  }
  for (const m of no.matchAll(/(?:title|placeholder|aria-label)="([^"]*)"/g)) {
    const v = m[1].replace(/\s+/g, ' ').trim();
    if (v && HAS_HAN.test(v)) out.add(v);
  }
  return out;
}

function gapMode(files) {
  const dict = loadDict();
  const list = files.length ? files : [
    'src/index.html', 'src/js/mini.js', 'src/js/graph.js', 'src/js/maint.js',
    'src/js/settings.js', 'src/js/study.js', 'src/js/lookup.js',
    'src/js/translate.js', 'src/js/library.js', 'src/js/update.js',
  ];
  let grand = 0;
  for (const rel of list) {
    const f = path.join(ROOT, rel);
    if (!fs.existsSync(f)) continue;
    const found = rel.endsWith('.html') ? extractHtml(f) : extractJs(f);
    const miss = [...found].filter((s) => !Object.prototype.hasOwnProperty.call(dict, s));
    miss.sort((a, b) => a.length - b.length);
    grand += miss.length;
    console.log(`\n===== ${rel}  共 ${found.size} 条，缺 ${miss.length} 条 =====`);
    for (const s of miss) console.log('  ' + JSON.stringify(s));
  }
  console.log(`\n合计缺 ${grand} 条`);
  console.log('（提示：这里混着示例数据 / 调试词库等噪声，需要人看；闸门不含本模式）');
}

if (process.argv.includes('--gap')) {
  gapMode(process.argv.slice(2).filter((a) => !a.startsWith('--')));
  process.exit(0);
}

/* ============================================================
   模式一：死键 + 真翻译（构建闸门）
   ============================================================ */

const escapeRe = (s) => s.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');

function regexFor(key) {
  const parts = key.split('{n}');
  return new RegExp('^' + parts.map(escapeRe).join('(.*?)') + '$');
}

let failed = 0;

// ---- 1. 死键：模板键的正则必须能匹配它自己的样本 ----
{
  const dict = loadDict();
  const bad = [];
  for (const k of Object.keys(dict)) {
    const re = regexFor(k);
    const samples = [
      k.replace(/\{n\}/g, '12'),
      k.replace(/\s+/g, ' ').replace(/\{n\}/g, '12'), // 渲染时会折叠内部空白
      k.replace(/\{n\}/g, '128'),
    ];
    if (samples.some((s) => !re.test(s))) bad.push(k);
  }
  console.log(`字典键 ${Object.keys(dict).length} 条，死键 ${bad.length} 条`);
  for (const k of bad) console.log('  × ' + JSON.stringify(k));
  if (bad.length) failed += bad.length;
}

// ---- 2. 真翻译 ----
const sandbox = {
  console,
  localStorage: { getItem: () => null, setItem: () => {} },
  document: {
    documentElement: {},
    body: null,
    addEventListener: () => {},
    createTreeWalker: undefined,
  },
  window: { addEventListener: () => {} },
  MutationObserver: undefined,
};
sandbox.window.document = sandbox.document;
vm.createContext(sandbox);
vm.runInContext(fs.readFileSync(path.join(ROOT, 'src/js/i18n.js'), 'utf8'), sandbox);
const I18n = sandbox.window.I18n;
I18n.setLang('en', { persist: false });

/**
 * 键 = 界面/场景，值 = **渲染后真正出现的文本**（不是源码字面量）。
 *
 * 第三个字段：
 *   （省略） 严格 —— 译文里**一个汉字都不许剩**
 *   'prefix'  宽松 —— 只要求**包装句**被翻译。这些串里嵌了后端返回的
 *             `e.message`（Rust 侧 `Err("…".into())`，本身就是中文），
 *             把几百条后端错误文案全部双语化是另一件事；
 *             但包装句漏翻属于本检查范围，所以仍断言「中文前缀必须消失」。
 */
const CASES = [
  // ---- 迷你悬浮窗 ----
  ['mini 标题', '背词'],
  ['mini 按钮', '认识'],
  ['mini 按钮', '不认识'],
  ['mini 空态', '词库里还没有可背的词。先在主窗口导入或下载一本词库。'],
  ['mini 取词失败', '取词失败：当前没有进行中的题目', 'prefix'],
  ['mini 无释义', '这个词还没有释义 —— 点右上角「补全」让 AI 生成完整词条。'],
  ['mini 义项数', '…共 5 个义项'],
  ['mini 例句朗读按钮', '朗读例句'],
  ['mini 词组标题', '词组 / 变形'],
  ['mini 未背过', '还没背过'],
  ['mini 状态行', '熟练度 40% · 正确率 75% · 下次复习 in 3 d'],
  ['mini 进度', '今日 12 · 待复习 30'],
  ['mini 反馈（认识）', '认识 · 下次复习 2026-10-13 09:00'],
  ['mini 反馈（不认识）', '不认识 · 下次复习 很快'],
  ['mini 已掌握', '「carpet」已掌握'],
  ['mini 补全中', '补全中…'],
  ['mini 补全失败', '没补出新内容（可先在设置页部署本地大模型）'],
  ['mini 补全成功', '已补全「carpet」的词条'],
  ['mini 尺寸提示', '尺寸 360×220'],
  ['mini 透明度提示', '不透明度 78%'],
  ['mini 置顶提示', '已置顶'],
  ['mini 取消置顶提示', '已取消置顶'],
  ['mini 提交失败', '提交失败'],
  // 多占位符：回填按**捕获顺序**，译文里 {n} 的顺序必须与键一致
  ['mini 多占位符语序', '为「apple」新增 8 条关系'],

  // ---- 详情卡状态行（同一句话的另一个渲染点）----
  ['详情卡 未学习', '尚未学习'],
  ['详情卡 熟练度', '熟练度 40%'],
  ['详情卡 正确率', '· 正确率'],
  ['详情卡 下次复习', '% · 下次复习 in 3 d'],

  // ---- 词图 ----
  ['图谱 关系类型', '同义'],
  ['图谱 关系类型', '反义'],
  ['图谱 关系类型', '派生'],
  ['图谱 关系类型', '相关'],
  ['图谱 关系类型', '上义'],
  ['图谱 关系类型', '下义'],
  ['图谱 构建中', '构建中…'],
  ['图谱 已清空', '已清空图谱关系'],
  ['图谱 发散中', 'AI 发散中…'],
  ['图谱 空态', '先在图上点一个词，或搜索一个词'],
  ['图谱 无匹配', '没有匹配的词'],
  // 被 <b> 拆散的文本节点：只能按碎片建键
  ['图谱 图例碎片 1', '共'],
  ['图谱 图例碎片 2', '个词 /'],
  ['图谱 图例碎片 3', '条关系'],
  ['图谱 面包屑碎片 1', '以'],
  ['图谱 面包屑碎片 2', '为中心'],
  ['图谱 空图', '还没有关系数据。点「构建图谱」从词库抽取关系。'],
  ['图谱 全局视图', '全局视图 · 点任意词以它为中心展开'],
  ['图谱 无释义', '暂无释义（这个词只有关系数据，没有完整词条）。'],
  ['图谱 连接数', '连接 3 条'],
  ['图谱 连接数+熟练度', '· 熟练度 40%'],
  ['图谱 元信息 A', '连接 3 条 · 词库已收录'],
  ['图谱 元信息 B', '连接 3 条 · 熟练度 40% · 词库已收录'],
  ['图谱 元信息 C', '连接 3 条 · 词库外（模型发散）'],
  ['图谱 悬停提示', '连接 3 条 · 单击看介绍 · 双击发散'],
  ['图谱 抽取结果', '已从词库抽取 120 条新关系'],
  ['图谱 发散结果', '为「apple」新增 8 条关系'],
  ['图谱 已隐藏', '已隐藏 2 类'],
  ['图谱 无模型提示', '（可在设置页「本地大模型」一键部署）'],
  ['图谱 搜索失败', '搜索失败：网络不可用', 'prefix'],
  ['图谱 读取失败', '读取图谱失败：数据库忙', 'prefix'],
  ['图谱 当前显示', '（当前显示 45 条）'],
  ['图谱 按钮', '以它为中心展开'],
  ['图谱 按钮', '回到全局'],

  // ---- 词条质量面板 ----
  ['质量 标题', '词条质量'],
  ['质量 按钮', '语种统计'],
  ['质量 按钮', '清理语种错标'],
  ['质量 按钮', 'AI 自检一批'],
  ['质量 按钮', '停止'],
  ['质量 字段', '自检条数'],
  ['质量 说明 1', '语种闸门'],
  ['质量 说明 2', '：词条入库前按书写系统判定 —— 含假名判日语、含谚文判韩语、纯汉字判中文，只有拉丁字母的词才归入英语词表。第三方词表、粘贴导入、手动加词全部过这道闸门，混进来的日语会被拦下并计进报告。'],
  ['质量 说明 3', 'AI 自检'],
  ['质量 说明 4', '：每个词条纳入背词表时由本地大模型校一遍拼写、词性、中文释义与例句；有问题就修正并'],
  ['质量 说明 5', '复检一轮'],
  ['质量 说明 6', '，仍不合格才标记剔除（不真删，只是不再进出题队列）。每次校验都留日志，可在下面查。'],
  ['质量 状态', '通过'],
  ['质量 状态', '已修正'],
  ['质量 状态', '已剔除'],
  ['质量 进行中', '自检中…'],
  ['质量 失败', '自检失败'],
  ['质量 空态', '还没有自检记录。'],
  ['质量 停止提示', '已请求停止，当前这个词跑完就停'],
  ['质量 空词库', '词库还是空的。'],
  ['质量 清理中', '清理中…'],
  ['质量 面板', '学习数据'],
  ['质量 读取失败', '读取失败：文件不存在', 'prefix'],
  ['质量 日志读取失败', '读取自检日志失败：文件不存在', 'prefix'],
  ['质量 语种名', '意大利语'],
  ['质量 语种名', '葡萄牙语'],
  ['质量 语种名', '阿拉伯语'],
  ['质量 语种名', '泰语'],
  ['质量 语种名', '印地语'],
  ['质量 语种名', '希伯来语'],
  ['质量 语种名', '希腊语'],
];

let caseFail = 0;
for (const [where, src, mode] of CASES) {
  const out = I18n.translate(src);
  if (out === src) {
    console.log(`  × 【${where}】没命中：${JSON.stringify(src)}`);
    caseFail += 1;
    continue;
  }
  if (mode === 'prefix') {
    if (/失败：/.test(out)) {
      console.log(`  × 【${where}】包装句没翻：${JSON.stringify(src)} → ${JSON.stringify(out)}`);
      caseFail += 1;
    }
    continue;
  }
  if (HAS_HAN.test(out)) {
    console.log(`  × 【${where}】译文里还有汉字：${JSON.stringify(src)} → ${JSON.stringify(out)}`);
    caseFail += 1;
  }
}
const strict = CASES.filter((c) => !c[2]).length;
console.log(`渲染用例 ${CASES.length} 条（严格 ${strict} / 宽松 ${CASES.length - strict}），失败 ${caseFail} 条`);
failed += caseFail;

if (failed) {
  console.log(`\n结论：${failed} 项失败`);
  process.exit(1);
}
console.log('结论：全部通过');
process.exit(0);
