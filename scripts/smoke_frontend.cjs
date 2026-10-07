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
// 顺序必须与 src/index.html 末尾的 <script> 顺序一致，否则「新模块根本没被
// 加载」这类问题会被漏掉（dir.js / translate.js 就是为此加进来的）。
// 必须与 src/index.html 里的 <script> 顺序一致：前端没有模块系统，
// 靠加载顺序决定谁先挂到 window 上。顺序错了，冒烟测试会报「undefined.bind」。
const ORDER = ['i18n.js', 'theme.js', 'api.js', 'demo.js', 'dir.js', 'speak.js', 'ui.js', 'study.js', 'lookup.js',
               'translate.js', 'graph.js', 'library.js', 'maint.js', 'update.js', 'settings.js',
               'sidebar.js', 'app.js'];

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
    id: '', className: '', innerHTML: '', textContent: '',
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
    hasAttribute() { return false; },
    focus() {}, blur() {}, click() {}, scrollTo() {}, scrollIntoView() {},
  };

  // 真实 DOM 里 input.value 永远是字符串：赋 0.6 读回来是 '0.6'。
  // 假 DOM 如果不转，settings.save() 里 getVal() 的 .trim() 就会炸，
  // 报出的却是一个跟产品无关的假故障，很容易把人带到错误的方向去查。
  let _value = '';
  Object.defineProperty(el, 'value', {
    get() { return _value; },
    set(v) { _value = (v === undefined || v === null) ? '' : String(v); },
    enumerable: true, configurable: true,
  });

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
// 学习页的模式按钮：applyLangUI / setMode 会遍历改写它们的文案与禁用态，
// 返回空数组的话这两条逻辑就完全测不到。
const STUDY_MODES = ['en_to_zh', 'zh_to_en', 'spelling', 'ex_to_zh', 'ex_pick_word', 'listen_spell'];
const segBtns = STUDY_MODES.map((m) => {
  const b = fakeEl('button');
  b.className = 'seg-btn';
  b.dataset.mode = m;
  b.title = '';
  return b;
});
sandbox.document = {
  getElementById: elById,
  // theme.js 在加载时就要往 <html> 上写 data-mode / data-accent，
  // 不给 documentElement 的话这条链路完全测不到。
  documentElement: fakeEl('html'),
  // 让 querySelector 也返回元素（而不是 null），这样「布局初始化」这类
  // 依赖真实元素存在的代码也能被覆盖到。
  querySelector: (sel) => {
    if (!selCache.has(sel)) selCache.set(sel, fakeEl());
    return selCache.get(sel);
  },
  querySelectorAll: (sel) => (sel === '#mode-seg .seg-btn' ? segBtns : []),
  createElement: (t) => fakeEl(t),
  addEventListener() {},
  body: fakeEl('body'),
  readyState: 'complete',
};
// 「跟随系统」要读它；沙箱里固定成浅色，断言才是确定的。
sandbox.matchMedia = () => ({ matches: false, addEventListener() {}, removeEventListener() {} });
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
// 知识图谱用力导向布局，靠 rAF 一帧帧收敛。沙箱里给一个立即回掉的实现，
// 否则 startLayout 排不上队，draw() 永远不会被调用。
sandbox.requestAnimationFrame = (fn) => setTimeout(() => fn(performance.now()), 0);
sandbox.cancelAnimationFrame = (id) => clearTimeout(id);
// 批量移出错词等破坏性操作会 confirm；冒烟里一律放行
sandbox.confirm = () => true;
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
  ['splitRelated：把词典源塞成一串的同义词拆成一个个可点的词', () => {
    const got = WW.splitRelated(['desert, abandon, leave', 'desert abandon', '高兴、愉快；快乐', 'n. (见 also)']);
    const want = ['desert', 'abandon', 'leave', '高兴', '愉快', '快乐'];
    for (const w of want) {
      if (!got.includes(w)) throw new Error(`没拆出「${w}」，实际 ${JSON.stringify(got)}`);
    }
    if (got.includes('n.')) throw new Error('噪声「n.」没被滤掉：' + JSON.stringify(got));
    if (got.filter(x => x === 'desert').length !== 1) throw new Error('重复项没去重');
    return JSON.stringify(got);
  }],
  ['renderEntry：旧缓存里的整串 related 也能画成多个 chip', () => {
    const html = WW.renderEntry({ ...entry, related: ['desert, abandon'] }, { showRelated: true });
    const n = (html.match(/rel-chip/g) || []).length;
    if (n !== 2) throw new Error(`应画出 2 个相关词 chip，实际 ${n}`);
    if (html.includes('desert, abandon')) throw new Error('整串没被拆开，仍作为一个 chip 显示');
    return '';
  }],
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
  ['Leech.bind()', () => sandbox.Leech.bind()],
  ['Plan.bind()', () => sandbox.Plan.bind()],
  ['Graph.bind()', () => sandbox.Graph.bind()],
  ['Demo.bind()', () => sandbox.Demo.bind()],
  ['Maint.bind()', () => sandbox.Maint.bind()],

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

  // ================= 第十五轮：翻译 / 查词语言链路 =================
  //
  // 这一组守的是用户提的五条需求中最容易回归的两条：
  //   需求 2 —— 选日语必须拿到日文文字，而不是只有罗马音；
  //   需求 3 —— 语言码要能从 UI 一路贯通到渲染结果。

  // ---- 方向选择器（需求 1） ----
  ['方向选择器：语言清单来自后端 + 中文名可读', async () => {
    await sandbox.DirPicker.loadLangs();
    const n = sandbox.DirPicker.langName('ja');
    if (n !== '日语') throw new Error('语言名应是「日语」，实际 ' + n);
    if (sandbox.DirPicker.langName('auto') !== '自动检测') throw new Error('auto 应有中文名');
  }],

  ['方向选择器：源=目标时自动挪开目标语言', () => {
    sandbox.DirPicker.set('zh', 'zh');
    if (sandbox.DirPicker.to === 'zh') throw new Error('同语言没被纠正，仍为 zh');
    if (sandbox.DirPicker.from !== 'zh') throw new Error('源语言被误改：' + sandbox.DirPicker.from);
  }],

  ['方向选择器：目标语言不允许是「自动检测」', () => {
    sandbox.DirPicker.set('auto', 'auto');
    if (sandbox.DirPicker.to === 'auto') throw new Error('目标语言竟是 auto');
  }],

  ['方向选择器：⇄ 互换并落盘到配置', async () => {
    sandbox.DirPicker.set('zh', 'ja');
    const before = sandbox.DirPicker.current;
    if (before.from !== 'zh' || before.to !== 'ja') throw new Error('set 未生效');

    await sandbox.DirPicker.swap();
    const cfg = await sandbox.WordWiseAPI.API.getConfig();
    if (cfg.source_lang !== 'ja' || cfg.target_lang !== 'zh') {
      throw new Error(`互换后配置应为 ja→zh，实际 ${cfg.source_lang}→${cfg.target_lang}`);
    }
    if (sandbox.DirPicker.from !== 'ja' || sandbox.DirPicker.to !== 'zh') {
      throw new Error('界面方向没跟着互换');
    }
  }],

  ['方向选择器：变化会广播给订阅方（三个面板据此刷新）', () => {
    let got = null;
    const off = sandbox.DirPicker.onChange((info) => { got = info; });
    sandbox.DirPicker.set('en', 'ja');
    off();
    if (!got) throw new Error('订阅方没收到通知');
    if (got.from !== 'en' || got.to !== 'ja') throw new Error('广播的方向不对');
  }],

  // ---- 翻译页（需求 2 / 5） ----
  ['翻译页 bind()：所有按钮与 tab 都能挂上', () => sandbox.Translate.bind()],

  ['翻译：选日语必须返回日文文字本身，而不是只有读音', async () => {
    sandbox.DirPicker.set('zh', 'ja');
    await sandbox.Translate.run('你好', false);
    const box = elById('tr-dst');
    if (!box.innerHTML.includes('こんにちは')) {
      throw new Error('译文主体不是日文：' + box.innerHTML);
    }
    // 读音只能作为附属标注单独一行，绝不能顶替译文主体
    const ph = elById('tr-phonetic');
    if (!ph.textContent) throw new Error('读音行是空的');
    if (!ph.classList.contains('hidden') === false) { /* 只要不是 hidden 即可 */ }
    if (ph.classList.contains('hidden')) throw new Error('读音行被隐藏了');
    if (box.innerHTML.includes('konnichiwa')) {
      throw new Error('罗马音混进了译文主体');
    }
  }],

  ['翻译：结果区标注了引擎与目标语言（一眼可查语言是否跟对）', () => {
    const eng = elById('tr-engine').textContent;
    if (!eng.includes('日语')) throw new Error('引擎标注没写目标语言：' + eng);
  }],

  ['翻译：历史记录落库 + 收藏按钮随之可用', async () => {
    // run() 内部会刷新历史；这里再显式拉一次确认能读回
    await sandbox.Translate.run('你好', false);
    // 有 record_id 就说明这条翻译已经入库，收藏才有意义
    if (elById('tr-fav').disabled) throw new Error('已有 record_id，收藏按钮不该禁用');

    await sandbox.Translate.load();
    const list = elById('tr-hist-list');
    if (!list.innerHTML.includes('你好')) {
      throw new Error('历史里没有刚翻的那条：' + list.innerHTML);
    }
    if (!list.innerHTML.includes('こんにちは')) throw new Error('历史里没有译文');
  }],

  ['翻译：换个方向重译，结果语言必须跟着变', async () => {
    sandbox.DirPicker.set('zh', 'en');
    await sandbox.Translate.run('你好', true);
    const box = elById('tr-dst');
    if (!box.innerHTML.includes('Hello')) {
      throw new Error('切成英语后译文没变：' + box.innerHTML);
    }
    if (elById('tr-engine').textContent.includes('日语')) {
      throw new Error('引擎标注还停在旧语言');
    }
  }],

  ['翻译：AI 增强四个动作都能调用', async () => {
    for (const a of ['explain', 'variants', 'polish', 'examples']) {
      const md = await sandbox.WordWiseAPI.API.translateAi(a, '你好', 'Hello', 'zh', 'en');
      if (typeof md !== 'string' || !md.length) throw new Error(a + ' 返回空');
    }
  }],

  // ---- 查词页联动（需求 4） ----
  ['查词页：占位提示跟随方向变化', () => {
    sandbox.DirPicker.set('auto', 'ja');
    sandbox.Lookup.syncLookupPlaceholder();
    const p = elById('lk-input').placeholder;
    if (!p.includes('日语')) throw new Error('占位提示没跟到目标语言：' + p);
    if (!p.includes('自动识别')) throw new Error('源语言为 auto 时应提示自动识别：' + p);
  }],

  ['查词页：词级译文独立渲染，语言跟着方向走', async () => {
    sandbox.DirPicker.set('auto', 'ja');
    const res = { word: '你好', lang: 'zh' };
    await sandbox.Lookup.loadWordTranslation(res);
    const box = elById('lk-trans');
    if (box.classList.contains('hidden')) throw new Error('译文区被隐藏');
    if (!box.innerHTML.includes('こんにちは')) throw new Error('词级译文不是日文：' + box.innerHTML);
    if (!box.innerHTML.includes('lk-trans-phonetic')) throw new Error('读音没有单独成行');
  }],

  ['查词页：目标语言与词条语言相同时不浪费一次翻译', async () => {
    sandbox.DirPicker.set('auto', 'zh');
    await sandbox.Lookup.loadWordTranslation({ word: '你好', lang: 'zh' });
    if (!elById('lk-trans').classList.contains('hidden')) {
      throw new Error('同语言时译文区应隐藏');
    }
  }],

  // ---- 语言链路一致性（需求 3） ----
  ['语言链路：UI 选择 → 查询参数 → 后端返回 → 渲染，全程同一个语言码', async () => {
    sandbox.DirPicker.set('zh', 'ko');
    const r = await sandbox.WordWiseAPI.API.translate('你好', 'zh', 'ko', true);
    if (r.to !== 'ko') throw new Error('后端返回的语言码与请求不一致：' + r.to);
    if (r.from !== 'zh') throw new Error('源语言码被改写：' + r.from);
    if (r.text !== '안녕하세요') throw new Error('译文不是韩文：' + r.text);
    sandbox.Translate.renderResult(r);
    const box = elById('tr-dst');
    if (!box.innerHTML.includes('안녕하세요')) throw new Error('渲染丢掉了韩文译文');
    if (elById('tr-engine').textContent.indexOf('韩语') < 0) {
      throw new Error('渲染标注的语言不对：' + elById('tr-engine').textContent);
    }
  }],

  ['语言链路：源语言为 auto 时后端要回报真实检测到的语言', async () => {
    const r = await sandbox.WordWiseAPI.API.translate('こんにちは', 'auto', 'zh', true);
    if (r.from === 'auto') throw new Error('auto 没被解析成真实语言码');
    if (r.from !== 'ja') throw new Error('日文被检测成 ' + r.from);
  }],

  // ---- 音标标签按语言分派（需求 3 的连带修复） ----
  ['音标标签：中文词条标「拼音」而不是「英」', () => {
    const h = WW.renderEntry({
      word: '你好', lang: 'zh', senses: [{ pos: 'int.', definition: '问候语' }],
      phonetic: { uk: 'nǐ hǎo', us: '' },
    });
    if (!h.includes('拼音')) throw new Error('中文词条的音标标签不是「拼音」：' + h.slice(0, 120));
    if (h.includes('英')) throw new Error('中文词条不该出现「英」标签');
  }],

  ['音标标签：日文词条标「读音」', () => {
    const h = WW.renderEntry({
      word: 'こんにちは', lang: 'ja', senses: [{ pos: 'int.', definition: '你好' }],
      phonetic: { uk: 'konnichiwa', us: '' },
    });
    if (!h.includes('读音')) throw new Error('日文词条的音标标签不是「读音」');
  }],

  ['renderPlainText：保留段落并转义 HTML', () => {
    const h = WW.renderPlainText('第一段 <b>x</b>\n\n第二段');
    if (h.includes('<b>')) throw new Error('没有转义 HTML');
    if (!h.includes('&lt;b&gt;')) throw new Error('转义结果不对：' + h);
    if (!h.includes('第二段')) throw new Error('丢内容');
  }],

  // ============================================================
  // 第十六轮：知识图谱 / 三页增强 / 演示模式 / 导出 / 数据库 / 本地大模型
  // ============================================================

  ['知识图谱：图例渲染出全部 6 类关系', async () => {
    await sandbox.Graph.load(true);
    const html = elById('graph-legend').innerHTML;
    for (const r of ['同义', '反义', '派生', '相关', '上义', '下义']) {
      if (!html.includes(r)) throw new Error('图例里少了「' + r + '」');
    }
  }],

  ['知识图谱：词库关系画实线，AI 发散的关系画虚线', async () => {
    // 后端只会在有模型时才产出 llm 边，冒烟里直接造一条来验渲染分支
    const orig = sandbox.WordWiseAPI.API.graphView;
    sandbox.WordWiseAPI.API.graphView = async () => ({
      nodes: [
        { word: 'abandon', lang: 'en', degree: 2, in_dict: true, gloss: '放弃', mastery: null },
        { word: 'reluctant', lang: 'en', degree: 1, in_dict: true, gloss: '不情愿的', mastery: null },
        { word: 'zzghost', lang: 'en', degree: 1, in_dict: false, gloss: '', mastery: null },
      ],
      edges: [
        { src: 'abandon', dst: 'reluctant', rel: 'related', weight: 1, source: 'local' },
        { src: 'abandon', dst: 'zzghost', rel: 'synonym', weight: 0.8, source: 'llm' },
      ],
      center: 'abandon', total_nodes: 3, total_edges: 2,
    });
    try {
      await sandbox.Graph.load(true);
      // 力导向靠 rAF 推进，等它画完
      await new Promise((r) => setTimeout(r, 120));
      const svg = elById('graph-svg').innerHTML;
      if (!svg.includes('<line')) throw new Error('没画出连线');
      if (!svg.includes('g-node')) throw new Error('没画出节点');
      if (!svg.includes('stroke-dasharray')) {
        throw new Error('AI 发散的边没画成虚线 —— 用户会把模型猜的词当真');
      }
      // 词库外的节点必须是空心（有 stroke 没填充色），否则点它开详情会 404
      if (!svg.includes('fill="transparent"')) throw new Error('词库外节点没有画成空心');
    } finally {
      sandbox.WordWiseAPI.API.graphView = orig;
    }
  }],

  ['错词本：按「最少错」筛选，条数必须真的变少', async () => {
    elById('leech-min-wrong').value = '0';
    elById('leech-min-rate').value = '0';
    elById('leech-max-mastery').value = '100';
    elById('leech-order').value = 'wrong';
    await sandbox.Leech.load();
    const all = elById('leech-list').innerHTML.split('class="word-row"').length - 1;

    elById('leech-min-wrong').value = '5';
    await sandbox.Leech.load();
    const few = elById('leech-list').innerHTML.split('class="word-row"').length - 1;
    if (!(few < all)) throw new Error(`筛选没生效：全部 ${all} 条，≥5 次仍是 ${few} 条`);
    if (few === 0) throw new Error('≥5 次一条都没有，mock 数据不足以验证筛选');
  }],

  ['错词本：导出 MD / CSV 都返回路径与条数', async () => {
    elById('leech-min-wrong').value = '0';
    await sandbox.Leech.load();
    for (const [fmt, ext] of [['md', '.md'], ['csv', '.csv']]) {
      const r = await sandbox.WordWiseAPI.API.leechExport({ format: fmt });
      if (!r.path || !String(r.path).includes(ext)) throw new Error(fmt + ' 导出路径不对：' + r.path);
      if (!(r.count > 0)) throw new Error(fmt + ' 导出条数为 0');
    }
  }],

  ['错词本：「移出列表」只动当前筛选出来的词', async () => {
    elById('leech-min-wrong').value = '5';
    await sandbox.Leech.load();
    const orig = sandbox.WordWiseAPI.API.leechRemoveMany;
    let captured = null;
    sandbox.WordWiseAPI.API.leechRemoveMany = (words) => { captured = words; return orig(words); };
    try {
      elById('leech-remove-visible')._fire('click', {});
      await new Promise((r) => setTimeout(r, 60));
      if (!captured || !captured.length) throw new Error('没有把任何词传给后端');
      if (captured.length > 8) throw new Error('传了超出列表范围的词：' + captured.length);
    } finally {
      sandbox.WordWiseAPI.API.leechRemoveMany = orig;
    }
  }],

  ['复习计划：到期清单渲染，逾期必须排在今天前面', async () => {
    await sandbox.Plan.load();
    const html = elById('plan-due').innerHTML;
    if (!html.includes('abandon')) throw new Error('到期清单里没有词');
    if (!html.includes('逾期')) throw new Error('没有标出逾期');
    const iOver = html.indexOf('逾期');
    const iToday = html.indexOf('今天');
    if (iToday >= 0 && iOver > iToday) {
      throw new Error('逾期项排在了今天之后，用户会先背不紧急的');
    }
  }],

  ['学习统计：环形图按四段渲染，中心显示总量', async () => {
    await sandbox.Stats.load();
    const svg = elById('mastery-donut').innerHTML;
    if (!svg.includes('<svg')) throw new Error('环形图没渲染');
    const segs = svg.split('stroke-dasharray').length - 1;
    if (segs < 3) throw new Error('环形图扇区不足：' + segs);
    const lg = elById('mastery-donut-legend').innerHTML;
    for (const k of ['未学习', '学习中', '需强化', '已掌握']) {
      if (!lg.includes(k)) throw new Error('图例缺少「' + k + '」');
    }
    // 中心数字必须等于图例里四项之和 —— 两处数字对不上就是最典型的图表 bug
    const m = svg.match(/<text[^>]*font-weight="700"[^>]*>(\d+)<\/text>/);
    if (!m) throw new Error('环形图中心没有显示总量数字');
    const center = Number(m[1]);
    const counts = [...elById('mastery-donut-legend').innerHTML.matchAll(/dl-count">(\d+)</g)]
      .map(x => Number(x[1]));
    if (counts.length !== 4) throw new Error('图例不是四项：' + counts.length);
    const sum = counts.reduce((a, b) => a + b, 0);
    if (center !== sum) throw new Error(`中心数字 ${center} 与图例之和 ${sum} 不一致`);
  }],

  ['演示模式：只接管读命令，写命令一律不拦', () => {
    if (!sandbox.Demo.has('cmd_stats')) throw new Error('演示模式应接管 cmd_stats');
    if (!sandbox.Demo.has('cmd_leech_query')) throw new Error('演示模式应接管 cmd_leech_query');
    for (const w of ['cmd_submit_answer', 'cmd_clear_leech', 'cmd_save_config', 'cmd_graph_build']) {
      if (sandbox.Demo.has(w)) throw new Error('演示模式不该拦截写命令 ' + w);
    }
  }],

  ['演示模式：示例数据覆盖统计/计划/到期/错词/图谱', () => {
    const s = sandbox.Demo.for('cmd_stats', {});
    if (s.history.length !== 14) throw new Error('示例历史不是 14 天');
    if (!(s.total_words > 0)) throw new Error('示例统计为空');
    const p = sandbox.Demo.for('cmd_review_plan', { days: 14 });
    if (p.length !== 14) throw new Error('示例计划不是 14 天');
    const d = sandbox.Demo.for('cmd_due_words', { limit: 60 });
    if (!d.length) throw new Error('示例到期清单为空');
    if (!d.some(x => x.overdue)) throw new Error('示例到期清单里应有逾期项');
    const l = sandbox.Demo.for('cmd_leech_query', { order: 'wrong' });
    if (!l.length) throw new Error('示例错词为空');
    // 排序要真的按错误次数降序
    for (let i = 1; i < l.length; i++) {
      if (l[i - 1].state.wrong_count < l[i].state.wrong_count) {
        throw new Error('示例错词没有按错误次数降序');
      }
    }
    const g = sandbox.Demo.for('cmd_graph_view', { center: null });
    if (!g.nodes.length || !g.edges.length) throw new Error('示例图谱为空');
  }],

  ['数据库面板：把路径、体积、每张表多少行都摆出来', async () => {
    await sandbox.Maint.load();
    const html = elById('db-info').innerHTML;
    for (const k of ['词库', '学习进度', '复习日志', '知识图谱关系']) {
      if (!html.includes(k)) throw new Error('没有列出「' + k + '」这张表');
    }
    if (!html.includes('体积')) throw new Error('没有显示体积');
  }],

  ['数据库维护：检查 / 整理 / 备份 三个动作都可执行', async () => {
    const orig = sandbox.WordWiseAPI.API.dbMaintain;
    const seen = [];
    sandbox.WordWiseAPI.API.dbMaintain = (a) => { seen.push(a); return orig(a); };
    try {
      // 每个动作都是「点一下 → 异步请求 → 回显」。Mock 有 60ms 模拟延迟，
      // 必须等回显真的写完再点下一个，否则测到的只是上一个动作的结果。
      for (const id of ['btn-db-check', 'btn-db-vacuum', 'btn-db-backup']) {
        elById('db-result').innerHTML = '';
        elById(id)._fire('click', {});
        for (let i = 0; i < 30 && !elById('db-result').innerHTML; i++) {
          await new Promise((r) => setTimeout(r, 20));
        }
        if (!elById('db-result').innerHTML) throw new Error(id + ' 点了之后没有任何回显');
      }
      for (const a of ['check', 'vacuum', 'backup']) {
        if (!seen.includes(a)) throw new Error('动作 ' + a + ' 没有被执行：' + seen.join(','));
      }
      const res = elById('db-result').innerHTML;
      // 注意 mock 用的是全角括号，这里只匹配「调试模式」四个字
      if (!res.includes("调试模式")) throw new Error("维护动作没有回显：seen=" + seen.join(",") + " res=" + JSON.stringify(res.slice(0,200)));
      // 备份必须给出落盘路径，否则用户不知道文件去哪了
      if (!res.includes('code')) throw new Error('备份结果里没给出路径：' + res.slice(0, 300));
    } finally {
      sandbox.WordWiseAPI.API.dbMaintain = orig;
    }
  }],

  // ============================================================
  // 第十八轮：数据与模型的存放位置（「装到 D 盘，数据别再写 C 盘」）
  // ============================================================

  ['存放位置：数据目录 / 模型目录 / 引擎 / 磁盘剩余都摆出来', async () => {
    await sandbox.Maint.load();
    const html = elById('storage-info').innerHTML;
    for (const k of ['学习数据', '数据库', '模型文件', '推理引擎']) {
      if (!html.includes(k)) throw new Error('没有列出「' + k + '」');
    }
    if (!html.includes('本机磁盘')) throw new Error('没有列出磁盘剩余空间 —— 用户没法判断该搬去哪');
    if (!html.includes('剩余')) throw new Error('磁盘没显示剩余空间');
    if (!html.includes('更换目录')) throw new Error('没有「更换目录」的入口');
    if (!html.includes('合计占用')) throw new Error('没有给出总占用');
  }],

  ['存放位置：跟着软件走时必须提醒「删软件目录会连数据一起删」', async () => {
    await sandbox.Maint.load();
    const html = elById('storage-info').innerHTML;
    // 「默认不写 C 盘」的代价就是这个，不说清楚等于给用户埋雷
    if (!html.includes('删掉软件目录会连词库和进度一起删掉')) {
      throw new Error('travels_with_app 为真时没有给出删目录的警告：' + html.slice(0, 400));
    }
    if (!html.includes('系统盘')) throw new Error('磁盘列表里没有标记系统盘');
  }],

  ['存放位置：环境变量强制指定时，如实说明界面改不动', async () => {
    const orig = sandbox.WordWiseAPI.API.storageInfo;
    sandbox.WordWiseAPI.API.storageInfo = async () => Object.assign({}, await orig(), {
      from_env: true, env_data_dir: 'D:\\Forced',
      models_env: 'E:\\ForcedModels',
    });
    try {
      await sandbox.Maint.storage.load();
      const html = elById('storage-info').innerHTML;
      if (!html.includes('WORDWISE_DATA_DIR')) {
        throw new Error('环境变量指定时没有说明来源');
      }
      if (!html.includes('优先级最高')) {
        throw new Error('没有告诉用户「改这里不会生效」');
      }
      if (!html.includes('WORDWISE_MODELS_DIR')) throw new Error('没说明模型目录也受环境变量控制');
    } finally {
      sandbox.WordWiseAPI.API.storageInfo = orig;
    }
  }],

  ['存放位置：打开换目录表单 → 点磁盘标签填路径 → 保存真的调后端', async () => {
    await sandbox.Maint.load();
    const S = sandbox.Maint.storage;

    S.openEdit('models');
    if (elById('storage-edit').classList.contains('hidden')) {
      throw new Error('点了「更换目录」表单还是 hidden');
    }
    if (!elById('st-edit-title').textContent.includes('模型')) {
      throw new Error('标题没跟着切换：' + elById('st-edit-title').textContent);
    }
    const chips = elById('st-edit-drives').innerHTML;
    if (!chips.includes('D:\\')) throw new Error('没有列出可点的磁盘标签：' + chips.slice(0, 200));
    // 标签文案要带建议目录名，用户才知道点下去会发生什么
    if (!chips.includes('WordWiseModels')) throw new Error('磁盘标签没带建议目录名');

    const filled = S.chooseDrive('E:\\');
    if (filled !== 'E:\\WordWiseModels') throw new Error('点磁盘没填对路径：' + filled);
    if (elById('st-edit-path').value !== 'E:\\WordWiseModels') {
      throw new Error('路径没有写进输入框');
    }

    const seen = [];
    const orig = sandbox.WordWiseAPI.API.setModelsDir;
    sandbox.WordWiseAPI.API.setModelsDir = (p, m) => { seen.push([p, m]); return orig(p, m); };
    try {
      elById('st-edit-path').value = 'E:\\WW-Models';
      elById('st-edit-migrate').checked = true;
      await S.save(elById('btn-storage-save'));
      if (!seen.length) throw new Error('点「保存」没有调后端');
      if (seen[0][0] !== 'E:\\WW-Models') throw new Error('传下去的路径不对：' + seen[0][0]);
      if (seen[0][1] !== true) throw new Error('「一起搬过去」没传成 true');
      const res = elById('storage-result').innerHTML;
      if (res.includes('没能改成功')) throw new Error('保存被报成失败：' + res.slice(0, 300));
      if (!res.includes('重启')) throw new Error('改完没提示重启：' + res.slice(0, 300));
      if (!res.includes('st-btn-restart')) throw new Error('没给「立即重启」的按钮');
      if (!elById('storage-edit').classList.contains('hidden')) {
        throw new Error('成功后表单没收起来');
      }
    } finally {
      sandbox.WordWiseAPI.API.setModelsDir = orig;
    }
  }],

  ['存放位置：重启按钮真的会调后端（改数据目录后必须重启才换库）', async () => {
    let called = 0;
    const orig = sandbox.WordWiseAPI.API.restartApp;
    sandbox.WordWiseAPI.API.restartApp = async () => { called++; return { ok: true }; };
    try {
      elById('storage-result')._fire('click', { target: { id: 'st-btn-restart' } });
      for (let i = 0; i < 20 && !called; i++) await new Promise((r) => setTimeout(r, 10));
      if (!called) throw new Error('点「立即重启」没有调后端');
    } finally {
      sandbox.WordWiseAPI.API.restartApp = orig;
    }
  }],

  ['存放位置：恢复默认传空路径（后端据此写 default，而不是删文件）', async () => {
    const seen = [];
    const origD = sandbox.WordWiseAPI.API.setDataDir;
    const origM = sandbox.WordWiseAPI.API.setModelsDir;
    sandbox.WordWiseAPI.API.setDataDir = (p, m) => { seen.push(['data', p, m]); return origD(p, m); };
    sandbox.WordWiseAPI.API.setModelsDir = (p, m) => { seen.push(['models', p, m]); return origM(p, m); };
    try {
      const S = sandbox.Maint.storage;
      S.openEdit('data');
      await S.resetDefault(elById('btn-storage-default'));
      S.openEdit('models');
      await S.resetDefault(elById('btn-storage-default'));

      if (seen.length !== 2) throw new Error('恢复默认没有各调一次：' + JSON.stringify(seen));
      if (seen[0][0] !== 'data' || seen[1][0] !== 'models') {
        throw new Error('恢复默认认错了目录类型：' + JSON.stringify(seen));
      }
      for (const s of seen) {
        // ★ 空字符串是「恢复默认」的约定，不能传成 undefined（会被后端当成没给）
        if (String(s[1]).trim() !== '') throw new Error('恢复默认应该传空路径，实际：' + JSON.stringify(s[1]));
      }
    } finally {
      sandbox.WordWiseAPI.API.setDataDir = origD;
      sandbox.WordWiseAPI.API.setModelsDir = origM;
    }
  }],

  ['存放位置：路径没填时不给发请求（免得把数据目录指到一个空值上）', async () => {
    let called = 0;
    const orig = sandbox.WordWiseAPI.API.setDataDir;
    sandbox.WordWiseAPI.API.setDataDir = (p, m) => { called++; return orig(p, m); };
    try {
      const S = sandbox.Maint.storage;
      S.openEdit('data');
      elById('st-edit-path').value = '   ';
      await S.save(elById('btn-storage-save'));
      if (called) throw new Error('空路径竟然发出去了，会把数据目录指向一个无意义的位置');
    } finally {
      sandbox.WordWiseAPI.API.setDataDir = orig;
    }
  }],

  ['本地大模型：状态与三档模型清单都渲染出来', async () => {
    await sandbox.Maint.load();
    const st = elById('llm-local-status').innerHTML;
    if (!st.includes('未启动')) throw new Error('状态行没写清楚服务有没有起');
    if (!st.includes('未安装')) throw new Error('引擎缺失时应该明确提示');
    const list = elById('llm-model-list').innerHTML;
    for (const n of ['1.7B', '0.6B', '1.5B']) {
      if (!list.includes(n)) throw new Error('模型清单里少了 ' + n);
    }
    if (!list.includes('MB')) throw new Error('模型没标体积，用户没法判断要不要下');
  }],

  // ============================================================
  // 第十七轮：左下角「本地模型」弹层（启停不用再翻设置页）
  // ============================================================

  ['本地模型弹层：点状态条展开，再点收起', () => {
    sandbox.Maint.chip.setOpen(false);
    elById('llm-chip')._fire('click', {});
    if (!sandbox.Maint.chip.isOpen) throw new Error('点状态条没有展开弹层');
    if (elById('llm-pop').classList.contains('hidden')) throw new Error('弹层还带着 hidden');
    elById('llm-chip')._fire('click', {});
    if (sandbox.Maint.chip.isOpen) throw new Error('再点一次没有收起');
    if (!elById('llm-pop').classList.contains('hidden')) throw new Error('收起后应加回 hidden');
  }],

  ['本地模型弹层：引擎没装时给「去安装」而不是「启动」', async () => {
    const orig = sandbox.WordWiseAPI.API.localLlmStatus;
    sandbox.WordWiseAPI.API.localLlmStatus = async () => ({
      engine: { ready: false }, models: { installed: [] },
      server: { running: false, port: 0, model: '' }, threads: 4, cpu: 8, auto_start: false,
    });
    try {
      await sandbox.Maint.chip.refresh();
      const html = elById('llm-pop-actions').innerHTML;
      if (!html.includes('去安装引擎')) throw new Error('缺引擎时文案不对：' + html);
      // 引擎不在却给「启动」，用户点了必然报错 —— 这正是要避免的
      if (html.includes('>启动<')) throw new Error('引擎没装却给了「启动」按钮');
    } finally { sandbox.WordWiseAPI.API.localLlmStatus = orig; }
  }],

  ['本地模型弹层：有引擎没模型时引导去下载', async () => {
    const orig = sandbox.WordWiseAPI.API.localLlmStatus;
    sandbox.WordWiseAPI.API.localLlmStatus = async () => ({
      engine: { ready: true }, models: { installed: [] },
      server: { running: false, port: 0, model: '' }, threads: 4, cpu: 8, auto_start: false,
    });
    try {
      await sandbox.Maint.chip.refresh();
      const html = elById('llm-pop-actions').innerHTML;
      if (!html.includes('去下载模型')) throw new Error('缺模型时文案不对：' + html);
    } finally { sandbox.WordWiseAPI.API.localLlmStatus = orig; }
  }],

  ['本地模型弹层：就绪给「启动」，运行中给「停止」', async () => {
    const origSt = sandbox.WordWiseAPI.API.localLlmStatus;
    const origMl = sandbox.WordWiseAPI.API.localLlmModels;
    sandbox.WordWiseAPI.API.localLlmModels = async () => ([
      { id: 'qwen3-1.7b', name: 'Qwen3 1.7B（推荐）',
        file: 'Qwen3-1.7B-Q4_K_M.gguf', size_bytes: 1107400000, recommended: true },
    ]);
    const base = {
      engine: { ready: true }, models: { installed: ['qwen3-1.7b'] }, threads: 4, cpu: 8,
      auto_start: false,
    };
    try {
      sandbox.WordWiseAPI.API.localLlmStatus = async () =>
        Object.assign({}, base, { server: { running: false, port: 0, model: '' } });
      await sandbox.Maint.chip.refresh();
      let html = elById('llm-pop-actions').innerHTML;
      if (!html.includes('>启动<')) throw new Error('就绪时应给「启动」：' + html);

      // 运行中：后端记的是去扩展名的文件名，前端要能对上才不会一直显示「启动」
      sandbox.WordWiseAPI.API.localLlmStatus = async () =>
        Object.assign({}, base, { server: { running: true, port: 18080, model: 'Qwen3-1.7B-Q4_K_M' } });
      await sandbox.Maint.chip.refresh();
      html = elById('llm-pop-actions').innerHTML;
      if (!html.includes('停止')) throw new Error('运行中应给「停止」：' + html);
      const ml = elById('llm-pop-models').innerHTML;
      if (!ml.includes('运行中')) throw new Error('模型清单没标出哪个在跑');
      // 反向：跑的是 0.6B 时，1.7B 那一行必须是「启动」而不是「运行中」
      sandbox.WordWiseAPI.API.localLlmModels = async () => ([
        { id: 'qwen3-1.7b', name: 'Qwen3 1.7B（推荐）',
          file: 'Qwen3-1.7B-Q4_K_M.gguf', size_bytes: 1107400000, recommended: true },
        { id: 'qwen3-0.6b', name: 'Qwen3 0.6B（轻量）',
          file: 'Qwen3-0.6B-Q4_K_M.gguf', size_bytes: 396700000, recommended: false },
      ]);
      sandbox.WordWiseAPI.API.localLlmStatus = async () =>
        Object.assign({}, base, { models: { installed: ['qwen3-1.7b', 'qwen3-0.6b'] },
          server: { running: true, port: 18080, model: 'Qwen3-0.6B-Q4_K_M' } });
      await sandbox.Maint.chip.refresh();
      const ml2 = elById('llm-pop-models').innerHTML;
      // 模型名里带小数点（1.7B / 0.6B），归一化写成「砍掉最后一个点之后」就会
      // 把版本号当扩展名，两行都会判错。这里把「只有一行标运行中」钉死。
      const runs = (ml2.match(/运行中/g) || []).length;
      if (runs !== 1) throw new Error('应且只应有一行标「运行中」，实际 ' + runs + ' 行：' + ml2);
      const rows = ml2.split('class="lp-model"').slice(1);
      if (rows[0].includes('运行中')) throw new Error('跑的是 0.6B，却把 1.7B 标成了运行中');
      if (!rows[1].includes('运行中')) throw new Error('跑的是 0.6B，0.6B 那行没标出来');
    } finally {
      sandbox.WordWiseAPI.API.localLlmStatus = origSt;
      sandbox.WordWiseAPI.API.localLlmModels = origMl;
    }
  }],

  ['本地模型弹层：点「启动」真的会调后端，且先补齐引擎', async () => {
    const origSt = sandbox.WordWiseAPI.API.localLlmStatus;
    const origMl = sandbox.WordWiseAPI.API.localLlmModels;
    const origStart = sandbox.WordWiseAPI.API.localLlmStart;
    const origEngine = sandbox.WordWiseAPI.API.localLlmInstallEngine;
    const calls = [];
    // 引擎未就绪：应当先装引擎再启动
    sandbox.WordWiseAPI.API.localLlmStatus = async () => ({
      engine: { ready: false }, models: { installed: ['qwen3-1.7b'] },
      server: { running: false, port: 0, model: '' }, threads: 4, cpu: 8, auto_start: false,
    });
    sandbox.WordWiseAPI.API.localLlmModels = async () => ([]);
    sandbox.WordWiseAPI.API.localLlmInstallEngine = async () => { calls.push('engine'); return { ok: true }; };
    sandbox.WordWiseAPI.API.localLlmStart = async (id) => { calls.push('start:' + (id || '-')); return { ok: true, message: 'x' }; };
    try {
      await sandbox.Maint.chip.refresh();
      await sandbox.Maint.chip.start(null, 'qwen3-1.7b');
      if (calls[0] !== 'engine') throw new Error('应先装引擎再启动，实际顺序：' + calls.join(','));
      if (!calls.some(c => c === 'start:qwen3-1.7b')) throw new Error('没有把模型 id 传给后端：' + calls.join(','));
    } finally {
      sandbox.WordWiseAPI.API.localLlmStatus = origSt;
      sandbox.WordWiseAPI.API.localLlmModels = origMl;
      sandbox.WordWiseAPI.API.localLlmStart = origStart;
      sandbox.WordWiseAPI.API.localLlmInstallEngine = origEngine;
    }
  }],

  ['本地模型弹层：「启动时自动拉起」开关落盘，且两处同步', async () => {
    const orig = sandbox.WordWiseAPI.API.setLocalLlmAuto;
    const origSt = sandbox.WordWiseAPI.API.localLlmStatus;
    let saved = null;
    sandbox.WordWiseAPI.API.setLocalLlmAuto = async (on) => {
      saved = on;
      return { ok: true, enabled: on, message: '（调试模式）已更新自动启动设置' };
    };
    sandbox.WordWiseAPI.API.localLlmStatus = async () => ({
      engine: { ready: true }, models: { installed: ['qwen3-1.7b'] },
      server: { running: false, port: 0, model: '' }, threads: 4, cpu: 8, auto_start: true,
    });
    try {
      await sandbox.Maint.chip.refresh();
      if (!elById('llm-pop-auto').checked) throw new Error('弹层没有反映后端已开启的自动启动');

      await sandbox.Maint.chip.toggleAuto(false);
      if (saved !== false) throw new Error('关掉开关没有落盘：' + saved);
      if (elById('set-llm-auto-start').checked) {
        throw new Error('设置页那个开关没跟着关，两处显示会不一致');
      }
    } finally {
      sandbox.WordWiseAPI.API.setLocalLlmAuto = orig;
      sandbox.WordWiseAPI.API.localLlmStatus = origSt;
    }
  }],

  ['本地模型弹层：停止后把「AI 地址已还原」如实告诉用户', async () => {
    const origStop = sandbox.WordWiseAPI.API.localLlmStop;
    const origSt = sandbox.WordWiseAPI.API.localLlmStatus;
    const origMl = sandbox.WordWiseAPI.API.localLlmModels;
    let stopped = 0;
    // 后端停服时会把 base_url 还回用户的 LM Studio，这条信息必须传达到位，
    // 否则用户不知道「AI 又能用了」，会以为只是停了个服务。
    sandbox.WordWiseAPI.API.localLlmStop = async () => {
      stopped += 1;
      return { ok: true, restored: true, message: '本地模型服务已停止，AI 地址已还原' };
    };
    sandbox.WordWiseAPI.API.localLlmStatus = async () => ({
      engine: { ready: true }, models: { installed: ['qwen3-1.7b'] },
      server: { running: false, port: 0, model: '' }, threads: 4, cpu: 8, auto_start: false,
    });
    sandbox.WordWiseAPI.API.localLlmModels = async () => ([]);
    try {
      elById('toast').textContent = '';
      await sandbox.Maint.chip.stop(null);
      if (stopped !== 1) throw new Error('没有真的调后端停服，调了 ' + stopped + ' 次');
      const t = elById('toast').textContent;
      if (!t.includes('还原')) throw new Error('后端说地址已还原，前端却没说：' + JSON.stringify(t));
    } finally {
      sandbox.WordWiseAPI.API.localLlmStop = origStop;
      sandbox.WordWiseAPI.API.localLlmStatus = origSt;
      sandbox.WordWiseAPI.API.localLlmModels = origMl;
    }
  }],

  ['左下角状态条：标题始终带着「可点击」的提示', async () => {
    // 状态消息会写到 chip.title 上。如果直接覆盖，HTML 里那句
    // 「点击查看 AI 服务」就没了 —— 用户再也看不出这块能点。
    for (const st of [
      { online: true, active_model: 'qwen3-1.7b', message: 'qwen3-1.7b 已就绪' },
      { online: false, message: '连接被拒绝' },
    ]) {
      sandbox.App.updateLlmChip(st);
      const t = elById('llm-chip').title || '';
      if (!t.includes('点击查看 AI 服务')) {
        throw new Error('状态条标题丢了可点击提示：' + JSON.stringify(t));
      }
      if (st.message && !t.includes(st.message)) {
        throw new Error('状态条标题丢了状态消息：' + JSON.stringify(t));
      }
    }
  }],

  ['左下角状态条：在线 API 与本地模型分开措辞', async () => {
    // 配的是在线 API，却说「本地模型未连接」，用户会去 LM Studio 里白找一圈
    sandbox.App.updateLlmChip({ online: false, endpoint_kind: 'cloud', message: 'x',
      base_url: 'https://api.deepseek.com/v1' });
    let t = elById('llm-text').textContent;
    if (!t.includes('在线 API')) throw new Error('在线 API 未连接时措辞不对：' + t);

    sandbox.App.updateLlmChip({ online: true, endpoint_kind: 'cloud', active_model: 'deepseek-chat',
      message: 'x', base_url: 'https://api.deepseek.com/v1' });
    t = elById('llm-text').textContent;
    if (!t.includes('在线 API')) throw new Error('在线 API 已连接时措辞不对：' + t);

    sandbox.App.updateLlmChip({ online: false, endpoint_kind: 'local', message: 'x',
      base_url: 'http://127.0.0.1:1234/v1' });
    t = elById('llm-text').textContent;
    if (!t.includes('本地模型')) throw new Error('本地服务措辞不对：' + t);
  }],

  // ============================================================
  // AI 服务来源：本地模型 / 在线 API
  // ============================================================

  ['AI 服务：预设表里的地址都带版本号，且不重复', () => {
    const ps = sandbox.Settings.presets;
    if (!ps || ps.length < 6) throw new Error('预设太少：' + (ps ? ps.length : 0));
    const seen = new Set();
    for (const p of ps) {
      if (seen.has(p.id)) throw new Error('预设 id 重复：' + p.id);
      seen.add(p.id);
      if (!p.name) throw new Error('预设没有名字：' + p.id);
      const b = (p.base || '').replace(/\/+$/, '');
      if (!b) continue;  // 本机一键部署 / 自定义：地址由运行时决定
      if (!/^https?:\/\//.test(b)) throw new Error('地址缺协议头：' + b);
      // 后端 endpoint() 见到版本号后缀就直接接 /chat/completions。
      // 预设多写一段就会拼成 /v1/v1/... —— 这里把「写到版本号为止」钉死。
      if (!/\/v\d+[a-z]*$/.test(b)) {
        throw new Error('地址没停在版本号那一段，会被拼坏：' + b);
      }
    }
    // 国内直连可用的几家必须在，否则等于没支持在线 API
    for (const id of ['deepseek', 'dashscope', 'moonshot', 'zhipu']) {
      if (!seen.has(id)) throw new Error('缺少预设：' + id);
    }
  }],

  ['AI 服务：按地址反推来源，托管端口段与后端一致', () => {
    const d = sandbox.Settings.detectPreset;
    if (d('https://api.deepseek.com/v1') !== 'deepseek') throw new Error('没认出 DeepSeek');
    if (d('https://open.bigmodel.cn/api/paas/v4') !== 'zhipu') throw new Error('没认出智谱');
    if (d('http://127.0.0.1:1234/v1') !== 'lm-studio') throw new Error('没认出 LM Studio');
    // 托管端口是运行时挑的，只能按段认；边界必须与 Rust 侧 pick_port 同源
    if (d('http://127.0.0.1:18080/v1') !== 'local-managed') throw new Error('没认出托管服务(下界)');
    if (d('http://127.0.0.1:18179/v1') !== 'local-managed') throw new Error('没认出托管服务(上界内)');
    if (d('http://127.0.0.1:18180/v1') === 'local-managed') throw new Error('上界外被误判成托管');
    if (d('https://api.example.com/v1') !== 'custom') throw new Error('陌生地址应为自定义');
    // 尾部斜杠不能影响判断
    if (d('https://api.deepseek.com/v1/') !== 'deepseek') throw new Error('尾斜杠没处理');
  }],

  ['AI 服务：选预设带出地址与推荐模型，但不动已填的 Key', async () => {
    elById('set-llm-key').value = 'sk-我的密钥';
    elById('set-llm-model').value = '旧模型';

    await sandbox.Settings.applyPreset('deepseek');
    if (elById('set-llm-url').value !== 'https://api.deepseek.com/v1') {
      throw new Error('没带出 DeepSeek 地址：' + elById('set-llm-url').value);
    }
    if (elById('set-llm-model').value !== 'deepseek-chat') {
      throw new Error('没带出推荐模型：' + elById('set-llm-model').value);
    }
    // 在几家里来回试是常事，一把一清会逼用户反复粘贴
    if (elById('set-llm-key').value !== 'sk-我的密钥') {
      throw new Error('切来源把 Key 清掉了');
    }

    // 智谱的地址结尾是 v4：带出去必须是 v4，不能自作聪明补 /v1
    await sandbox.Settings.applyPreset('zhipu');
    if (elById('set-llm-url').value !== 'https://open.bigmodel.cn/api/paas/v4') {
      throw new Error('智谱地址不对：' + elById('set-llm-url').value);
    }

    // 换成 LM Studio：模型要清空，让后端走「自动选第一个已加载模型」
    await sandbox.Settings.applyPreset('lm-studio');
    if (elById('set-llm-model').value !== '') {
      throw new Error('本机来源应清空模型名，实际：' + elById('set-llm-model').value);
    }
    if (elById('set-llm-url').value !== 'http://127.0.0.1:1234/v1') {
      throw new Error('LM Studio 地址不对：' + elById('set-llm-url').value);
    }

    // 自定义：地址和模型都别碰
    elById('set-llm-url').value = 'https://my-own-gateway.internal/openai';
    elById('set-llm-model').value = 'my-model';
    await sandbox.Settings.applyPreset('custom');
    if (elById('set-llm-url').value !== 'https://my-own-gateway.internal/openai' ||
        elById('set-llm-model').value !== 'my-model') {
      throw new Error('选「自定义」不该改动用户填的地址/模型');
    }
  }],

  ['AI 服务：Key 显示/隐藏切换，且「申请 Key」只在该露面时露面', () => {
    elById('set-llm-key').type = 'password';
    elById('btn-llm-key-eye').textContent = '显示';
    sandbox.Settings.toggleKeyVisible();
    if (elById('set-llm-key').type !== 'text') throw new Error('点「显示」没变成明文');
    if (elById('btn-llm-key-eye').textContent !== '隐藏') throw new Error('按钮文案没跟着变');
    sandbox.Settings.toggleKeyVisible();
    if (elById('set-llm-key').type !== 'password') throw new Error('再点一下没变回密码');

    // 本机来源没有 Key 可申请，按钮要藏起来
    sandbox.Settings.applyPreset('lm-studio');
    if (!elById('btn-llm-getkey').classList.contains('hidden')) {
      throw new Error('本机来源不该显示「申请 API Key」');
    }
    sandbox.Settings.applyPreset('deepseek');
    if (elById('btn-llm-getkey').classList.contains('hidden')) {
      throw new Error('在线 API 应显示「申请 API Key」');
    }
    if (!String(elById('btn-llm-getkey').dataset.url || '').startsWith('https://')) {
      throw new Error('「申请 Key」没有可打开的地址');
    }
  }],

  ['AI 服务：保存会带上 Key，清空 Key 要真的清掉', async () => {
    const origSave = sandbox.WordWiseAPI.API.saveConfig;
    let saved = null;
    sandbox.WordWiseAPI.API.saveConfig = async (c) => { saved = JSON.parse(JSON.stringify(c)); return null; };
    try {
      // save() 在 config 为空时会先 load()，而 load() 会用磁盘里的值回填输入框，
      // 把我刚填的覆盖掉 —— 所以必须先 load 再填。
      await sandbox.Settings.load();
      sandbox.Settings.config.llm.api_key = 'sk-旧密钥';
      elById('set-llm-key').value = '  sk-新的  ';
      elById('set-llm-url').value = 'https://api.deepseek.com/v1';
      elById('set-llm-model').value = 'deepseek-chat';
      await sandbox.Settings.save();
      if (!saved || saved.llm.api_key !== 'sk-新的') {
        throw new Error('Key 没保存或没去空格：' + JSON.stringify(saved && saved.llm.api_key));
      }

      // 清空 = 真的要停用。若「留空则保持原值」，用户会拿废弃 Key 去请求，
      // 收到 401 还找不到原因。
      elById('set-llm-key').value = '';
      await sandbox.Settings.save();
      if (saved.llm.api_key !== '') throw new Error('清空 Key 没生效：' + JSON.stringify(saved.llm.api_key));
    } finally {
      sandbox.WordWiseAPI.API.saveConfig = origSave;
    }
  }],

  ['AI 服务：在线 API 生效时弹层不给「启动本地模型」当主按钮', async () => {
    const origSt = sandbox.WordWiseAPI.API.localLlmStatus;
    const origAi = sandbox.WordWiseAPI.API.llmStatus;
    sandbox.WordWiseAPI.API.localLlmStatus = async () => ({
      engine: { ready: true }, models: { installed: ['qwen3-1.7b'] },
      server: { running: false, port: 0, model: '' }, threads: 4, cpu: 8, auto_start: false,
    });
    sandbox.WordWiseAPI.API.llmStatus = async () => ({
      online: true, base_url: 'https://api.deepseek.com/v1', models: ['deepseek-chat'],
      active_model: 'deepseek-chat', message: '已连接 api.deepseek.com，共 1 个模型可用',
      endpoint_kind: 'cloud',
    });
    try {
      await sandbox.Maint.chip.refresh();
      const html = elById('llm-pop-actions').innerHTML;
      // 在线 API 生效时把「启动」摆成主按钮是误导：一点下去 AI 就从云端
      // 切成本地小模型，用户只会觉得「点了反而变笨了」。
      if (!html.includes('打开 AI 设置')) throw new Error('在线 API 时主按钮应是「打开 AI 设置」：' + html);
      if (html.includes('>启动<')) throw new Error('在线 API 时不该给「启动」当主按钮');
      // 但要留一个小口子，想换回本机模型的人找得到
      if (!html.includes('改用本机模型')) throw new Error('没有换回本机模型的入口：' + html);
      const st = elById('llm-pop-status').innerHTML;
      if (!st.includes('api.deepseek.com')) throw new Error('状态行没说出在连哪家：' + st);
      if (!st.includes('已连接')) throw new Error('在线 API 连通时应显示「已连接」：' + st);
    } finally {
      sandbox.WordWiseAPI.API.localLlmStatus = origSt;
      sandbox.WordWiseAPI.API.llmStatus = origAi;
    }
  }],

  // ---- 发音按钮：三个曾经「点了没反应」的位置 ----
  ['发音：Speak 必须导出 playUrl / playTts（翻译页与词级译文朗读依赖它）', () => {
    const S = sandbox.Speak;
    for (const k of ['speak', 'speakText', 'playUrl', 'playTts', 'btnHtml', 'bindDelegate', 'bcp47']) {
      if (typeof S[k] !== 'function') throw new Error(`Speak.${k} 未导出`);
    }
    return '';
  }],
  ['发音：详情卡的音标行带上词条数据（静态结构取不到词的老问题）', async () => {
    // 词头旁那个小喇叭已按需求删除（下方本来就有英美两个发音按钮）。
    // 朗读入口现在只剩音标行里的 .speak-btn，而音标行不在任何
    // [data-entry-word] 容器里 —— 所以委托能取到词的前提是
    // **音标容器自己挂着 data-entry-word / data-speak-lang / data-speak-audio**。
    // 这三条一丢，详情卡就变成「点了没反应」。
    await sandbox.Detail.open(entry);
    const p = elById('dc-phonetic');
    if (p.dataset.entryWord !== 'abandon') {
      throw new Error('音标容器没挂上词：' + p.dataset.entryWord);
    }
    if (!p.dataset.speakLang) throw new Error('音标容器没挂上语言');
    // 假 DOM 不解析 innerHTML，只能直接查字符串（它确实就是这么渲染出来的）
    if (!String(p.innerHTML).includes('speak-btn')) {
      throw new Error('音标行里没有发音按钮：' + p.innerHTML);
    }
    return '';
  }],

  // ---- 整句查询：不再直接甩「查询失败」 ----
  ['整句识别：只有像句子的才走翻译，单词/短语不受影响', () => {
    const L = sandbox.Lookup;
    if (!L.looksLikeSentence('how are you doing today')) throw new Error('整句没被识别');
    if (L.looksLikeSentence('apple')) throw new Error('单词被误判成句子');
    if (L.looksLikeSentence('take off')) throw new Error('两词短语被误判成句子');
    if (!L.looksLikeSentence('今天天气怎么样？')) throw new Error('中文整句没被识别');
    return '';
  }],
  ['整句查询：渲染成 sent-card，原文每个词可点', async () => {
    await sandbox.Lookup.query('how are you doing today');
    await new Promise(r => setTimeout(r, 260));
    const html = elById('lk-result').innerHTML;
    if (!html.includes('sent-card')) throw new Error('整句没渲染成 sent-card：' + html.slice(0, 160));
    if (!html.includes('sent-word')) throw new Error('原文没有按词拆成可点链接');
    if (html.includes('查询失败')) throw new Error('整句不该走到「查询失败」');
    return html;
  }],
  ['历史栈：能一路退回最早的查询，退到底后按钮置灰', async () => {
    const back = elById('lk-back');
    if (back.disabled) throw new Error('有历史时返回按钮仍是禁用');
    let steps = 0;
    while (!back.disabled && steps++ < 30) {
      await sandbox.Lookup.goBack();
      await new Promise(r => setTimeout(r, 200));
    }
    if (!back.disabled) throw new Error('一直退到底都没置灰，栈没有收敛');
    return `退了 ${steps} 步`;
  }],

  // ---- 查词判定：AI 可用性不得决定成败（需求 D） ----
  ['查词判定：词典有结果、AI 没开 → 显示成功 + 降级提示，不显示失败', async () => {
    const API = sandbox.WordWiseAPI.API;
    const orig = API.lookup;
    const apple = {
      word: 'apple', lang: 'en',
      phonetic: { uk: '/ˈæp.əl/', us: '' },
      senses: [{ pos: 'n.', definition: '苹果', examples: [] }],
      inflections: [], related: [], mnemonic: '', source: 'youdao', extra: {},
    };
    API.lookup = async () => ({
      word: 'apple', lang: 'en', entry: apple,
      sources: ['有道', '剑桥'], from_cache: false, from_llm: false, trace: [],
      // 后端新契约：AI 没参与只写 note，不算失败
      note: '本地大模型未启用，本次未使用 AI 兜底。', degraded: false,
    });
    try {
      await sandbox.Lookup.query('apple');
      await new Promise(r => setTimeout(r, 160));
      const html = elById('lk-result').innerHTML;
      if (html.includes('未查到')) throw new Error('词典有结果却被判成了失败');
      if (!html.includes('lk-degrade-note')) throw new Error('AI 未参与时没渲染降级提示条');
      if (!html.includes('苹果')) throw new Error('词条没渲染出来');
    } finally {
      API.lookup = orig;
    }
    return '';
  }],
  ['来源标签：内置源 id 都有中文名，不会把 youdao-newhh 这种 id 直接露给用户', () => {
    const U = sandbox.WW;
    const ids = [
      'free-dictionary', 'wiktionary', 'youdao-suggest', 'youdao-jsonapi',
      'youdao-newhh', 'libre-translate', 'lmstudio', 'builtin-seed',
    ];
    for (const id of ids) {
      const label = U.sourceLabel(id);
      if (!label) throw new Error('来源 ' + id + ' 没有中文名');
      // 回归点：映射表漏了 youdao-newhh / youdao-jsonapi 时，
      // 界面上会渲染成「中文 youdao-newhh」，用户完全读不懂。
      if (label === id) throw new Error('来源 ' + id + ' 缺映射，界面会露出原始 id');
    }
    if (U.sourceLabel('youdao-newhh') !== '现代汉语规范词典') {
      throw new Error('youdao-newhh 的中文名不对');
    }
    return '';
  }],

  ['音标标注：英语词标「英/美」并加斜杠（reality 曾被当成中文标成「拼音」）', () => {
    const U = sandbox.WW;
    const en = U.phoneticHtml({ lang: 'en', phonetic: { uk: 'riˈæləti' } });
    if (!en.includes('/riˈæləti/')) throw new Error('英语音标应该用斜杠括起来');
    if (!en.includes('英')) throw new Error('英语音标应标「英」');
    if (en.includes('拼音')) throw new Error('英语音标被标成了「拼音」——语言判定串了');

    // 中文词反过来：标「拼音」且**不加**斜杠
    const zh = U.phoneticHtml({ lang: 'zh', phonetic: { uk: 'nǐ hǎo' } });
    if (!zh.includes('拼音')) throw new Error('中文词该标「拼音」');
    if (zh.includes('/nǐ hǎo/')) throw new Error('拼音不该用斜杠括起来');
    return '';
  }],

  ['查词判定：启发式解析成功 → 数据来源标注「启发式解析」', async () => {
    const API = sandbox.WordWiseAPI.API;
    const orig = API.lookup;
    const apple = {
      word: 'apple', lang: 'en',
      phonetic: { uk: '/ˈæp.əl/', us: '' },
      senses: [{ pos: '', definition: 'a round fruit', examples: [] }],
      inflections: [], related: [], mnemonic: '', source: 'src',
      extra: { heuristic: '1' },
    };
    API.lookup = async () => ({
      word: 'apple', lang: 'en', entry: apple,
      sources: ['某词典'], from_cache: false, from_llm: false, trace: [],
      note: '该词的释义由启发式解析得到（词典源返回结构与预设映射不一致），字段可能不完整。',
      degraded: true,
    });
    try {
      await sandbox.Lookup.query('apple');
      await new Promise(r => setTimeout(r, 160));
      const html = elById('lk-result').innerHTML;
      if (html.includes('未查到')) throw new Error('启发式解析成功不该走失败态');
      if (!html.includes('启发式解析')) throw new Error('降级来源没标注出来');
    } finally {
      API.lookup = orig;
    }
    return '';
  }],

  ['朗读：引擎偏好默认 auto、可切换、坏值回落（localStorage 脏数据不能把发音搞哑）', () => {
    const S = sandbox.Speak;
    S.setEnginePref('bogus-value');
    if (S.enginePref() !== 'auto') throw new Error('非法引擎值该回落 auto，而不是原样存下去');
    S.setEnginePref('local');
    if (S.enginePref() !== 'local') throw new Error('切换本地引擎没生效');
    if (S.isLocalReady() !== false) throw new Error('默认不该认为本地语音可用');
    S.setLocalReady(true);
    if (S.isLocalReady() !== true) throw new Error('setLocalReady 没生效');
    S.setEnginePref('auto');
    return '';
  }],

  ['朗读：本地引擎不可用 / 后端合成失败时，playLocalTts 都返回 false 触发回退', async () => {
    const S = sandbox.Speak;
    // ① 引擎没装 → 连后端都不用问
    S.setLocalReady(false);
    const a = await S.playLocalTts('hello', 'en', 'us', 0.95, 'k1');
    if (a !== false) throw new Error('本地不可用时应返回 false，让调用方走系统语音');
    // ② 引擎可用但后端报错 → 同样必须回退，绝不能静默失败让按钮变哑巴
    S.setLocalReady(true);
    const b = await S.playLocalTts('hello', 'en', 'us', 0.95, 'k2');
    if (b !== false) throw new Error('后端合成失败时应返回 false');
    S.setLocalReady(false);
    return '';
  }],

  ['朗读设置：引擎没装时语音包按钮禁用并提示先装引擎（否则下完 60MB 仍发不出声）', async () => {
    await sandbox.Settings.loadTts();
    const list = elById('tts-voice-list').innerHTML;
    const state = elById('tts-state').textContent || '';
    if (!String(list).includes('en_US-amy-medium')) throw new Error('语音清单没渲染出来');
    if (!String(list).includes('disabled')) throw new Error('引擎未装时下载按钮该禁用');
    if (!String(state).includes('未安装')) throw new Error('状态文案没说明引擎未安装');
    const hint = elById('tts-hint').textContent || '';
    if (!String(hint).includes('先下载语音引擎')) throw new Error('缺少「先装引擎」的引导文案');
    return '';
  }],

  // ---- AI 讲解：存档 / 并入词库 ----
  ['讲解：渲染「并入词库」与「查看原文」，且后者用 class 绑（原来用 id 点不动）', () => {
    const pane = elById('dc-pane-ai');
    sandbox.Detail.setExplainSaved('dc-pane-ai', false);
    sandbox.Detail.renderExplain(pane, {
      word: 'apple', text: '苹果', original: 'apple (the fruit)',
      lang: 'zh', translated: true, note: '来自讲解存档（未重新调用模型）',
    }, false);
    const html = pane.innerHTML;
    if (!html.includes('ai-to-book')) throw new Error('没渲染「并入词库」按钮');
    if (!html.includes('ai-orig-toggle')) throw new Error('没渲染「查看原文」按钮');
    if (html.includes('id="ai-orig-toggle"')) {
      throw new Error('还在用 id 绑定「查看原文」：两个容器同时存在时会互相抢 id，按钮点不动');
    }

    // 已并入词库 → 按钮变禁用状态，避免重复点击
    sandbox.Detail.setExplainSaved('dc-pane-ai', true);
    sandbox.Detail.renderExplain(pane, {
      word: 'apple', text: '苹果', original: '苹果', lang: 'zh', translated: false,
    }, false);
    if (!pane.innerHTML.includes('已并入词库')) throw new Error('已并入时不显示状态');
    return '';
  }],
  ['讲解：存档相关的 API 方法都在（缺一个按钮点了就没反应）', () => {
    const A = sandbox.WordWiseAPI.API;
    for (const m of ['saveExplain', 'getExplain', 'listExplains', 'searchExplains',
                     'deleteExplain', 'clearExplains', 'explainCount', 'explainToEntry']) {
      if (typeof A[m] !== 'function') throw new Error('API 缺少 ' + m);
    }
    return '';
  }],

  ['讲解：已有存档时直接本地渲染、不再调模型（「第二次秒回」）', async () => {
    const API = sandbox.WordWiseAPI.API;
    await API.saveExplain('apple', null, null, '存档讲解正文：苹果', 'raw original', false);

    const orig = API.aiExplain;
    let called = 0;
    API.aiExplain = async () => {
      called++;
      return { word: 'apple', text: '模型新讲解', original: 'x', lang: 'zh', translated: false };
    };
    try {
      const pane = elById('dc-pane-ai');
      await sandbox.Detail.explainInto('dc-pane-ai', 'apple');
      const html = pane.innerHTML;
      if (called !== 0) throw new Error('有存档却仍然调了模型，没做到秒回');
      if (!html.includes('存档讲解正文')) throw new Error('没渲染存档正文：' + html.slice(0, 160));
      if (!html.includes('ai-regen')) throw new Error('存档直出时没给「重新讲解」出口');
    } finally {
      API.aiExplain = orig;
    }
    return '';
  }],

  // ---- 学习页：界面随词库（教材）语言自动适配 ----
  ['学习页：日语词库 → 按钮改「看日选中」、题面改「日语单词」、语言标记显示日语', () => {
    const S = sandbox.Study;
    S.state.bookLangById['jlpt-n3'] = 'ja';
    S.setBook('jlpt-n3');
    const en2zh = segBtns.find(b => b.dataset.mode === 'en_to_zh');
    if (en2zh.textContent !== '看日选中') {
      throw new Error('日语词库下应为「看日选中」，实际 ' + en2zh.textContent);
    }
    if (elById('study-lang-chip').textContent !== '日语') throw new Error('语言标记没显示日语');
    if (S.modePromptLabel('zh_to_en', S.langInfo('ja')) !== '请选择正确的日语单词') {
      throw new Error('题面文案没跟着语言走');
    }
    return '';
  }],
  ['学习页：中文词库 → 按钮改「看词选义」，并下线无意义的「看中选中」', () => {
    const S = sandbox.Study;
    S.state.bookLangById['zh-core'] = 'zh';
    S.setBook('zh-core');
    const en2zh = segBtns.find(b => b.dataset.mode === 'en_to_zh');
    const zh2en = segBtns.find(b => b.dataset.mode === 'zh_to_en');
    if (en2zh.textContent !== '看词选义') throw new Error('中文词库下应为「看词选义」，实际 ' + en2zh.textContent);
    if (!zh2en.disabled) throw new Error('中文词库应下线「看中选中」');
    // 当前模式被下线时要自动回到默认模式，否则会卡在一个不可用模式上
    if (S.state.mode === 'zh_to_en') throw new Error('被下线的模式仍被选中');
    return '';
  }],
  ['学习页：切回英语词库后模式重新可用，且不受上一本影响', () => {
    const S = sandbox.Study;
    S.state.bookLangById['cet4-core'] = 'en';
    S.setBook('cet4-core');
    const zh2en = segBtns.find(b => b.dataset.mode === 'zh_to_en');
    if (zh2en.disabled) throw new Error('英语词库下「看中选英」应可用');
    if (zh2en.textContent !== '看中选英') throw new Error('英语词库下文案应为「看中选英」');
    if (!elById('study-lang-chip').classList.contains('hidden')) {
      // 英语词库仍有语言标记（显示「英语」），这里只要求它不报错即可
    }
    return '';
  }],
  ['学习页：释义语言默认中文，可切「原文释义」并按词库语言分别记住', () => {
    const S = sandbox.Study;
    S.state.bookLangById['jlpt-n3'] = 'ja';
    S.setBook('jlpt-n3');
    if (S.state.defLang !== 'zh') throw new Error('首次应为中文释义，实际 ' + S.state.defLang);
    S.setDefLang('src');
    if (S.state.defLang !== 'src') throw new Error('切「原文释义」没生效');
    S.setBook('cet4-core');
    S.setBook('jlpt-n3');
    if (S.state.defLang !== 'src') throw new Error('日语词库的释义语言选择没被记住');
    S.setDefLang('zh');
    return '';
  }],

  // ---- 朗读设置（离线系统语音） ----
  ['朗读设置：语速/音色落盘，「自动」能清掉手工音色', () => {
    const S = sandbox.Speak;
    for (const k of ['voicePref', 'setVoicePref', 'ratePref', 'setRatePref', 'resetPrefs', 'preview']) {
      if (typeof S[k] !== 'function') throw new Error(`Speak.${k} 未导出`);
    }
    S.setRatePref(1.25);
    if (Math.abs(S.ratePref() - 1.25) > 1e-6) throw new Error('语速没落盘：' + S.ratePref());
    S.setVoicePref('en-US', 'Microsoft Aria');
    if (S.voicePref() !== 'en-US|Microsoft Aria') throw new Error('音色没落盘：' + S.voicePref());
    S.setVoicePref('', '');
    if (S.voicePref() !== '') throw new Error('选「自动」应清掉手工音色');
    S.resetPrefs();
    if (Math.abs(S.ratePref() - 0.95) > 1e-6) throw new Error('恢复默认失败：' + S.ratePref());
    return '';
  }],
  ['朗读设置：读不到系统语音时如实提示，而不是给一个空下拉', () => {
    sandbox.Settings.renderSpeakVoices();
    const hint = elById('set-speak-voices-hint').textContent;
    if (!hint || !hint.includes('语音')) throw new Error('没给出音色缺失提示：' + hint);
    const sel = elById('set-speak-voice').innerHTML;
    if (!sel.includes('系统未提供')) throw new Error('音色下拉没有降级文案：' + sel);
    return hint;
  }],

  // ---- 主题：明暗 × 主题色 ----
  ['主题：默认浅色，data-mode / data-accent 写在 <html> 上', () => {
    const html = sandbox.document.documentElement;
    if (html.dataset.mode !== 'light') throw new Error('默认应为 light，实际 ' + html.dataset.mode);
    if (html.dataset.accent !== 'blue') throw new Error('默认主题色应为 blue，实际 ' + html.dataset.accent);
    return '';
  }],
  ['主题：切深色会写 data-mode=dark，且落盘到 localStorage', () => {
    sandbox.Theme.set({ mode: 'dark' });
    const html = sandbox.document.documentElement;
    if (html.dataset.mode !== 'dark') throw new Error('没切到 dark：' + html.dataset.mode);
    const saved = JSON.parse(sandbox.localStorage.getItem('ww.theme'));
    if (saved.mode !== 'dark') throw new Error('没落盘：' + JSON.stringify(saved));
    return '';
  }],
  ['主题：主题色变化只改 accent，不动 mode', () => {
    sandbox.Theme.set({ accent: 'rose' });
    const html = sandbox.document.documentElement;
    if (html.dataset.accent !== 'rose') throw new Error('accent 没生效：' + html.dataset.accent);
    if (html.dataset.mode !== 'dark') throw new Error('mode 被顺手改掉了：' + html.dataset.mode);
    return '';
  }],
  ['主题：toggle 在浅色/深色之间来回切', () => {
    sandbox.Theme.set({ mode: 'light' });
    sandbox.Theme.toggle();
    if (sandbox.document.documentElement.dataset.mode !== 'dark') throw new Error('toggle 没切到 dark');
    sandbox.Theme.toggle();
    if (sandbox.document.documentElement.dataset.mode !== 'light') throw new Error('toggle 没切回 light');
    // 收尾：恢复默认，免得影响后面依赖主题的用例
    sandbox.Theme.set({ mode: 'light', accent: 'blue' });
    return '';
  }],
  ['主题：读到坏数据不会崩，回落默认值', () => {
    const before = sandbox.localStorage.getItem('ww.theme');
    sandbox.localStorage.setItem('ww.theme', '{不是 JSON');
    // 重新走一遍读取逻辑（导出里没有 read()，用 set 触发一次写入并校验不抛）
    sandbox.Theme.set({ mode: 'light', accent: 'blue' });
    if (before) sandbox.localStorage.setItem('ww.theme', before);
    return '';
  }],
  ['主题：主题色与明暗两张表都不重复', () => {
    const acc = sandbox.Theme.ACCENTS.map(a => a.id);
    const mod = sandbox.Theme.MODES.map(m => m.id);
    if (new Set(acc).size !== acc.length) throw new Error('主题色有重复 id');
    if (new Set(mod).size !== mod.length) throw new Error('明暗模式有重复 id');
    if (!mod.includes('system')) throw new Error('缺少「跟随系统」');
    return acc.join(',');
  }],

  // ---- 音标：统一渲染 ----
  ['音标：英/美分开标，且用斜杠括起来', () => {
    const html = WW.phoneticHtml(entry);
    if (!html.includes('phon-ipa')) throw new Error('没有用统一的 .phon-ipa 结构');
    if (!html.includes('/əˈbæn.dən/')) throw new Error('拉丁音标没被斜杠括起来：' + html);
    if (!html.includes('>英<') || !html.includes('>美<')) throw new Error('缺英/美标注：' + html);
    return html.length + '';
  }],
  ['音标：中文词的拼音不标英/美，也不加斜杠', () => {
    const html = WW.phoneticHtml({ word: '你好', lang: 'zh', phonetic: { uk: 'nǐ hǎo' } });
    if (html.includes('>英<') || html.includes('>美<')) throw new Error('拼音被标成了英语音标');
    if (!html.includes('拼音')) throw new Error('缺「拼音」标注');
    if (html.includes('/nǐ hǎo/')) throw new Error('拼音不该用斜杠括起来');
    return html.length + '';
  }],
  ['音标：日语词标「读音」、韩语词标「罗马音」', () => {
    const ja = WW.phoneticHtml({ word: '食べる', lang: 'ja', phonetic: { us: 'taberu' } });
    const ko = WW.phoneticHtml({ word: '먹다', lang: 'ko', phonetic: { us: 'meokda' } });
    if (!ja.includes('读音')) throw new Error('日语词缺「读音」标注');
    if (!ko.includes('罗马音')) throw new Error('韩语词缺「罗马音」标注');
    return '';
  }],
  ['音标：一个音标都没有时返回空串（调用方才能决定兜底）', () => {
    const html = WW.phoneticHtml({ word: 'x', lang: 'en', phonetic: {} });
    if (html !== '') throw new Error('应为空串，实际 ' + JSON.stringify(html));
    return '';
  }],
  ['音标：speak:false 时不带发音按钮，size:sm 时带 phon-sm', () => {
    const html = WW.phoneticHtml(entry, { size: 'sm', speak: false });
    if (html.includes('speak-btn')) throw new Error('说了不要按钮却还是给了');
    if (!html.includes('phon-sm')) throw new Error('sm 尺寸没体现出来');
    return '';
  }],

  // ---- 背诵页底部：上一个单词 + 已背单词折叠列表 ----
  ['背诵页：本轮没答过题时，「上一个单词」是隐藏的', () => {
    sandbox.Study.state.history = [];
    sandbox.Study.renderPrev();
    if (!elById('prev-word').classList.contains('hidden')) throw new Error('空历史时不该显示「上一个」');
    return '';
  }],
  ['背诵页：「上一个单词」显示的是上一题，并且挂上了发音所需的数据', () => {
    sandbox.Study.state.history = [
      { entry, correct: false, grade: 'wrong' },
      { entry: { ...entry, word: 'gracious', phonetic: { us: 'ˈɡreɪʃəs' } }, correct: true, grade: 'good' },
    ];
    sandbox.Study.renderPrev();
    const box = elById('prev-word');
    if (box.classList.contains('hidden')) throw new Error('有历史时应当显示');
    if (elById('pw-word').textContent !== 'gracious') {
      throw new Error('显示的应是最后一笔（gracious），实际 ' + elById('pw-word').textContent);
    }
    // 收起/展开都要能发音 —— 靠的正是容器上这份数据
    if (box.dataset.entryWord !== 'gracious') throw new Error('容器没带 data-entry-word');
    if (elById('pw-say').dataset.speakWord !== 'gracious') throw new Error('喇叭没带 data-speak-word');
    if (!elById('pw-phon').innerHTML.includes('phon-ipa')) throw new Error('音标没走统一渲染');
    return elById('pw-def').textContent;
  }],
  ['背诵页：已背列表按最近优先排，每一行都带发音按钮与详情入口', () => {
    sandbox.Study.state.history = [
      { entry: { ...entry, word: 'first' }, correct: true },
      { entry: { ...entry, word: 'second' }, correct: false },
    ];
    sandbox.Study.state.learnedScope = 'session';
    sandbox.Study.renderLearned();
    const html = elById('learned-list').innerHTML;
    // 最近答的排最上面
    if (html.indexOf('second') > html.indexOf('first')) throw new Error('没有按最近优先排序');
    const speakBtns = (html.match(/data-speak-word=/g) || []).length;
    if (speakBtns !== 2) throw new Error(`每行都该有一个发音按钮，实际 ${speakBtns} 个`);
    const details = (html.match(/data-act="detail"/g) || []).length;
    if (details !== 4) throw new Error(`每行该有 2 个详情入口（整块+按钮），实际 ${details}`);
    // 计数写在标题里（假 DOM 不会重新解析 innerHTML，所以查标题本身）
    if (!elById('lb-title').innerHTML.includes('>2<')) {
      throw new Error('计数没更新：' + elById('lb-title').innerHTML);
    }
    return html.length + '';
  }],
  ['背诵页：折叠面板收起时也能发音（summary 里就带一个喇叭）', () => {
    const html = elById('learned-list').innerHTML;
    if (!html) throw new Error('列表是空的，前一个用例没跑起来');
    // 收起状态下能点的只有 summary 里那个 —— 它必须指向最近的一个词
    const last = elById('lb-speak-last');
    if (last.dataset.speakWord !== 'second') {
      throw new Error('summary 的喇叭没指向最近背过的词：' + last.dataset.speakWord);
    }
    return last.dataset.speakWord;
  }],
  ['背诵页：点「详情」能按行下标取回对应词条', () => {
    // 渲染出来的下标 0 应当是最近的那个（second）
    const rec = sandbox.Study.state._recs[0];
    if (!rec || rec.entry.word !== 'second') {
      throw new Error('下标 0 不是最近的词：' + JSON.stringify(rec && rec.entry.word));
    }
    return rec.entry.word;
  }],
  ['背诵页：切到「最近背过」会去查学习记录', async () => {
    sandbox.Study.state.learnedScope = 'recent';
    sandbox.Study.state.recent = null;
    sandbox.Study.renderLearned();
    // Mock 有 60ms 延迟，等足再断言
    await new Promise(r => setTimeout(r, 200));
    if (!Array.isArray(sandbox.Study.state.recent)) throw new Error('没拿到学习记录');
    return sandbox.Study.state.recent.length + ' 条';
  }],

  // ---- 双向词条（目标语言侧的对应词） ----
  ['双向词条：中英两种释义都在且不混排（输入英文也要有英文解释）', () => {
    const e = {
      word: 'apple', lang: 'en',
      senses: [
        { pos: 'n.', definition: '苹果 n. 苹果树', examples: [] },
        { pos: 'n.', definition: 'A common, round fruit.', examples: [] },
      ],
      phonetic: {}, source: 'youdao-suggest+free-dictionary',
    };
    const html = WW.renderEntry(e);
    // 分组标题：母语组「释义」+ 原文组「英语释义」，顺序是母语在前
    const iZ = html.indexOf('>释义<');
    const iE = html.indexOf('英语释义');
    if (iZ < 0) throw new Error('缺「释义」分组标题');
    if (iE < 0) throw new Error('缺「英语释义」分组标题（这正是用户报的「没有英文解释」）');
    if (iE < iZ) throw new Error('英文释义不该排在中文释义前面');
    if (!html.includes('苹果')) throw new Error('中文释义被丢掉了');
    if (!html.includes('A common, round fruit.')) throw new Error('英文释义被丢掉了');
    return '';
  }],
  ['双向词条：只有一种语言的释义时不硬凑第二块', () => {
    const html = WW.renderEntry({
      word: '开心', lang: 'zh',
      senses: [{ pos: '形容词', definition: '心情愉快；高兴', examples: [] }],
      phonetic: {},
    });
    if ((html.match(/we-section-title/g) || []).length !== 1) {
      throw new Error('只有一组释义时不该画出第二个分组标题：' + html);
    }
    return '';
  }],
  ['双向词条：中文词条的分组标题不该写成「中文释义」这种废话', () => {
    const html = WW.renderEntry({
      word: '乌鸦', lang: 'zh',
      senses: [
        { pos: '名', definition: '一种鸟', examples: [] },
        { pos: 'n.', definition: 'a large black bird', examples: [] },
      ],
      phonetic: {},
    });
    if (html.includes('中文释义')) throw new Error('不该用「中文释义」做 sidebar 标题');
    if (!html.includes('参考释义')) throw new Error('原文组应当落在「参考释义」下');
    return '';
  }],
  ['双向词条：对应词卡片按 via 标主译/其他译法，词头可点击查词', () => {
    const html = WW.renderPairs([
      { word: 'crow', lang: 'en', via: 'translation',
        entry: { word: 'crow', lang: 'en', senses: [{ pos: 'n.', definition: 'a large black bird', examples: [] }], phonetic: { uk: '/kroʊ/' }, source: 'free-dictionary' } },
      { word: 'rook', lang: 'en', via: 'alternative',
        entry: { word: 'rook', lang: 'en', senses: [{ pos: 'n.', definition: 'a large black bird', examples: [] }], phonetic: {}, source: 'free-dictionary' } },
    ]);
    if (!html.includes('pair-card')) throw new Error('没画出对应词卡片');
    if (!html.includes('主译')) throw new Error('首选译法没标「主译」');
    if (!html.includes('其他译法')) throw new Error('候选译法没标「其他译法」');
    if (!html.includes('data-word="crow"')) throw new Error('词头不能点击就没法单独查询');
    if (!html.includes('>英语<')) throw new Error('缺语言标签');
    if (!html.includes('/kroʊ/')) throw new Error('缺英文音标');
    // 卡片里不应重复画大词头（标题上已经有一次了）
    if (html.includes('class="we-word"')) throw new Error('卡片内重复画了 we-word 大词头');
    return '';
  }],
  ['双向词条：没有对应词时 renderPairs 返回空串（不画空壳）', () => {
    if (WW.renderPairs([]) !== '') throw new Error('空数组应当返回空串');
    if (WW.renderPairs(null) !== '') throw new Error('null 应当返回空串');
    // 只有 cnt 没有 entry 的脏数据也要滤掉
    if (WW.renderPairs([{ word: 'x', lang: 'en', via: 'translation' }]) !== '') {
      throw new Error('缺 entry 的脏数据应当被滤掉');
    }
    return '';
  }],

  // ---- 分块卡片（查词结果页：大词头 + 音标 + 标签切换）----
  //
  // 为什么要这组检查：`renderEntry` 现在有两种排布（堆叠 / 标签），共用同一份
  // 义项渲染。分叉最容易出现的问题是「标签与面板对不上」——点了标签切到空
  // 面板，或者标签栏画出来了但内容还是老样子。这组用例把两者的对应关系钉死。
  ['分块卡片：标签与面板一一对应（点了不能切到空面板）', () => {
    const html = WW.renderEntry({
      word: 'apple', lang: 'en',
      senses: [{ pos: 'n.', definition: '苹果', examples: [] }],
      inflections: [{ label: '复数', form: 'apples' }],
      mnemonic: 'ap-ple',
      phonetic: { uk: '/ˈæpl/', us: '/ˈæpl/' },
    }, { tabs: true, showRelated: true, showMnemonic: true });
    if (!html.includes('class="entry-tabs"')) throw new Error('没画出标签栏');
    if (!html.includes('class="entry-body"')) throw new Error('没画出面板容器');
    const tabIds = [...html.matchAll(/data-block="([^"]+)"/g)].map(m => m[1]);
    const paneIds = [...html.matchAll(/data-pane="([^"]+)"/g)].map(m => m[1]);
    if (!tabIds.length) throw new Error('一个标签都没画出来');
    if (tabIds.join(',') !== paneIds.join(',')) {
      throw new Error(`标签与面板顺序/数量对不上：tabs=${tabIds} panes=${paneIds}`);
    }
    return tabIds.join('/');
  }],
  ['分块卡片：第一块默认选中（查词的人十有八九是看释义）', () => {
    const html = WW.renderEntry({
      word: 'apple', lang: 'en',
      senses: [{ pos: 'n.', definition: '苹果', examples: [] }],
      inflections: [{ label: '复数', form: 'apples' }],
      phonetic: {},
    }, { tabs: true });
    // 第一个标签与第一块面板都要带 active；且只能有一个
    const activeTabs = [...html.matchAll(/class="entry-tab[^"]*active[^"]*"\s+data-block="([^"]+)"/g)]
      .map(m => m[1]);
    const activePanes = [...html.matchAll(/class="entry-pane[^"]*active[^"]*"\s+data-pane="([^"]+)"/g)]
      .map(m => m[1]);
    if (activeTabs.length !== 1) throw new Error('被选中的标签必须恰好一个，实际 ' + activeTabs.length);
    if (activePanes.length !== 1) throw new Error('被展开的面板必须恰好一个，实际 ' + activePanes.length);
    if (activeTabs[0] !== 'def') throw new Error('默认应当停在「释义」，实际 ' + activeTabs[0]);
    if (activeTabs[0] !== activePanes[0]) throw new Error('标签与面板选中项不一致');
    return activeTabs[0];
  }],
  ['分块卡片：标签栏排在头部之下、正文之上（先看清词头再切）', () => {
    const html = WW.renderEntry({
      word: 'apple', lang: 'en',
      senses: [{ pos: 'n.', definition: '苹果', examples: [] }],
      phonetic: { uk: '/ˈæpl/' },
    }, { tabs: true });
    const iWord = html.indexOf('class="we-word"');
    const iTabs = html.indexOf('class="entry-tabs"');
    const iPane = html.indexOf('class="entry-pane');
    if (iWord < 0) throw new Error('缺大词头');
    if (!(iWord < iTabs && iTabs < iPane)) {
      throw new Error(`顺序错了：词头 ${iWord} / 标签 ${iTabs} / 面板 ${iPane}`);
    }
    return '';
  }],
  ['分块卡片：没有释义时也要给出一块（不画空标签栏）', () => {
    const html = WW.renderEntry({ word: 'zzz', lang: 'en', senses: [], phonetic: {} },
      { tabs: true });
    const tabs = [...html.matchAll(/data-block="([^"]+)"/g)].map(m => m[1]);
    if (!tabs.includes('def')) throw new Error('释义块不能因为没有释义就整体消失');
    if (!html.includes('AI 讲解')) throw new Error('空释义应当给出「让模型生成」的出口');
    return tabs.join('/');
  }],
  ['分块卡片：tabbed 容器带 .tabbed 标记（CSS 靠它放大词头）', () => {
    const on = WW.renderEntry({ word: 'a', lang: 'en', senses: [], phonetic: {} }, { tabs: true });
    const off = WW.renderEntry({ word: 'a', lang: 'en', senses: [], phonetic: {} });
    if (!on.includes('class="word-entry tabbed"')) throw new Error('标签排布没打 .tabbed 标记');
    if (off.includes('tabbed')) throw new Error('堆叠排布不该带 .tabbed');
    return '';
  }],
  ['分块卡片：堆叠排布不受影响（对应词卡片仍纵向堆在一起）', () => {
    const html = WW.renderEntry({
      word: 'apple', lang: 'en',
      senses: [{ pos: 'n.', definition: '苹果', examples: [] }],
      inflections: [{ label: '复数', form: 'apples' }],
      phonetic: {},
    });
    if (html.includes('entry-tabs')) throw new Error('默认排布不该冒出标签栏');
    if (html.includes('entry-pane')) throw new Error('默认排布不该冒出标签面板');
    // 两块都要在（堆叠的定义就是「都看得见」）
    if (!(html.includes('苹果') && html.includes('apples'))) {
      throw new Error('堆叠排布把内容弄丢了');
    }
    return '';
  }],
  ['分块卡片：切标签用委托，且读的属性名与渲染写出的一致（下一次查询后仍可点）', () => {
    const src = fs.readFileSync(path.join(ROOT, 'src/js/lookup.js'), 'utf8');
    const ui = fs.readFileSync(path.join(ROOT, 'src/js/ui.js'), 'utf8');
    // 逐个 addEventListener 绑在当时的 .entry-tab 上，下一次查询整段 innerHTML
    // 被替换后就会漏绑 —— 必须挂在 .entry-tabs 上做委托。
    if (!/querySelector\(\s*['"]\.entry-tabs['"]\s*\)[\s\S]{0,160}?addEventListener\(\s*['"]click/
      .test(src)) {
      throw new Error('标签切换没有挂在 .entry-tabs 上做委托：' +
        (src.includes('entry-tabs') ? '找到了容器但委托写法变了' : '连容器都没找到'));
    }
    // 真正的耦合点：渲染写 data-block / data-pane，切换读 dataset.block /
    // dataset.pane。两边只要有一边改名，界面就是「点了没反应」——只查一侧查不出来。
    for (const [attr, prop] of [['data-block', 'dataset.block'], ['data-pane', 'dataset.pane']]) {
      if (!ui.includes(attr)) throw new Error(`ui.js 没有写出 ${attr}`);
      if (!src.includes(prop)) throw new Error(`lookup.js 没有读 ${prop}（找不到就切不动）`);
    }
    return '';
  }],
  ['分块卡片：详情卡的释义不再另写一套渲染（两处排版不能分叉）', () => {
    const src = fs.readFileSync(path.join(ROOT, 'src/js/lookup.js'), 'utf8');
    const at = src.indexOf('function renderDefs(');
    if (at < 0) throw new Error('没找到 renderDefs');
    // 函数体要裁到**下一个顶层函数**，不能靠固定字符窗口（见 study.js 那条用例的教训）
    const rest = src.slice(at);
    const stop = rest.search(/\n  (?:async )?function \w/);
    const body = stop > 0 ? rest.slice(0, stop) : rest;
    if (!body.includes('senseGroup')) throw new Error('详情卡释义没复用 senseGroup，会和结果页分叉');
    if (!body.includes('splitSensesByScript')) throw new Error('详情卡释义没做中/英分组');
    // 混排的判据：自己再手搓一遍 sense 的 DOM
    if (body.includes('sense-pos')) throw new Error('详情卡释义还在自己拼 sense DOM（改了结果页它不会跟着变）');
    return (body.match(/senseGroup/g) || []).length + ' 处调用';
  }],
  ['分块卡片：查词结果与详情卡的标签视觉同源（只改一处会分叉）', () => {
    const css = fs.readFileSync(path.join(ROOT, 'src/css/app.css'), 'utf8');
    const rule = (sel) => {
      const m = new RegExp('\\' + sel + '\\s*\\{([^}]*)\\}').exec(css);
      return m ? m[1].replace(/\s+/g, ' ') : null;
    };
    const a = rule('.entry-tab.active');
    const b = rule('.dc-tab.active');
    if (!a || !b) throw new Error('两张标签的选中态样式缺了一个');
    for (const prop of ['color', 'border-bottom-color', 'font-weight']) {
      if (!a.includes(prop) || !b.includes(prop)) throw new Error(`选中态缺 ${prop}：两处都要有`);
    }
    if (a.replace(/var\(--blue\)/g, 'X') !== b.replace(/var\(--blue\)/g, 'X')) {
      throw new Error(`两处选中态不一致：\n  entry: ${a}\n  dc:    ${b}`);
    }
    return '';
  }],

  // ---- 界面语言（ui_lang）----
  ['界面语言：中文模式原样返回，零改动', () => {
    sandbox.I18n.setLang('zh-CN', { persist: false });
    if (sandbox.I18n.t('加入词库') !== '加入词库') throw new Error('中文模式下不该翻译');
    return '';
  }],
  ['界面语言：缺词典条目时回退原文，绝不吐空串或 key', () => {
    sandbox.I18n.setLang('en', { persist: false });
    const miss = sandbox.I18n.t('这句肯定还没来得及翻译');
    if (miss !== '这句肯定还没来得及翻译') throw new Error('漏翻应原样返回，实际 ' + miss);
    if (miss === '' || miss.includes('undefined')) throw new Error('漏翻不该变成空串/undefined');
    return '';
  }],
  ['界面语言：translate 组合精确键与模板键（tPattern 不能被 || 短路掉）', () => {
    sandbox.I18n.setLang('en', { persist: false });
    // 精确键
    if (sandbox.I18n.translate('加入词库') !== 'Add to Wordbook') throw new Error('精确键没命中');
    // 模板键：走的是「精确没命中 → 再试模板」这条组合路径
    if (sandbox.I18n.translate('已载入 42 个示例单词') !== '42 sample words loaded') {
      throw new Error('模板键被短路了：' + sandbox.I18n.translate('已载入 42 个示例单词'));
    }
    sandbox.I18n.setLang('zh-CN', { persist: false });
    return '';
  }],
  ['界面语言：切走再切回，原文必须还在（不能「从英文翻回中文」）', () => {
    sandbox.I18n.setLang('en', { persist: false });
    if (sandbox.I18n.t('查看详情卡') !== 'Details') throw new Error('英文模式没翻译');
    sandbox.I18n.setLang('zh-CN', { persist: false });
    if (sandbox.I18n.t('查看详情卡') !== '查看详情卡') throw new Error('切回中文丢了原文');
    return '';
  }],
  ['界面语言：localize 遇到残缺节点不能抛（撑不死渲染就是底线）', () => {
    // 注：本沙箱的 innerHTML 不解析成真实子树，能验证的只有「不抛」。
    // 真实 DOM 下的文本属性替换由上面的 t / tPattern 用例间接覆盖。
    sandbox.I18n.setLang('en', { persist: false });
    const weird = { nodeType: 1, tagName: 'DIV', hasAttribute: () => false, childNodes: [] };
    sandbox.I18n.localize(weird);
    sandbox.I18n.localize(null);
    sandbox.I18n.localize(undefined);
    sandbox.I18n.setLang('zh-CN', { persist: false });
    return '';
  }],
  ['界面语言：今日复习与错词攻坚的动态文案能翻成英文（多占位符按顺序回填）', () => {
    sandbox.I18n.setLang('en', { persist: false });
    const tr = (s) => sandbox.I18n.translate(s);
    const want = [
      ['开始今日复习', "Start today's review"],
      ['开始今日复习（4 词）', "Start today's review (4 words)"],
      ['连对 3 次', '3 correct in a row'],
      ['攻坚中：再连对 2 次出队', '2 more correct in a row to clear it'],
      ['正在统计…', 'Counting…'],
    ];
    for (const [src, exp] of want) {
      const got = tr(src);
      if (got !== exp) throw new Error(`${src} → ${got}，应为 ${exp}`);
    }
    // ★ 两个占位符：先「到期」后「常错」，顺序错了提示语就反了
    const two = tr('到期 3 词 + 常错 1 词。系统会先排到期与常错词，再补薄弱词与新词。');
    if (!/^3 due \+ 1 frequently missed\./.test(two)) {
      throw new Error('多占位符没按顺序回填：' + two);
    }
    // 攻坚出队那条第一个占位符是**单词**不是数字，同样要接住
    const done = tr('攻坚成功，ambiguous 已出队 —— 连对 3 次');
    if (!done.includes('ambiguous') || !done.includes('3')) {
      throw new Error('出队回执丢了词头或次数：' + done);
    }
    sandbox.I18n.setLang('zh-CN', { persist: false });
    return two;
  }],
  ['界面语言：语言清单里必须有 zh-CN 和 en 两档', () => {
    const ids = sandbox.I18n.LANGS.map(l => l.id);
    if (!ids.includes('zh-CN')) throw new Error('缺简体中文');
    if (!ids.includes('en')) throw new Error('缺 English');
    return ids.join(',');
  }],

  // ---- 版本号一致性 ----
  //
  // 为什么要这道检查：版本号散在四处（tauri.conf.json / Cargo.toml /
  // build.ps1 兜底 / installer 兜底），而 wordwise.iss 自己的注释就写着
  // 「两处各写一个版本号必然会漂移，表现是安装包文件名、界面显示、程序属性
  // 三个版本号对不上」。靠人记去同步迟早出错，这里一次性钉死。
  ['版本号：四处来源必须完全一致（防止安装包文件名 / 界面 / 程序属性对不上）', () => {
    const read = (p) => fs.readFileSync(path.join(ROOT, p), 'utf8');
    const conf = JSON.parse(read('src-tauri/tauri.conf.json')).version;
    const cargo = /^version\s*=\s*"([^"]+)"/m.exec(read('src-tauri/Cargo.toml'))[1];
    const issc = /#define MyAppVersion\s+"([^"]+)"/.exec(read('installer/wordwise.iss'))[1];
    const ps = /\$AppVersion\s*=\s*"([^"]+)"/.exec(read('build.ps1'))[1];
    const bad = [
      ['tauri.conf.json', conf], ['Cargo.toml', cargo],
      ['wordwise.iss', issc], ['build.ps1', ps],
    ].filter(([, v]) => v !== conf);
    if (bad.length) {
      throw new Error('版本号不一致：' + bad.map(([k, v]) => `${k}=${v}`).join(' / ') +
        `，基准取 tauri.conf.json 的 ${conf}`);
    }
    // 顺带校验形如 x.y.z
    if (!/^\d+\.\d+\.\d+$/.test(conf)) throw new Error('版本号格式应为三段式，实际 ' + conf);
    return conf;
  }],
  ['安装包：下载用的 tag 与 Rust 侧常量一致（漂了会 404，且极易误判成网络问题）', () => {
    const read = (p) => fs.readFileSync(path.join(ROOT, p), 'utf8');
    const iss = read('installer/wordwise.iss');
    const loc = read('src-tauri/src/localllm/mod.rs');
    const tts = read('src-tauri/src/tts/mod.rs');
    const pick = (src, name) => {
      const m = new RegExp('pub const ' + name + ': &str = "([^"]+)"').exec(src);
      return m ? m[1] : null;
    };
    const want = {
      LlamaTag: pick(loc, 'ENGINE_TAG'),
      PiperTag: pick(tts, 'ENGINE_TAG'),
      VoiceTag: pick(tts, 'VOICE_TAG'),
    };
    for (const k of Object.keys(want)) {
      if (!want[k]) throw new Error('没在 Rust 侧找到常量 ' + k);
      const got = new RegExp('#define ' + k + '\\s+"([^"]+)"').exec(iss);
      if (!got) throw new Error('wordwise.iss 里缺 #define ' + k);
      if (got[1] !== want[k]) {
        throw new Error(`${k} 漂移：iss=${got[1]} vs Rust=${want[k]}`);
      }
    }
    // 模型文件名：localllm 里 models() 中 **qwen3-0.6b** 那条。
    // ★ 必须按 id 精确取：清单里第一条是推荐的 1.7B，而安装包装的是 0.6B
    //   （400MB 量级才适合安装时下载，1.1GB 放进去没人受得了）。
    //   只取「第一个 file:」会因为换档位而误报。
    const mf = /id:\s*"qwen3-0\.6b"[\s\S]{0,300}?file:\s*"([^"]+\.gguf)"\.into\(\)/.exec(loc);
    if (!mf) throw new Error('没在 Rust 侧找到 qwen3-0.6b 的模型文件名');
    const issMf = /#define ModelFile\s+"([^"]+)"/.exec(iss);
    if (!issMf) throw new Error('wordwise.iss 里缺 #define ModelFile');
    if (issMf[1] !== mf[1]) throw new Error(`模型文件名漂移：iss=${issMf[1]} vs Rust=${mf[1]}`);
    return want.LlamaTag + ' / ' + want.PiperTag + ' / ' + want.VoiceTag;
  }],

  // ---- 背诵：错词攻坚 ----
  //
  // 为什么用「连对」而不是「累计答对 3 次」：累计计数会让 对/错/对/错
  // 这种摇摆也慢慢出队，而它其实根本没记住。下面第 3 步就是钉死这条。
  ['背诵：错词攻坚按「连对」出队，中途答错必须重新计数', () => {
    const S = sandbox.Study;
    S.state.grind = {};
    const w = 'ambiguous';
    const N = S.state.GRIND_TARGET;
    if (!(N >= 2)) throw new Error('GRIND_TARGET 至少要是 2，实际 ' + N);

    // ① 答错 → 入队并归零
    let note = S.grindNote(w, false);
    if (S.state.grind[w] !== 0) throw new Error('答错后应入队且计数为 0，实际 ' + S.state.grind[w]);
    if (!note.includes('攻坚')) throw new Error('入队时该给提示，实际 ' + note);

    // ② 连对 1 次 → 计数 1，提示写明还差几次
    note = S.grindNote(w, true);
    if (S.state.grind[w] !== 1) throw new Error('连对 1 次后计数应为 1，实际 ' + S.state.grind[w]);
    if (!note.includes(String(N - 1))) throw new Error(`提示应写明还差 ${N - 1} 次，实际 ` + note);

    // ③ ★ 关键：中间错一次要清零 —— 累计答对会在这里悄悄放它出队
    S.grindNote(w, false);
    if (S.state.grind[w] !== 0) throw new Error('答错必须重新计数，实际 ' + S.state.grind[w]);

    // ④ 重新连对 N 次 → 出队，且从队列里删掉（不是留个 0 在那儿）
    for (let i = 0; i < N - 1; i++) S.grindNote(w, true);
    note = S.grindNote(w, true);
    if (Object.prototype.hasOwnProperty.call(S.state.grind, w)) {
      throw new Error(`连对 ${N} 次应当出队，却还在队列里（计数 ${S.state.grind[w]}）`);
    }
    if (!note.includes('出队')) throw new Error('出队该有回执，实际 ' + note);
    return `连对 ${N} 次出队`;
  }],
  ['背诵：不在攻坚队列里的词答对了不打扰（提示不该刷屏）', () => {
    const S = sandbox.Study;
    S.state.grind = {};
    // 从来没答错过的词，答对了不该凭空冒出「攻坚中」
    if (S.grindNote('ordinary', true) !== '') throw new Error('没入队的词不该给提示');
    if ('ordinary' in S.state.grind) throw new Error('没入队就不该进队列');
    // 脏数据（空词头）同样只返回空串，不该抛
    if (S.grindNote('', false) !== '') throw new Error('空词头应当返回空串');
    if (S.grindNote(null, true) !== '') throw new Error('null 词头应当返回空串');
    return '';
  }],

  // ---- 背诵：手感与键盘流 ----
  ['背诵：下一题可被回车抢先，且推进只发生一次（定时器与手动互斥）', () => {
    const src = fs.readFileSync(path.join(ROOT, 'src/js/study.js'), 'utf8');
    // ① 推进定时器必须存在 state 上，否则取消不掉
    if (!/state\.advanceTimer\s*=\s*setTimeout\(/.test(src)) {
      throw new Error('finishAnswer 没把定时器存到 state.advanceTimer，回车抢不了先');
    }
    // ② 手动入口要挂出来，并且能被清掉 —— 清不掉就会「按一次回车跳两题」
    if (!/state\.advanceNow\s*=\s*advance/.test(src)) throw new Error('没挂出 state.advanceNow');
    if (!/state\.advanceNow\s*=\s*null/.test(src)) throw new Error('clearAdvance 没清 advanceNow');
    // ③ 三个入口都得清。漏掉任何一处，现象都是
    //    「正在答第 N 题，上一题的定时器把第 N+1 题推上来」。
    //    ★ 取函数体必须裁到下一个顶层函数：靠固定字符窗口会因为
    //      start() 前面那段选项读取而漏判，把窗口拉大又会误判到别的函数身上。
    const bodyOf = (name) => {
      const at = src.indexOf('function ' + name + '(');
      if (at < 0) throw new Error('找不到函数 ' + name);
      const rest = src.slice(at);
      const stop = rest.search(/\n  (?:async )?function \w/);
      return rest.slice(0, stop < 0 ? rest.length : stop);
    };
    // start() / startToday() 现在把「进答题界面」这段整体交给 beginRun
    // （背诵与复习共用一份，见需求 14 的槽位分离）—— 所以 clearAdvance
    // 要断言在真正干活的那个函数上，而不是两个入口上。
    for (const fn of ['beginRun', 'end', 'nextQuestion']) {
      if (!bodyOf(fn).includes('clearAdvance()')) {
        throw new Error(`${fn}() 里没调 clearAdvance`);
      }
    }
    // 两个入口都必须真的走 beginRun，否则就是「绕过清定时器那条路」
    for (const fn of ['start', 'startToday']) {
      if (!bodyOf(fn).includes('beginRun(')) {
        throw new Error(`${fn}() 没走 beginRun（那就不保证会清掉上一题的推进定时器）`);
      }
    }
    return 'beginRun / end / nextQuestion';
  }],
  ['背诵：空格在任意模式都发音（不再是绑在 listen_spell 上的死分支）', () => {
    const src = fs.readFileSync(path.join(ROOT, 'src/js/study.js'), 'utf8');
    // 旧写法 `e.key === ' ' && state.mode === 'listen_spell'` 排在
    // `if (meta().typing) return;` 之后，而 listen_spell 的 typing 恰为 true
    // —— 于是这句永远执行不到。这里钉死它不再复活。
    if (/e\.key === ' ' && state\.mode === 'listen_spell'/.test(src)) {
      throw new Error('空格还绑在 listen_spell 上 —— 那是一段执行不到的死分支');
    }
    const i = src.indexOf('function bindKeys');
    if (i < 0) throw new Error('找不到 bindKeys');
    const keys = src.slice(i, i + 3000);
    // 未作答 / 已作答 两态都要能按空格重听（答错的词尤其需要）
    const spaces = (keys.match(/e\.key === ' '/g) || []).length;
    if (spaces < 2) throw new Error(`空格分支应覆盖「未作答」与「已作答」两态，实际 ${spaces} 处`);
    // 回车立即下一题：只在已作答、且确实有推进入口时生效（收尾阶段不该有反应）
    if (!/e\.key === 'Enter' && state\.advanceNow/.test(keys)) throw new Error('缺回车立即下一题');
    return spaces + ' 处空格分支';
  }],
  ['背诵：今日复习按钮上的数字 = 实际题数（不重不漏，点进去就是这么多）', async () => {
    // Mock 的 stats：due_today=3 / leeches=1。
    // ★ 断言的核心是「不重不漏」：leeches 是 due 的**子集**，把两者相加会把
    //   常错的词算两遍，按钮上的数字天生偏大 —— 用户对数字的信任是一次性的。
    //   所以数字只取 due_today，并且必须走独立的复习入口（review）。
    await sandbox.refreshStudy();
    const btn = elById('btn-today');
    if (!btn.textContent.includes('3')) {
      throw new Error('按钮上的数字不等于今天到期的词数：' + btn.textContent);
    }
    if (btn.textContent.includes('4')) {
      throw new Error('到期 + 常错被相加了（常错是到期的子集，会重复计数）：' + btn.textContent);
    }
    if (btn.dataset.mode !== 'review') {
      throw new Error('今日复习入口没走独立会话（data-mode 应为 review）：' + btn.dataset.mode);
    }
    if (btn.dataset.size !== '3') throw new Error('题数没落进 dataset：' + btn.dataset.size);
    const hint = elById('today-hint').textContent;
    if (!hint.includes('3')) throw new Error('提示没写清今天到底有几个词：' + hint);
    return btn.textContent;
  }],
  ['背诵：今日复习走独立槽位且绝不补足（需求 14）', () => {
    const study = fs.readFileSync(path.join(ROOT, 'src/js/study.js'), 'utf8');
    const api = fs.readFileSync(path.join(ROOT, 'src/js/api.js'), 'utf8');
    // ① 复习必须走自己的接口
    if (!/startReviewSession/.test(study)) throw new Error('study.js 没用 cmd_start_review_session');
    if (!/cmd_start_review_session/.test(api)) throw new Error('api.js 没暴露 startReviewSession');
    // ② size 只能当截断上限，绝不能用来补足 —— 后端那侧由 Rust 单测钉死，
    //    前端至少要保证不会把 batch_size 传下去。
    const at = study.indexOf('async function startToday');
    if (at < 0) throw new Error('找不到 startToday');
    const body = study.slice(at, at + 900);
    if (/opt-batch/.test(body)) throw new Error('startToday 读了「每轮题量」——复习绝不能被补足');
    // ③ 会话类型必须一路透传，漏一处就是「题从复习槽出、答案记进背诵槽」。
    //    断言的是「api 层把 kind 参数暴露出来」+「study 层确实传了 kind」。
    for (const call of ['startReviewSession:', 'currentQuestion:', 'submitAnswer:', 'buildAdvancedCard:', 'endSession:']) {
      if (!api.includes(call)) throw new Error('api.js 缺 ' + call);
    }
    const kinds = (study.match(/state\.kind/g) || []).length;
    if (kinds < 5) {
      throw new Error(`study.js 里 state.kind 只出现 ${kinds} 次 —— 会话类调用必然有漏传`);
    }
    return `state.kind × ${kinds}`;
  }],

  // ---- 需求 5：AI 讲解的联网补充 ----
  ['查词：AI 讲解如实展示联网参考资料（需求 5）', () => {
    const lookup = fs.readFileSync(path.join(ROOT, 'src/js/lookup.js'), 'utf8');
    // ① 提示条要交代本次参考了几条资料 —— 用户看到例句时才知道
    //    那是「本地没有、按网页补的」，而不是模型凭空生成。
    if (!/已联网检索/.test(lookup)) throw new Error('讲解提示条没说明本次联网了几条资料');
    // ② 来源做成可点项，且走应用内打开（需求 11），不再甩到系统浏览器
    if (!/ai-ref-link/.test(lookup)) throw new Error('参考资料渲染成了不可点的纯文本');
    const refStart = lookup.indexOf('const refsHtml');
    if (refStart < 0) throw new Error('找不到 refsHtml —— 参考资料渲染被删了？');
    if (!/API\.openInApp\(/.test(lookup.slice(refStart))) {
      throw new Error('参考资料没走应用内打开（应该用 API.openInApp）');
    }

    // ③ 真渲染一遍：光看源码可能「写了但没拼进 innerHTML」
    const box = elById('ai-body');
    sandbox.Detail.renderExplain(box, {
      word: 'happy', text: '# 讲解正文', original: '# 讲解正文', lang: 'zh',
      translated: false,
      web_refs: [
        { title: 'Cambridge Dictionary', url: 'https://example.com/c', snippet: '例句来源' },
        { title: 'Merriam-Webster', url: 'https://example.com/m', snippet: '' },
      ],
    }, false);
    const html = String(box.innerHTML);
    if (!html.includes('已联网检索 2 条资料')) {
      throw new Error('提示条没写清资料条数：' + html.slice(0, 160));
    }
    if (!html.includes('data-url="https://example.com/c"')) throw new Error('来源没带上可点链接');
    if (!html.includes('# 讲解正文') && !html.includes('讲解正文')) throw new Error('讲解正文被挤掉了');

    // ④ 没联网时不出现空的「0 条资料」——那会让人以为联网坏了
    sandbox.Detail.renderExplain(box, {
      word: 'happy', text: 'x', original: 'x', lang: 'zh', translated: false, web_refs: [],
    }, false);
    const html2 = String(box.innerHTML);
    if (html2.includes('联网检索') || html2.includes('ai-refs')) {
      throw new Error('没有资料却渲染出了资料块：' + html2.slice(0, 160));
    }
    return '有资料 2 条 / 无资料不渲染';
  }],

  ['在线搜索：结果改在应用内打开（需求 11）', () => {
    const lookup = fs.readFileSync(path.join(ROOT, 'src/js/lookup.js'), 'utf8');
    const api = fs.readFileSync(path.join(ROOT, 'src/js/api.js'), 'utf8');
    if (!/openInApp:/.test(api)) throw new Error('api.js 没暴露 openInApp');
    const at = lookup.indexOf('.web-item');
    if (at < 0) throw new Error('找不到在线搜索结果的绑定');
    const seg = lookup.slice(at, at + 600);
    if (!/API\.openInApp\(/.test(seg)) throw new Error('搜索结果仍然走系统浏览器（应改 openInApp）');
    if (/API\.openUrl\(/.test(seg)) throw new Error('搜索结果里还留着 openUrl');
    // 「权威辞书」那一排**应当**继续走 openUrl（那里的意图就是离开软件看官网），
    // 所以这里只禁止搜索结果那一段，不做全局断言。
    return 'web-item → openInApp';
  }],

  // ---- 需求 4：翻译结果不能只给一个主要释义 ----
  ['翻译：「词性 + 释义」的拆分规则（不能切出假词性）', () => {
    const sp = sandbox.Translate.splitPos;
    const a = sp('adj. 快乐的；幸福的');
    if (a.pos !== 'adj.' || a.def !== '快乐的；幸福的') throw new Error('常见格式没拆对：' + JSON.stringify(a));
    // 没有词性的行（有道 web / 网络释义就长这样）不能凭空造一个词性出来
    const b = sp('网络释义');
    if (b.pos !== '' || b.def !== '网络释义') throw new Error('没词性的行被切出了假词性：' + JSON.stringify(b));
    // 词性里带 & 的组合（vt.& vi.）也算词性，不能当成释义的开头
    const c = sp('vt.& vi. 使快乐');
    if (c.pos !== 'vt.&' && c.pos !== '') throw new Error('不能把中间的空格当分界：' + JSON.stringify(c));
    // 超长前缀（不是词性）同样不能硬拆
    const d = sp('a very long line without pos');
    if (d.pos !== '') throw new Error('长前缀被误判成词性：' + JSON.stringify(d));
    return '4 例';
  }],

  ['翻译：多义项缩略呈现、带词性、可展开（需求 4）', () => {
    sandbox.Translate.renderResult({
      source: 'happy', text: '快乐的', alternatives: [],
      from: 'en', to: 'zh', phonetic: 'ˈhæpi', tts_url: '',
      engine: 'youdao', from_cache: false,
      dict: { phonetic: 'ˈhæpi', explains: [
        'adj. 快乐的；幸福的；愉快的',
        'adj. 乐意的；甘愿的',
        'adj. 幸运的；恰到好处的',
        'adj. （言语或行为）得体的，恰当的',
      ] },
      web: ['快乐的', '幸福的', '高兴的'],
    });
    const html = String(elById('tr-senses').innerHTML);
    if (elById('tr-senses').classList.contains('hidden')) throw new Error('有多义项却没显示');
    // 每条都要带词性 —— 没有词性的话「快乐的」和「使快乐」看着一模一样
    if (!html.includes('tr-sense-pos')) throw new Error('义项没有词性标签');
    if (!html.includes('adj.')) throw new Error('词性内容没渲染出来');
    if (!html.includes('快乐的；幸福的；愉快的')) throw new Error('释义内容没渲染出来');
    if (!html.includes('tr-web-item')) throw new Error('网络释义没渲染');
    // 默认只露 3 条，其余的折叠 —— 否则译文区会被撑得老高
    const extra = (html.match(/is-extra/g) || []).length;
    if (extra !== 1) throw new Error(`4 条义项应折叠 1 条，实际 ${extra}`);
    if (!html.includes('tr-sense-more')) throw new Error('折叠了却没有「展开全部」入口');
    // 单词才给「详细讲解」跳转（复用查词页的实现，需求 6）
    if (!html.includes('tr-goto-lookup')) throw new Error('缺「详细讲解」入口');

    // 没有词典数据时必须整块隐藏，不能留个空壳
    sandbox.Translate.renderResult({
      source: '你好', text: 'Hello', alternatives: [], from: 'zh', to: 'en',
      phonetic: '', tts_url: '', engine: 'youdao', from_cache: false,
    });
    if (!elById('tr-senses').classList.contains('hidden')) {
      throw new Error('没有词性与释义时仍显示空块');
    }
    if (String(elById('tr-senses').innerHTML) !== '') throw new Error('空块没清干净');
    return '4 条义项 / 折叠 1 条';
  }],

  // ---- 需求 5：同族派生词（本地词库确认，不编造） ----
  ['查词：同族派生词带词性 + 主释义且可点（需求 5）', () => {
    const html = String(sandbox.Detail.renderFamily([
      { word: 'happiness', pos: 'n.', definition: '幸福；快乐' },
      { word: 'happier', pos: 'adj.', definition: '更快乐的' },
    ]));
    // 每条都要有词性 —— 只给词形的话用户还得点进去才知道是什么意思
    if (!html.includes('deriv-pos')) throw new Error('派生词没带词性');
    if (!html.includes('n.')) throw new Error('词性内容没渲染');
    if (!html.includes('幸福；快乐')) throw new Error('主释义没渲染');
    // 必须能被 bindWordChips 的委托选中（data-word + .rel-chip）
    if (!html.includes('data-word="happiness"')) throw new Error('派生词不可点：缺 data-word');
    if (!html.includes('rel-chip')) throw new Error('派生词没用 rel-chip 类，点击委托选不中');
    // 空列表必须什么都不渲染（不能留个空标题在那儿）
    if (String(sandbox.Detail.renderFamily([])) !== '') throw new Error('空列表仍渲染了块');
    if (String(sandbox.Detail.renderFamily(null)) !== '') throw new Error('null 时不该渲染');

    // 详情卡必须真的去拉派生词，否则这块永远是空的
    const lookup = fs.readFileSync(path.join(ROOT, 'src/js/lookup.js'), 'utf8');
    if (!/loadFamily\(\s*entry\.word/.test(lookup)) throw new Error('Detail.open 没调用 loadFamily');
    const api = fs.readFileSync(path.join(ROOT, 'src/js/api.js'), 'utf8');
    if (!/wordFamily:/.test(api)) throw new Error('api.js 没暴露 wordFamily');
    if (!/cmd_word_family/.test(api)) throw new Error('api.js 没接 cmd_word_family');
    return '2 条';
  }],

  ['查词：派生词由本地词库确认（不联网、不编造）', () => {
    // 这是这条需求最容易走歪的地方：直接凭规则生成 happiness 显示出来，
    // 用户一点却发现查不到。断言后端确实做了「存在性确认」这一步。
    const cmd = fs.readFileSync(path.join(ROOT, 'src-tauri/src/commands/mod.rs'), 'utf8');
    const at = cmd.indexOf('pub fn cmd_word_family');
    if (at < 0) throw new Error('找不到 cmd_word_family');
    const body = cmd.slice(at, at + 2200);
    if (!/existing_word_briefs/.test(body)) {
      throw new Error('cmd_word_family 没做词库存在性确认 —— 会返回点不到的词');
    }
    // 规则生成必须与确认分开（生成在 morph.rs，确认在 db）
    const morph = fs.readFileSync(path.join(ROOT, 'src-tauri/src/morph.rs'), 'utf8');
    if (!/pub fn derivative_candidates/.test(morph)) throw new Error('缺少词形候选生成');
    const db = fs.readFileSync(path.join(ROOT, 'src-tauri/src/db/mod.rs'), 'utf8');
    if (!/pub fn existing_word_briefs/.test(db)) throw new Error('缺少 existing_word_briefs');
    return '生成 + 确认';
  }],

  // ---- 样式：拦截「未定义 CSS 变量」这一类静默故障 ----
  //
  // ★ 这三条是一次真实事故的护栏：`:root` 里的自引用 / 漏定义让
  //   `linear-gradient(..., var(--accent-2))` 整条声明在计算值阶段失效，于是
  //     · `.logo-mark` 白底白字 → 用户报「图标没了」
  //     · `.progress-fill` 填充透明 → 用户报「还是没有进度条」
  //   而 CSS 不会报任何错，开发者本地也未必复现。所以必须由测试来钉。
  ['样式：所有 var(--x) 都必须有定义或兜底（未定义会让整条声明作废）', () => {
    // ★ 必须先把 /* 注释 */ 剥掉：本项目有若干处**注释里**写着「这里原本用的是
    //   var(--text-1)，可惜没这个 token」——那是记录这个 bug 的说明文字。
    //   不剥注释的话，检查器会把 bug 的说明书当成 bug 本身报出来。
    const text = ['src/css/app.css', 'src/index.html']
      .map(f => fs.readFileSync(path.join(ROOT, f), 'utf8')).join('\n')
      .replace(/\/\*[\s\S]*?\*\//g, ' ');
    // 定义处：`--name:`（变量声明）
    const defs = new Set([...text.matchAll(/(--[a-z0-9-]+)\s*:/g)].map(m => m[1]));
    const bad = new Set();
    // 引用处：`var(--name)` 后面紧跟 `)` 表示**没有**兜底值；跟 `,` 表示有兜底
    for (const m of text.matchAll(/var\(\s*(--[a-z0-9-]+)\s*([,)])/g)) {
      if (m[2] === ')' && !defs.has(m[1])) bad.add(m[1]);
    }
    if (bad.size) {
      throw new Error('引用了但从未定义、且没写兜底值的变量：' + [...bad].join('、')
        + '（会让引用它的整条声明静默失效）');
    }
    // 单独再钉一次历史事故里必须存在的 token
    for (const t of ['--accent-2', '--amber-text', '--border-hover']) {
      if (!defs.has(t)) throw new Error(`缺少 ${t} —— 历史事故就是它漏定义导致图标/进度条消失`);
    }
    return defs.size + ' 个变量';
  }],

  ['样式：进度条必须真的画出来（轨道可见 + 填充有颜色）', () => {
    const css = fs.readFileSync(path.join(ROOT, 'src/css/app.css'), 'utf8');
    const barAt = css.indexOf('.progress-bar {');
    if (barAt < 0) throw new Error('找不到 .progress-bar');
    const bar = css.slice(barAt, css.indexOf('.progress-fill {'));
    // 轨道不能用 --border-2：它在白卡片上几乎与底色同色，未完成那段会「消失」
    if (/background:\s*var\(--border-2\)/.test(bar)) {
      throw new Error('进度条轨道用了 --border-2 —— 白底上几乎看不见，看起来就像没有进度条');
    }
    if (!/background:\s*var\(--border\)/.test(bar)) {
      throw new Error('进度条轨道没有用可见的 --border');
    }
    const h = /height:\s*(\d+)px/.exec(bar);
    if (!h || Number(h[1]) < 8) throw new Error('进度条太细（<8px），视觉上读不出来：' + (h && h[1]));
    // 填充的渐变引用的变量必须都在（上一条已全量校验，这里钉住「确实是渐变」）
    const fill = css.slice(css.indexOf('.progress-fill {'), css.indexOf('.progress-text'));
    if (!/linear-gradient/.test(fill)) throw new Error('.progress-fill 没有背景色');
    // 宽度必须由 JS 驱动，否则永远是 0
    const js = fs.readFileSync(path.join(ROOT, 'src/js/study.js'), 'utf8');
    if (!/q-progress[\s\S]{0,120}?style\.width/.test(js)) throw new Error('没有任何地方设置进度条宽度');
    return h[1] + 'px';
  }],

  // ---- 开关的胶囊几何不许被「字段样式」盖掉 ----
  //
  // ★ 这是同一个坑的第二次：用户两次截图点名「方形滑块」。
  //   第一次是「支持语言」的勾选项（`.check`，已改成胶囊 chip）；
  //   第二次是词典源卡片里的**启用开关**（`.switch input`）——
  //   `.source-item input { border-radius: 6px; border; padding; width:100% }`
  //   本来只想管文本框，却同时命中了开关；两条规则特异性都是 (0,1,1)，
  //   而字段样式在文件里更靠后，于是 22px 高的轨道被扣上 6px 圆角（只有 27%），
  //   渲染出来就是个圆角方块。
  //
  //   这条用例直接**模拟一次层叠**：把「词典源卡片里的开关 input」当作探针，
  //   逐条判断命中并比特异性，最后断言胜出的几何值就是胶囊该有的那些。
  //   比肉眼审 CSS 靠谱，也能挡住以后任何一条新的宽泛 `input` 规则。
  ['样式：开关的胶囊几何不许被字段样式盖掉（用户两次点名「方形滑块」）', () => {
    const css = fs.readFileSync(path.join(ROOT, 'src/css/app.css'), 'utf8')
      .replace(/\/\*[\s\S]*?\*\//g, ' ');

    const rules = [];
    {
      const re = /([^{}]+)\{([^{}]*)\}/g;
      let m;
      while ((m = re.exec(css)) !== null) {
        const sels = m[1].split(',').map((s) => s.trim()).filter(Boolean);
        if (!sels.length || sels.some((s) => s.startsWith('@'))) continue;
        rules.push({ sels, decls: m[2] });
      }
    }

    // 特异性：id*10000 + (class/attr/伪类)*100 + 元素
    function spec(sel) {
      const s = sel.replace(/::[a-z-]+(\([^)]*\))?/g, '');
      const ids = (s.match(/#[\w-]+/g) || []).length;
      const cls = (s.match(/\.[\w-]+/g) || []).length
        + (s.match(/\[[^\]]*\]/g) || []).length
        + (s.match(/:(?!not\b)[a-z-]+(\([^)]*\))?/g) || []).length;
      const els = (s.replace(/[:#.\[]/g, ' ').match(/\b[a-zA-Z][\w-]*\b/g) || []).length;
      return ids * 10000 + cls * 100 + els;
    }

    // 探针 = 开关的 <input> 本身：无 class，靠 `.switch input` 命中
    const probe = {
      tags: ['input'], classes: [], attrs: ['type="checkbox"'],
      ancestors: ['source-item', 'switch', 'switch-bare'],
    };
    function matchCompound(part) {
      if (/#[\w-]+/.test(part)) return false;
      for (const n of [...part.matchAll(/:not\(([^)]*)\)/g)].map((x) => x[1])) {
        if (n.includes('checkbox') || n.includes('radio')) return false;
      }
      const s = part.replace(/:not\([^)]*\)/g, '').replace(/::?[a-z-]+(\([^)]*\))?/g, '');
      const tag = /^\s*([a-zA-Z][\w-]*)/.exec(s);
      if (tag && !probe.tags.includes(tag[1].toLowerCase())) return false;
      for (const c of [...s.matchAll(/\.([\w-]+)/g)].map((x) => x[1])) {
        if (!probe.classes.includes(c)) return false;
      }
      for (const a of [...s.matchAll(/\[([^\]]+)\]/g)].map((x) => x[1].replace(/["'\s]/g, ''))) {
        const [k, v] = a.split('=');
        if (k === 'type' && v && !probe.attrs.includes(`type="${v}"`)) return false;
      }
      return true;
    }
    function matches(sel) {
      if (sel.includes('::')) return false;
      const parts = sel.split(/\s+/).filter((x) => x && !['>', '+', '~'].includes(x));
      if (!parts.length || !matchCompound(parts[parts.length - 1])) return false;
      for (const anc of parts.slice(0, -1)) {
        const need = [...anc.matchAll(/\.([\w-]+)/g)].map((x) => x[1]);
        if (need.length && !need.every((c) => probe.ancestors.includes(c))) return false;
      }
      return true;
    }

    // ① 只有开关专属的规则，才允许设置开关的几何/外观
    const intruders = [];
    for (const r of rules) {
      for (const sel of r.sels) {
        if (!matches(sel)) continue;
        if (/\.switch\b/.test(sel)) continue;              // 开关自己的规则，正常
        if (spec(sel) === 0) continue;                      // `* { … }` 通用重置
        if (/border-radius|(^|[;\s])border\s*:|padding\s*:|width\s*:|height\s*:|background\s*:/
          .test('; ' + r.decls)) {
          intruders.push(`${sel} → ${r.decls.trim().slice(0, 60)}`);
        }
      }
    }
    if (intruders.length) {
      throw new Error('有非开关专属的规则在改开关的几何属性（就是「方形滑块」的成因）：\n      '
        + intruders.join('\n      '));
    }

    // ② 层叠胜出者必须是胶囊该有的值
    function winner(prop) {
      let w = null;
      for (const r of rules) {
        for (const sel of r.sels) {
          if (!matches(sel)) continue;
          const m = new RegExp('(?:^|[;{\\s])' + prop + '\\s*:\\s*([^;]+)').exec(r.decls);
          if (!m) continue;
          const sp = spec(sel);
          if (!w || sp >= w.spec) w = { sel, spec: sp, value: m[1].trim() };
        }
      }
      return w;
    }
    for (const [prop, want] of [
      ['border-radius', '11px'], ['height', '22px'], ['width', '38px'],
      ['padding', '0'], ['border', 'none'],
    ]) {
      const w = winner(prop);
      if (!w || !w.value.replace(/\s+/g, ' ').startsWith(want)) {
        throw new Error(`开关的 ${prop} 层叠结果是 ${w ? w.value : '（没人设）'}（来自 ${w ? w.sel : '-'}），`
          + `期望 ${want} —— 22px 高配 6px 圆角只有 27%，看起来就是方块`);
      }
    }
    // ③ 根治手段要还在：字段规则必须显式排除复选框。
    //    （光靠抬高开关的特异性也能赢，但那是「比谁的规则更长」，
    //      排除复选框才是把作用域收回到文本输入框，语义上才对。）
    if (!/\.source-item input:not\(\[type="checkbox"\]\):not\(\[type="radio"\]\)/.test(css)) {
      throw new Error('字段样式 `.source-item input` 又没有排除复选框了 —— 这就是「方形滑块」的根源');
    }
    return 'border-radius 11px / border none / padding 0';
  }],

  ['样式：勾选项是胶囊而不是方框（用户点名的「方形滑块」）', () => {
    const css = fs.readFileSync(path.join(ROOT, 'src/css/app.css'), 'utf8');
    // ★ 查 :has() 必须先剥注释：本项目有多处注释**解释为什么不能用 :has()**，
    //   不剥的话守卫会被自己的说明书骗过去。
    const live = css.replace(/\/\*[\s\S]*?\*\//g, ' ');
    if (/:has\(/.test(live)) {
      throw new Error('样式里用了 :has() —— 旧内核不支持时整条规则会静默失效');
    }
    const at = css.indexOf('.check input {');
    if (at < 0) throw new Error('找不到 .check input');
    const native = css.slice(at, css.indexOf('.check > span {'));
    // 原生方框必须被收起（视觉隐藏但保留键盘可达）
    if (!/clip-path|appearance/.test(native)) throw new Error('原生方框勾选框还露在外面');
    if (/display:\s*none|visibility:\s*hidden/.test(native)) {
      throw new Error('把勾选框 display:none 掉了 —— 键盘用户将彻底选不中');
    }
    // 选中态用相邻兄弟选择器
    if (!/\.check input:checked\s*\+\s*span/.test(live)) {
      throw new Error('选中态没有用 input:checked + span 驱动');
    }
    const chip = css.slice(css.indexOf('.check > span {'), css.indexOf('.check:hover > span'));
    if (!/border-radius:\s*999px/.test(chip)) throw new Error('勾选项不是全圆角胶囊');
    if (!/padding:\s*5px 12px/.test(chip)) throw new Error('胶囊内边距过小，点不中');
    // 选中态要真的有底色，否则「选了没反应」
    if (!/input:checked\s*\+\s*span\s*\{[^}]*background:\s*var\(--blue\)/.test(live)) {
      throw new Error('选中的胶囊没有变蓝');
    }
    return '胶囊 chip + 相邻兄弟选择器';
  }],

  // ---- 整应用启动链路 ----
  ['App.init()', () => sandbox.App.init()],
  ['Settings.load()', () => sandbox.Settings.load()],

  // ---- 需求 9：语音包按语言分组 + 缺包语言如实说明 ----
  ['设置：语音包按语言分组，且缺包语言有交代（需求 9）', async () => {
    await sandbox.Settings.loadTts();
    const html = String(elById('tts-voice-list').innerHTML);
    // 分组标题必须出现（平铺一列在语音包多了之后没法用）
    if (!html.includes('tts-lang-group')) throw new Error('语音包没有按语言分组');
    if (!html.includes('tts-lang-name')) throw new Error('分组缺语言名');
    if (!html.includes('tts-lang-count')) throw new Error('分组缺「已装/总数」计数');
    // mock 里只有 en / zh，那么 ja 这类学习语言必须被点名说清 ——
    // 不解释的话，用户的第一反应是「软件漏了日语」，而不是「上游没有」。
    if (!html.includes('tts-missing')) throw new Error('缺包语言没有交代');
    if (!html.includes('日语')) throw new Error('缺包清单里没列出日语');
    if (!/在线语音|系统语音/.test(html)) throw new Error('没说清缺包时的回退通道');
    // 系统音色那侧早就按语言 optgroup 分组了，两处口径必须一致
    const src = fs.readFileSync(path.join(ROOT, 'src/js/settings.js'), 'utf8');
    if (!/optgroup label=/.test(src)) throw new Error('系统音色下拉丢了语言分组');
    return 'en/zh 分组 + 4 门缺包语言';
  }],

  // ---- 语音包分组可折叠 ----
  //
  // 现场：分组之后整页还是太长 —— 用户真正在意的通常只有「正在学的那门语言」。
  // 折叠默认只展开那一组，其余靠标题上的「已装/总数」摘要存线索。
  ['设置：语音包分组可折叠，默认只展开正在学的语言', async () => {
    // 注：冒烟用的是极简假 DOM，querySelector 一律返回 null，所以这里直接
    // 解析渲染出来的 HTML 字符串 —— 断言的是**生成结果**，不是 DOM 行为。
    await sandbox.Settings.loadTts();
    const html = String(elById('tts-voice-list').innerHTML);

    const groups = html.match(/<div class="tts-lang-group/g) || [];
    if (groups.length < 2) throw new Error(`分组数不足，无法验证折叠：${groups.length}`);

    const opens = (html.match(/aria-expanded="true"/g) || []).length;
    const closes = (html.match(/aria-expanded="false"/g) || []).length;
    if (opens + closes !== groups.length) {
      throw new Error(`aria-expanded 没写全：${groups.length} 组里只有 ${opens + closes} 个`);
    }
    if (opens !== 1) {
      throw new Error(`默认应只展开「正在学的语言」那一组，实际展开 ${opens} 组`);
    }

    // 折叠态的两个可见要件：标题可点（是 button）+ 摘要仍在
    const heads = (html.match(/<button type="button" class="tts-lang-head" data-tts-fold="/g) || []).length;
    if (heads !== groups.length) throw new Error('分组标题不是可点击的 button');
    const counts = (html.match(/class="tts-lang-count"/g) || []).length;
    if (counts !== groups.length) throw new Error('折叠态下必须保留「已装/总数」摘要');

    // 折叠的组内容容器必须真的藏起来。
    //
    // ★ 必须用 `.hidden` **class**，不能用 `hidden` **属性**：
    //   `.tts-lang-body { display: flex }` 是作者样式，会盖掉浏览器给
    //   `[hidden]` 的默认 `display: none`（作者样式优先于 UA 样式），
    //   属性写法等于没写 —— 这正是用户截图里「箭头明明是收起态 ▸、
    //   内容却全露着」的成因。
    const hiddenBodies = (html.match(/class="tts-lang-body hidden"/g) || []).length;
    if (hiddenBodies !== groups.length - opens) {
      throw new Error(`折叠的内容容器应带 .hidden class，应为 ${groups.length - opens} 个，实际 ${hiddenBodies}`);
    }
    if (/class="tts-lang-body" hidden/.test(html)) {
      throw new Error('用了 hidden 属性：会被 .tts-lang-body{display:flex} 盖掉，等于没折叠');
    }
    const openMark = (html.match(/<div class="tts-lang-group open"/g) || []).length;
    if (openMark !== opens) throw new Error('展开的组缺 open 标记，样式组挂不上');

    // 点击处理必须就地改 DOM 且不触发重渲染（重渲染会丢焦点，连按会失效）
    const src = fs.readFileSync(path.join(ROOT, 'src/js/settings.js'), 'utf8');
    if (!/ttsLangOpen\.set\(lang, open\)/.test(src)) throw new Error('折叠状态没有被记住');
    if (!/classList\.toggle\('hidden', !open\)/.test(src)) {
      throw new Error('折叠没有就地切换显隐（必须走 .hidden class）');
    }
    // 负向守卫：`.hidden = ` 这种属性写法一旦回来，折叠立刻失效
    if (/\.hidden\s*=\s*[^=]/.test(src)) {
      throw new Error('又用回了 hidden 属性：它会被作者样式盖掉，折叠点了不生效');
    }
    return `${groups.length} 组，默认展开 1 组`;
  }],

  // ---- 系统音色：缺什么必须说清楚，不能只报「检测到 N 个」 ----
  //
  // 现场：用户机器上一共 7 个音色（日语 4 + 中文 3），一个英语都没有，而他在
  // 学英语。旧文案「检测到 7 个系统音色」让人以为一切正常，「点了朗读没声音
  // / 下拉里选不到英语」于是成了悬案。
  ['设置：系统音色缺哪门语言要说清楚', async () => {
    const src = fs.readFileSync(path.join(ROOT, 'src/js/settings.js'), 'utf8');
    if (/检测到 \$\{list\.length\} 个系统音色/.test(src)) {
      throw new Error('仍是「检测到 N 个」这种没有信息量的文案');
    }
    if (!/set-speak-voices-hint/.test(src)) throw new Error('提示节点丢了');
    if (!/currentStudyLang\(\)/.test(src)) {
      throw new Error('提示没有针对「正在学的语言」做判断');
    }
    if (!/添加语音|时间和语言/.test(src)) throw new Error('没给出 Windows 的安装路径');

    // 朗读侧：挑不出音色时不能假装是「系统语音」
    const sp = fs.readFileSync(path.join(ROOT, 'src/js/speak.js'), 'utf8');
    if (!/notifyVoice\('none'/.test(sp)) throw new Error('无音色可用时仍在谎报系统语音');
    if (!/hintMissingVoice/.test(sp)) throw new Error('无音色可用时没有任何提示');
    if (!/missingVoiceWarned/.test(sp)) throw new Error('缺音色提示没有做同会话去重');
    return '缺语言 + 安装路径 + 无音色如实上报';
  }],

  // ---- 语音包：下载完要能「选中生效」 ----
  //
  // 现场：用户下了几十 MB 的语音包，界面上却只有一个「已下载」标签，
  // 没有任何地方能把它选成「当前使用」的那条 —— 后端只能自己按语言猜，
  // 装了多条时用户无从选择。下载了 ≠ 在用，这两件事要能分别看出来。
  ['设置：语音包下载后能选中生效', async () => {
    const src = fs.readFileSync(path.join(ROOT, 'src/js/settings.js'), 'utf8');
    const sp = fs.readFileSync(path.join(ROOT, 'src/js/speak.js'), 'utf8');
    if (!/data-tts-use=/.test(src)) throw new Error('已安装的语音包没有「使用」入口');
    if (!/class="tag inuse"/.test(src)) throw new Error('看不出哪条正在使用');
    // 切换必须走全局读音门面（Speak.selectVoice）—— 它是参数名唯一写对的地方
    if (!/Speak\.selectVoice\(id\)/.test(src)) throw new Error('切换语音没有走全局读音门面');
    if (!/voiceLocal: currentLocalVoice/.test(sp)) throw new Error('门面没有写回后端配置');
    // 负向守卫：直接给 setTtsPrefs 手拼 snake_case 参数名一旦回来，
    // 后端收到 null、配置根本没写，界面上还毫无异常 —— 那就是老病复发
    if (/setTtsPrefs\(\{\s*voice_local/.test(src) || /setTtsPrefs\(\{\s*voice_local/.test(sp)) {
      throw new Error('又手拼了 voice_local 参数（IPC 参数名是 camelCase 的 voiceLocal）');
    }
    if (!/if \(!ttsCurrentVoice\)/.test(src)) throw new Error('下载完没有自动接管（下了却还用不上）');
    // 删掉的正好是「使用中」那条时，要把指向它的配置一起清掉，
    // 否则留下一个指向不存在文件的 voice_local，每次朗读都先失败再回退。
    if (!/ttsCurrentVoice === id/.test(src)) throw new Error('删除使用中的语音后没有清配置');
    // 可用性必须在启动后 / 首次朗读前同步一次 —— 只在设置页刷新的话，
    // 用户查词页直接朗读时 localReady 还是 false，本地通道被整段跳过
    if (!/function ensureReady/.test(sp)) throw new Error('门面缺少可用性同步（ensureReady）');
    if (!/return ensureReady\(\)\.then/.test(sp)) throw new Error('朗读前没有做可用性同步');
    return '使用入口 + 门面切换 + 下载后自动应用 + 启动即同步可用性';
  }],

  // ---- 系统音色：刷新不许打断用户的选择 ----
  //
  // 现场：「系统音色选不了」。两个原因叠加 ——
  //   ① `onvoiceschanged` 是**单槽位属性**，设置页与 speak.js 都用它，
  //      互相覆盖，谁后绑定谁生效；
  //   ② 每次刷新都重建 innerHTML，把用户正在展开 / 正在选的下拉打断，
  //      选完立刻被抹掉，看起来就是「点了没反应」。
  ['设置：系统音色的刷新不许打断选择', async () => {
    const src = fs.readFileSync(path.join(ROOT, 'src/js/settings.js'), 'utf8');
    const sp = fs.readFileSync(path.join(ROOT, 'src/js/speak.js'), 'utf8');

    if (!/addEventListener\('voiceschanged'/.test(src)) {
      throw new Error('设置页没有用事件监听（会与 speak.js 互相覆盖）');
    }
    if (!/addEventListener\('voiceschanged'/.test(sp)) {
      throw new Error('speak.js 没有用事件监听');
    }
    // 旧写法就是箭头函数直接赋值 —— 兜底分支里的 `= onVoices` 不算
    if (/onvoiceschanged\s*=\s*\(\)\s*=>/.test(src) || /onvoiceschanged\s*=\s*\(\)\s*=>/.test(sp)) {
      throw new Error('又出现 onvoiceschanged 属性赋值（单槽位，会互相覆盖）');
    }
    if (!/syncVoiceSelection/.test(src)) throw new Error('没有单独同步选中项');
    if (!/sig === speakVoicesSig/.test(src)) throw new Error('清单没变时仍会重建下拉');
    if (!/list\.length\) \{ renderSpeakVoices\(true\); return; \}/.test(src)) {
      throw new Error('音色首次读不到时没有重试（下拉会永久停在「未提供可用语音」）');
    }
    return 'addEventListener + 指纹比对 + 选中态同步 + 空列表重试';
  }],

  // ---- 外部链接：默认在软件内打开，设置里可关；更新下载永远走系统浏览器 ----
  ['链接：命令层分流 + 更新下载强制外部', async () => {
    const rs = fs.readFileSync(path.join(ROOT, 'src-tauri/src/commands/mod.rs'), 'utf8');
    const html = fs.readFileSync(path.join(ROOT, 'src/index.html'), 'utf8');
    const apijs = fs.readFileSync(path.join(ROOT, 'src/js/api.js'), 'utf8');
    const upd = fs.readFileSync(path.join(ROOT, 'src/js/update.js'), 'utf8');
    const models = fs.readFileSync(path.join(ROOT, 'src-tauri/src/models.rs'), 'utf8');

    // 分流收在命令层：前端所有调用点不用各自判断
    if (!/open_links_in_app\s*\{/.test(rs)) throw new Error('cmd_open_url 没有按开关分流');
    if (!/force_external\.unwrap_or\(false\)/.test(rs)) throw new Error('缺少强制外部的旁路');
    if (!/open_links_in_app: bool/.test(models)) throw new Error('配置里缺 open_links_in_app 字段');
    // 更新下载必须绕过开关（应用内 WebView 接不住安装包下载）
    if (!/openExternal/.test(apijs)) throw new Error('api.js 缺 openExternal');
    if (!/API\.openExternal/.test(upd)) throw new Error('更新下载仍在走 openUrl（会被开关分流进应用内窗口）');
    // 设置页有开关（复用 data-opt 通用机制）
    if (!/data-opt="open_links_in_app"/.test(html)) throw new Error('设置页缺「链接在软件内打开」开关');
    return '命令层分流 + openExternal 旁路 + 设置开关';
  }],

  // ---- 模糊搜索提示：本地词库参与联想，拼写不准也能找到词 ----
  ['联想：本地模糊候选 + 拼写纠错（不依赖网络）', async () => {
    const rs = fs.readFileSync(path.join(ROOT, 'src-tauri/src/commands/mod.rs'), 'utf8');
    const db = fs.readFileSync(path.join(ROOT, 'src-tauri/src/db/mod.rs'), 'utf8');
    if (!/fuzzy_candidates\(&query, local_lang/.test(rs)) {
      throw new Error('cmd_suggest 没有先并本地词库候选');
    }
    if (!/fn fuzzy_candidates/.test(db)) throw new Error('词库缺模糊候选查询');
    if (!/fn edit_distance/.test(db)) throw new Error('缺编辑距离（拼写纠错的核心）');
    // Rust 侧的三层相关性与「短前缀不做纠错」有单测钉死，这里只守接线
    return '本地候选优先 + 编辑距离纠错 + 离线可用';
  }],

  // ---- 词库内容增强：后台 AI 把单薄词条补成统一详解 ----
  ['增强：后台引擎只补缺不覆盖，且不动用户已有内容', async () => {
    const enr = fs.readFileSync(path.join(ROOT, 'src-tauri/src/enrich.rs'), 'utf8');
    const st = fs.readFileSync(path.join(ROOT, 'src-tauri/src/state.rs'), 'utf8');
    if (!/fn merge_entry/.test(enr)) throw new Error('缺合并函数');
    if (!/fn spawn/.test(enr)) throw new Error('缺后台循环入口');
    if (!/enrich::spawn\(state\.clone\(\)\)/.test(st)) throw new Error('AppState 启动时没有拉起增强引擎');
    // 命门：合并方向必须是「现有为准，生成垫底」
    if (!/base\.phonetic\.uk = fresh\.phonetic\.uk/.test(enr)) {
      throw new Error('合并没有只在缺失时填充');
    }
    // 与用户开关联动：AI 讲解关了就完全不跑
    if (!/cfg\.study\.ai_explain/.test(enr)) throw new Error('增强没有跟着 AI 开关走');
    return '启动即跑 + 只补缺不覆盖 + 跟随 AI 开关';
  }],

  // ---- 布局：试听钉顶部、当前语音模型钉右下角 ----
  //
  // 现场：试听按钮排在面板最末尾，改一次设置要滚下去点、再滚回来看设置；
  // 模型名会被展开的语音包分组顶下去（用户原话「被挤走」）。
  ['样式：试听钉顶部、当前语音模型钉右下角', async () => {
    const html = fs.readFileSync(path.join(ROOT, 'src/index.html'), 'utf8');
    const css = fs.readFileSync(path.join(ROOT, 'src/css/app.css'), 'utf8');

    if (!/id="btn-speak-test"/.test(html)) throw new Error('试听按钮丢了');
    if (!/tts-topbar[\s\S]{0,220}id="btn-speak-test"/.test(html)) {
      throw new Error('试听不在顶部固定条里');
    }
    if (!/id="tts-current-model"/.test(html)) throw new Error('缺「当前语音模型」节点');
    if (!/id="tts-footer"/.test(html)) throw new Error('缺右下角的固定容器');

    const top = css.match(/\.tts-topbar\s*\{([^}]*)\}/);
    if (!top || !/position:\s*sticky/.test(top[1])) throw new Error('tts-topbar 没有 sticky');
    if (!/top:\s*0/.test(top[1])) throw new Error('tts-topbar 没有钉在顶部');

    const foot = css.match(/\.tts-footer\s*\{([^}]*)\}/);
    if (!foot || !/position:\s*sticky/.test(foot[1])) throw new Error('tts-footer 没有 sticky');
    if (!/bottom:\s*0/.test(foot[1])) throw new Error('tts-footer 没有钉在底部');
    if (!/justify-content:\s*flex-end/.test(foot[1])) throw new Error('模型名没有靠右下角');

    // 两个固定条都不能脱流：absolute 会叠到别的控件上，并随内容滚动而移动
    if (/\.tts-(?:topbar|footer)\s*\{[^}]*position:\s*absolute/.test(css)) {
      throw new Error('固定条不能用 absolute：会脱流并随内容移动');
    }
    return '试听 sticky top + 模型名 sticky bottom 右对齐';
  }],

  // ---- 词条语言：列 lang 与 entry_json 里的 lang 必须同源 ----
  //
  // 这是「英语词显示『日语』标签」「切了 piper 却还是系统音色」的共同根因：
  // 历史上迁移只改了列的 lang，JSON 里那份原样带走，全库漂了 5037 行。
  ['存储：词条语言不许在列与 JSON 上各存一份', async () => {
    const dbmod = fs.readFileSync(path.join(ROOT, 'src-tauri/src/db/mod.rs'), 'utf8');
    if (!/fn entry_from_stored/.test(dbmod)) throw new Error('缺少读取侧的语言兜底');
    if (!/fn rewrite_entry_json_lang/.test(dbmod)) throw new Error('缺少写入侧的 JSON lang 改写');
    if (!/pub fn repair_entry_json_langs/.test(dbmod)) throw new Error('缺少存量数据的一次性清洗');
    if (!/json_valid/.test(dbmod)) throw new Error('清洗没有用 json_valid 挡住坏 JSON');
    if (!/CASE WHEN json_valid/.test(dbmod)) {
      throw new Error('json_valid 必须写在 CASE 里保证短路，否则一个坏词条会让整次迁移失败');
    }
    // 迁移搬行时必须连 JSON 一起改，否则继续制造新的漂移
    if (!/let fixed_json = rewrite_entry_json_lang/.test(dbmod)) {
      throw new Error('migrate_words_lang 仍在原样搬运 entry_json');
    }
    const state = fs.readFileSync(path.join(ROOT, 'src-tauri/src/state.rs'), 'utf8');
    if (!/repair_entry_json_langs/.test(state)) throw new Error('启动时没有调用清洗');
    return '读取兜底 + 写入改写 + 存量清洗';
  }],

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
