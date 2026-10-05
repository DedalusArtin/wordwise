#!/usr/bin/env node
/**
 * 前端渲染冒烟测试。
 *
 * 为什么需要它：
 *   `node --check` 只验语法，抓不出「未声明的全局标识符」这类运行时才炸的 bug。
 *   典型事故：speakBtn 只挂在 `window.WW.speakBtn`，而 renderEntry 里用的是
 *   裸名字 `speakBtn(...)` → 只要 showPhonetic 默认开启就必然
 *   `ReferenceError: speakBtn is not defined`，表现是查词永远转圈。
 *
 * 做法：在最小沙箱里按 index.html 的顺序加载全部前端脚本（api.js 在无 Tauri
 *   环境下自动进 Mock 模式），然后真正调用关键渲染/查词函数。
 *
 * 用法：node scripts/smoke_frontend.cjs
 * 退出码：0 = 全部通过；1 = 有失败
 */
const fs = require('fs');
const path = require('path');
const vm = require('vm');

const ROOT = path.resolve(__dirname, '..');
const ORDER = ['api.js', 'speak.js', 'ui.js', 'study.js', 'lookup.js',
               'library.js', 'settings.js', 'sidebar.js', 'app.js'];

/** 假 style：支持 setProperty / removeProperty（分栏比例就走这两个）。 */
function fakeStyle() {
  const map = new Map();
  return {
    setProperty(k, v) { map.set(k, v); },
    removeProperty(k) { map.delete(k); },
    getPropertyValue(k) { return map.get(k) || ''; },
    get _map() { return map; },
  };
}

/** 假 DOM 元素：所有方法都安全空转，innerHTML 可读写。 */
function fakeEl(tag) {
  const el = {
    tagName: tag || 'div',
    id: '', className: '', innerHTML: '', textContent: '', value: '',
    disabled: false, dataset: {}, style: fakeStyle(),
    getBoundingClientRect: () => ({ width: 1000, height: 600, top: 0, left: 0, right: 1000, bottom: 600 }),
    setPointerCapture() {}, releasePointerCapture() {}, hasPointerCapture() { return false; },
    classList: {
      _s: new Set(),
      add(c) { this._s.add(c); }, remove(c) { this._s.delete(c); },
      toggle(c, on) { if (on === undefined) { this._s.has(c) ? this._s.delete(c) : this._s.add(c); } else if (on) { this._s.add(c); } else { this._s.delete(c); } },
      contains(c) { return this._s.has(c); },
    },
    _listeners: new Map(),
    addEventListener(type, fn) {
      if (!this._listeners.has(type)) this._listeners.set(type, []);
      this._listeners.get(type).push(fn);
    },
    removeEventListener() {},
    /** 触发已注册的监听器，用于验证拖动换算等交互逻辑 */
    _fire(type, ev) {
      (this._listeners.get(type) || []).forEach((fn) =>
        fn(Object.assign({ preventDefault() {}, stopPropagation() {}, button: 0, pointerId: 1 }, ev)));
    },
    appendChild() {}, append() {}, remove() {}, insertBefore() {},
    querySelector() { return null; }, querySelectorAll() { return []; },
    // 命运两组选择器：
    //  - `.catalog-card`：在线词库的下载按钮靠它找所属卡片；
    //  - 自身 class 命中的选择器：模拟「e.target 就在匹配的元素里」。
    // 其余仍返回 null，保持「大部分代码走降级分支」的原语义。
    closest(sel) {
      if (sel === '.catalog-card') return fakeEl();
      const c = sel && sel.startsWith('.') ? sel.slice(1) : '';
      if (c && (' ' + this.className + ' ').includes(' ' + c + ' ')) return this;
      return null;
    },
    contains() { return false; },
    setAttribute() {}, getAttribute() { return null; }, removeAttribute() {},
    focus() {}, blur() {}, click() {}, scrollTo() {}, scrollIntoView() {},
  };
  return el;
}

/** 同一个 id 每次返回同一个元素，行为更接近真实 DOM。 */
const elCache = new Map();
function elById(id) {
  if (!elCache.has(id)) { const e = fakeEl(); e.id = id; elCache.set(id, e); }
  return elCache.get(id);
}

const sandbox = {
  console,
  setTimeout, clearTimeout, setInterval, clearInterval, queueMicrotask,
  URLSearchParams, Promise, Date, Math, JSON, RegExp, Error,
  KeyboardEvent: function () {},
};
sandbox.window = sandbox;
sandbox.globalThis = sandbox;
sandbox.addEventListener = () => {};
sandbox.removeEventListener = () => {};
sandbox.innerWidth = 1440;
sandbox.innerHeight = 900;

