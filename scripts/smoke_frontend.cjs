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
  ['发音：详情卡 🔊 带上词条数据（静态结构取不到词的老问题）', async () => {
    await sandbox.Detail.open(entry);
    const b = elById('dc-audio');
    if (b.dataset.speakWord !== 'abandon') throw new Error('dc-audio 没挂上词：' + b.dataset.speakWord);
    if (!b.dataset.speakLang) throw new Error('dc-audio 没挂上语言');
    if (!b.dataset.speakAudio && b.dataset.speakAudio !== '') throw new Error('dc-audio 的音频字段缺失');
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
    for (const fn of ['start', 'end', 'nextQuestion']) {
      if (!bodyOf(fn).includes('clearAdvance()')) {
        throw new Error(`${fn}() 里没调 clearAdvance`);
      }
    }
    return 'start / end / nextQuestion';
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
  ['背诵：今日复习入口把「到期 + 常错」合成一个数字（进站即背，不用自己加）', async () => {
    // Mock 的 stats：due_today=3 / leeches=1 → 今天该背 4 个
    await sandbox.refreshStudy();
    const btn = elById('btn-today');
    if (!btn.textContent.includes('4')) {
      throw new Error('按钮上没写今天该背的总数：' + btn.textContent);
    }
    if (btn.dataset.todo !== '4') throw new Error('todo 该落进 dataset，实际 ' + btn.dataset.todo);
    const hint = elById('today-hint').textContent;
    if (!hint.includes('到期 3') || !hint.includes('常错 1')) {
      throw new Error('提示没把两类词拆开说清楚：' + hint);
    }
    return btn.textContent;
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