const selCache = new Map();
sandbox.document = {
  getElementById: elById,
  // 让 querySelector 也返回元素（而不是 null），这样「布局初始化」这类
  // 依赖真实元素存在的代码也能被覆盖到。
  querySelector: (sel) => {
    if (!selCache.has(sel)) selCache.set(sel, fakeEl());
    return selCache.get(sel);
  },
  querySelectorAll: () => [],
  createElement: (t) => fakeEl(t),
  addEventListener() {},
  body: fakeEl('body'),
};
sandbox.location = { search: '' };
sandbox.navigator = { userAgent: 'node' };

// 真·内存版 localStorage：分栏比例/收起状态的持久化逻辑要能被验证
const lsMap = new Map();
sandbox.localStorage = {
  getItem: (k) => (lsMap.has(k) ? lsMap.get(k) : null),
  setItem: (k, v) => lsMap.set(k, String(v)),
  removeItem: (k) => lsMap.delete(k),
  clear: () => lsMap.clear(),
};
sandbox.Audio = function () {
  return { play: () => Promise.resolve(), pause() {}, addEventListener() {}, currentTime: 0 };
};
sandbox.SpeechSynthesisUtterance = function () {};
sandbox.speechSynthesis = { speak() {}, cancel() {}, getVoices: () => [] };
sandbox.fetch = () => Promise.reject(new Error('sandbox 内不发起真实请求'));

vm.createContext(sandbox);

let failed = 0;
for (const f of ORDER) {
  const p = path.join(ROOT, 'src', 'js', f);
  if (!fs.existsSync(p)) continue;
  try {
    vm.runInContext(fs.readFileSync(p, 'utf8'), sandbox, { filename: f });
  } catch (e) {
    console.log(`  [X] 加载 ${f} 失败 → ${e.name}: ${e.message}`);
    failed++;
  }
}

const WW = sandbox.WW;
if (!WW) {
  console.error('未拿到 window.WW，脚本加载失败');
  process.exit(1);
}

const entry = {
  word: 'abandon', lang: 'en',
  phonetic: { uk: '/əˈbæn.dən/', us: '/əˈbæn.dən/' },
  senses: [{
    pos: 'v.', definition: '放弃；抛弃',
    examples: [{ text: 'He abandoned his car.', translation: '他弃车而去。' }],
  }],
  inflections: [{ label: '过去式', form: 'abandoned' }],
  related: ['desert'], mnemonic: 'a-ban-don', source: 'free-dictionary',
};

const cases = [
  // ---- ui.js 纯渲染 ----
  ['renderEntry（默认，含音标+发音按钮）', () => WW.renderEntry(entry)],
  ['renderEntry（关音标）', () => WW.renderEntry(entry, { showPhonetic: false })],
  ['renderEntry（全开）', () => WW.renderEntry(entry, { showRelated: true, showMnemonic: true })],
  ['renderEntry（空词条）', () => WW.renderEntry(null)],
  ['renderMarkdown', () => WW.renderMarkdown('# 标题\n\n- 一\n- 二\n\n| a | b |\n| - | - |\n| 1 | 2 |')],
  ['speakBtn', () => WW.speakBtn(entry, 'us', '发音')],
  ['attachListSearch（元素缺失时降级）', () => WW.attachListSearch({ input: 'nope', list: 'nope' })],
  ['attachListSearch（正常挂载）', () => WW.attachListSearch({ input: 'lib-search', list: 'lib-list' }).apply()],

  // ---- Detail 详情卡（走 renderDefs / renderInfl / renderEx / renderRel） ----
  ['Detail.open()', () => sandbox.Detail.open(entry)],

  // ---- 查词页完整链路（Mock 后端） ----
  ['Lookup.loadWebResults()', () => sandbox.Lookup.loadWebResults('abandon')],
  ['Lookup.query("abandon")  ← 用户实际路径', () => sandbox.Lookup.query('abandon')],

  // ---- 查词页分栏：拖动条 / 收起右栏 ----
  ['分栏：setSplit 越界自动收敛到 0.30~0.78', () => {
    const hi = sandbox.Lookup.setSplit(0.95);
    const lo = sandbox.Lookup.setSplit(0.10);
    if (Math.abs(hi - 0.78) > 1e-6) throw new Error('上限应为 0.78，实际 ' + hi);
    if (Math.abs(lo - 0.30) > 1e-6) throw new Error('下限应为 0.30，实际 ' + lo);
    sandbox.Lookup.setSplit(0.52);
  }],
  ['分栏：--lk-left 被写入容器', () => {
    sandbox.Lookup.setSplit(0.5);
    const v = sandbox.document.querySelector('.lookup-cols').style.getPropertyValue('--lk-left');
    if (v !== '50.00%') throw new Error('期望 50.00%，实际 ' + v);
  }],
  ['分栏：拖动 pointermove 按像素换算比例（含上下限）', () => {
    sandbox.Lookup.initLayout();
    sandbox.Lookup.setSplit(0.52);
    const rz = elById('lk-resizer');
    rz._fire('pointerdown', { clientX: 520 });
    rz._fire('pointermove', { clientX: 700 });          // 容器宽 1000 → 700/1000
    if (Math.abs(sandbox.Lookup.currentSplit - 0.70) > 1e-6) {
      throw new Error('期望 0.70，实际 ' + sandbox.Lookup.currentSplit);
    }
    rz._fire('pointermove', { clientX: 100 });          // 撞左栏 300px 下限
    if (Math.abs(sandbox.Lookup.currentSplit - 0.30) > 1e-6) {
      throw new Error('左边界应收到 0.30，实际 ' + sandbox.Lookup.currentSplit);
    }
    rz._fire('pointermove', { clientX: 990 });          // 撞右栏 300px 下限
    if (Math.abs(sandbox.Lookup.currentSplit - 0.70) > 1e-6) {
      throw new Error('右边界应收到 0.70，实际 ' + sandbox.Lookup.currentSplit);
    }
    rz._fire('pointerup', {});
  }],
  ['分栏：收起/展开右栏 + 按钮文案同步', () => {
    const btn = elById('lk-ai-toggle');
    sandbox.Lookup.setCollapsed(true);
    if (!sandbox.Lookup.isCollapsed()) throw new Error('收起后 isCollapsed 应为 true');
    if (btn.textContent !== '展开右栏') throw new Error('按钮文案未同步：' + btn.textContent);
    sandbox.Lookup.setCollapsed(false);
    if (sandbox.Lookup.isCollapsed()) throw new Error('展开后 isCollapsed 应为 false');
    if (btn.textContent !== '收起右栏') throw new Error('按钮文案未同步：' + btn.textContent);
  }],
  ['分栏：比例与收起状态能跨会话恢复', () => {
    sandbox.Lookup.setSplit(0.6);
    sandbox.Lookup.setCollapsed(true);
    sandbox.Lookup.initLayout();     // 模拟下次启动
    if (Math.abs(sandbox.Lookup.currentSplit - 0.6) > 1e-6) {
      throw new Error('比例未恢复：' + sandbox.Lookup.currentSplit);
    }
    if (!sandbox.Lookup.isCollapsed()) throw new Error('收起状态未恢复');
    sandbox.Lookup.setCollapsed(false);
    sandbox.Lookup.setSplit(0.52);
  }],

  // ---- 其余模块的纯渲染 ----
  ['Leech/Library/Stats 无 DOM 时不抛错',
    () => Promise.all([sandbox.Leech.load(), sandbox.Library.loadList(), sandbox.Stats.load()])],

  // ---- 词库页四个标签的渲染（走过「本地词库 / 在线词库 / 单词列表 / 已背过的词」） ----
  ['Books.loadBooks()', () => sandbox.Books.loadBooks()],
  ['Books.loadCatalog()', () => sandbox.Books.loadCatalog()],
  ['Books.loadLearned()', () => sandbox.Books.loadLearned()],
  ['Books.switchTab("words")', () => sandbox.Books.switchTab('words')],
  ['Books.switchTab("learned")', () => sandbox.Books.switchTab('learned')],
  ['Books.switchTab("catalog")', () => sandbox.Books.switchTab('catalog')],

  // ---- 背诵 / 侧边栏 ----
  ['Study.setMode("en_to_zh")', () => sandbox.Study.setMode('en_to_zh')],
  ['Sidebar.quickQuiz()', () => sandbox.Sidebar.quickQuiz()],

  // ---- 各模块 bind()：这段在应用启动时必跑 ----
  ['Study.bind()', () => sandbox.Study.bind()],
  ['Detail.bind()', () => sandbox.Detail.bind()],
  ['Lookup.bind()', () => sandbox.Lookup.bind()],
  ['Library.bind()', () => sandbox.Library.bind()],
  ['Books.bind()', () => sandbox.Books.bind()],
  ['Settings.bind()', () => sandbox.Settings.bind()],
  ['Sidebar.bind()', () => sandbox.Sidebar.bind()],

  // ---- 在线词库「下载并导入」：点下去必须立刻有反应 ----
  //
  // 这条守的是用户报的「下载点了没反应」。按钮事件绑在 #lib-catalog 上做委托，
  // 这里朝容器派发一次 click，验证整条链路（委托 → 忙碌态 → 调用后端 → 复位）。
  // 必须放在各模块 bind() 之后，否则监听器还没挂上。
  ['在线词库：委托点击「下载并导入」有完整反馈', async () => {
    const box = elById('lib-catalog');
    const btn = fakeEl('button');
    btn.className = 'btn-book-download';
    btn.dataset.id = 'cet4-core';
    box._fire('click', { target: btn });
    if (btn.textContent !== '下载中…') {
      throw new Error('点击后按钮没进入忙碌态：' + btn.textContent);
    }
    if (btn.dataset.busy !== '1') throw new Error('缺少连点保护标记');
    // Mock 的后端有 60ms 模拟延迟，等足 150ms 再断言
    await new Promise((r) => setTimeout(r, 150));
    if (btn.disabled) throw new Error('下载结束后按钮仍是 disabled');
    if (btn.textContent !== '重新下载') {
      throw new Error('按钮文案未复位：' + btn.textContent);
    }
  }],

  // ---- AI 讲解语言（第十三轮） ----
  //
  // 守的是用户报的「选了目标语言，AI 讲解还是全英文」：
  //  - 后端返回的已是 ExplainResult 结构体，前端必须取 `.text` 渲染，
  //    直接把返回值塞进 innerHTML 会变成 `[object Object]`；
  //  - 切换语言要立刻落盘，下次讲解/追问才能沿用（需求 5）；
  //  - 译文替换原文后仍要能「查看原文」对照（需求 4）。
  ['讲解语言：下拉填充 + 切换即落盘', async () => {
    const opts = sandbox.WordWiseAPI.explainLangOptions('ja');
    if (!/value="ja"\s+selected/.test(opts)) throw new Error('当前语言未被选中：' + opts);
    if ((opts.match(/<option/g) || []).length < 5) throw new Error('可选语言太少');

    await sandbox.WordWiseAPI.API.setExplainLang('ja');
    const cfg = await sandbox.WordWiseAPI.API.getConfig();
    if (cfg.explain_lang !== 'ja') throw new Error('没记住上次的讲解语言：' + cfg.explain_lang);
    await sandbox.WordWiseAPI.API.setExplainLang('zh');   // 复原，别影响后续用例
  }],

  ['讲解：ExplainResult 渲染正文 + 「查看原文」可来回切换', () => {
    const box = elById('ai-body');
    const res = {
      word: 'abandon',
      text: '## 核心含义\n放弃；抛弃。',
      original: '## Core meaning\nto give up completely.',
      lang: 'zh', translated: true,
    };
    sandbox.Detail.renderExplain(box, res, false);
    if (!box.innerHTML.includes('已自动翻译为')) throw new Error('缺少语言提示条');
    if (!box.innerHTML.includes('放弃')) throw new Error('渲染的不是译文');
    if (box.innerHTML.includes('give up')) throw new Error('译文态不该出现原文');
    if (!box.innerHTML.includes('ai-orig-toggle')) throw new Error('缺少「查看原文」入口');

    sandbox.Detail.renderExplain(box, res, true);
    if (!box.innerHTML.includes('give up')) throw new Error('切到原文后仍显示译文');
    if (!box.innerHTML.includes('返回译文')) throw new Error('切换后按钮文案没变');
  }],

  // ---- 整应用启动链路 ----
  ['App.init()', () => sandbox.App.init()],
  ['Settings.load()', () => sandbox.Settings.load()],
  ['Plan.load()', () => sandbox.Plan.load()],
];

(async () => {
  for (const [name, fn] of cases) {
    try {
      const out = await fn();
      const extra = typeof out === 'string' ? `（${out.length} 字符）` : '';
      console.log(`  [OK] ${name}${extra}`);
    } catch (e) {
      console.log(`  [X] ${name} → ${e.name}: ${e.message}`);
      failed++;
    }
  }
  console.log('');
  // 必须显式退出：App.init() 里注册了 setInterval，否则 node 会一直挂着不结束
  if (failed) {
    console.log(`结论：${failed} 项失败`);
    process.exit(1);
  }
  console.log('结论：全部通过');
  process.exit(0);
})();
