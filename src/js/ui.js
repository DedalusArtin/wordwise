/* ============================================================
   ui.js —— 通用界面工具
   词条渲染、提示、标签页、Markdown 轻解析等
   ============================================================ */

/**
 * 发音按钮 HTML。
 *
 * ★ 必须是**顶层函数声明**，不能只挂 `window.WW.speakBtn`：
 *   `renderEntry()` 里用的是裸名字 `speakBtn(...)`，而裸名字只会往
 *   全局作用域找（`window.speakBtn`），找不到 `window.WW.speakBtn` 这个属性。
 *   只挂 WW 的话，只要 `showPhonetic !== false`（默认开启）就必然
 *   `ReferenceError: speakBtn is not defined`，结果就是查词永远转圈。
 *   speak.js 尚未加载时降级为空字符串。
 */
function speakBtn(entry, accent, label) {
  return window.Speak ? window.Speak.btnHtml(entry, accent, label) : '';
}

/**
 * 音标 HTML。全项目**所有**显示音标的位置都走它。
 *
 * 抽出来的三个理由：
 *   1. 标注必须跟着**词条语言**走 —— 中文词的拼音、日语词的假名读音、
 *      韩语词的罗马音，都不该被标成「英 / 美」；
 *   2. 只有拉丁字母的读音才用斜杠括起来，拼音加斜杠反而像乱码；
 *   3. IPA 有自己的一套字符（ˈ ɡ ʃ ʊ θ），必须用覆盖得住的字体栈，
 *      否则在部分机器上会掉成方框。字体在 CSS 的 --font-ipa 里统一配。
 *
 * @param {object} entry 词条（读 entry.phonetic 与 entry.lang）
 * @param {object} [opts] { size: 'sm'，speak: false 可去掉每条读音旁的小喇叭 }
 */
function phoneticHtml(entry, opts) {
  const o = opts || {};
  const ph = (entry && entry.phonetic) || {};
  const lg = String((entry && entry.lang) || 'en').toLowerCase().split(/[-_]/)[0];
  const withSpeak = o.speak !== false;

  const pairs = [];
  if (lg === 'zh') {
    const py = ph.uk || ph.us;
    if (py) pairs.push({ tag: '拼音', ipa: py, accent: 'us' });
  } else if (lg === 'ja') {
    const rb = ph.uk || ph.us;
    if (rb) pairs.push({ tag: '读音', ipa: rb, accent: 'us' });
  } else if (lg === 'ko') {
    const rb = ph.uk || ph.us;
    if (rb) pairs.push({ tag: '罗马音', ipa: rb, accent: 'us' });
  } else {
    if (ph.uk) pairs.push({ tag: '英', ipa: ph.uk, accent: 'uk' });
    if (ph.us) pairs.push({ tag: '美', ipa: ph.us, accent: 'us' });
  }
  // 一个都没有时不留一个空壳，交给调用方决定要不要放个纯发音按钮
  if (!pairs.length) return '';

  const items = pairs.map((p) => {
    const raw = String(p.ipa == null ? '' : p.ipa).trim().replace(/^\/+|\/+$/g, '');
    if (!raw) return '';
    // 斜杠是「音标」的符号。中文的拼音、日语的假名读音、韩语的罗马音都不是音标，
    // 加斜杠反而像乱码 —— 所以按**词条语言**判断，而不是按字符集猜。
    const slashed = !['zh', 'ja', 'ko'].includes(lg);
    const ipa = slashed ? `/${esc(raw)}/` : esc(raw);
    return `<span class="phon-item">
      <i class="phon-tag">${esc(p.tag)}</i>
      <span class="phon-ipa">${ipa}</span>
      ${withSpeak ? speakBtn(entry, p.accent, p.tag + '发音') : ''}
    </span>`;
  }).join('');

  if (!items) return '';
  return `<span class="phon${o.size === 'sm' ? ' phon-sm' : ''}">${items}</span>`;
}

/** HTML 转义，防止词条内容破坏结构。 */
function esc(s) {
  if (s === null || s === undefined) return '';
  return String(s)
    .replace(/&/g, '&amp;')
    .replace(/</g, '&lt;')
    .replace(/>/g, '&gt;')
    .replace(/"/g, '&quot;')
    .replace(/'/g, '&#39;');
}

/**
 * 把 related 拆成一个个真正可查的词。
 *
 * 后端 `split_related()` 已经拆过一遍，但**旧缓存里存的是拆之前的数据**
 * （dict_cache TTL 30 天），所以前端必须再兜一层 —— 否则老用户点相关词
 * 仍然是拿 "desert, abandon, leave" 整串去查，结果显示「查询失败」。
 */
function splitRelated(list) {
  const out = [];
  const seps = /[,;，；、/|\n\r\t·]+/;
  const edge = /^[.。,，()\[\]"']+|[.。,，()\[\]"']+$/g;
  const clean = (s) => String(s).trim().replace(edge, '');
  for (const raw of (list || [])) {
    if (typeof raw !== 'string') continue;
    for (const piece of raw.split(seps)) {
      const p = clean(piece);
      if (!p) continue;
      // CJK 串里的空格常是词组的一部分，保留原样；拉丁串按空格再拆
      const words = /[\u3040-\u30ff\u4e00-\u9fff\uac00-\ud7af]/.test(p) ? [p] : p.split(/\s+/);
      for (const raw2 of words) {
        const w = clean(raw2);
        if (!w || w.length > 30) continue;
        if (!out.includes(w)) out.push(w);
      }
    }
  }
  return out.slice(0, 20);
}

/** 全局轻提示。 */
let toastTimer = null;
function toast(msg, type = '') {
  const el = document.getElementById('toast');
  if (!el) return;
  el.textContent = msg;
  el.className = 'toast ' + type;
  el.classList.remove('hidden');
  clearTimeout(toastTimer);
  toastTimer = setTimeout(() => el.classList.add('hidden'), type === 'err' ? 5200 : 2600);
}

/** 加载中占位。 */
function loadingHtml(text = '加载中…') {
  return `<div class="loading"><div class="spinner"></div><span>${esc(text)}</span></div>`;
}

/**
 * 渲染词条。
 *
 * 两种排布，共用同一份头部与同一份义项渲染：
 *  - 默认（`tabs: false`）：头部 + 各块**纵向堆叠**。对应词卡片用它 ——
 *    那里的词头已经画在卡片标题上了，再套一排标签只是让用户多点一次。
 *  - `tabs: true`：头部 + **分块卡片 + 底部标签切换**（查词结果页用它）。
 *    参照有道词典：大词头，音标紧随其下，标签栏压在头部之下、内容之上，
 *    一次只呈现一块 —— 长词条就不会被撑成一条读不完的竖条。
 *
 * @param {object} entry  WordEntry
 * @param {object} opts   { showInflections, showExamples, showRelated, showMnemonic,
 *                          showPhonetic, compactHead, tabs }
 */
function renderEntry(entry, opts = {}) {
  const o = {
    showInflections: true,
    showExamples: true,
    showRelated: false,
    showMnemonic: true,
    showPhonetic: true,
    // 头部只画音标与来源标签、不重复画大词头（双向词条的对应卡片用它，
    // 因为词头已经画在卡片标题上了，再来一遍就是加倍冗余）
    compactHead: false,
    // 分块 + 底部标签切换（见函数头说明）
    tabs: false,
    ...opts,
  };
  if (!entry) return '<div class="empty-state"><p>无内容</p></div>';

  /* ---------- 头部：大词头 + 音标成对 + 来源标签 ---------- */
  const head = [];
  head.push('<div class="we-head">');
  if (!o.compactHead) {
    head.push(`<div class="we-word">${esc(entry.word)}</div>`);
  }

  if (o.showPhonetic) {
    // 统一走 phoneticHtml：语言标注、斜杠规则、IPA 字体都只有一份实现。
    // 词条一个音标都没有时，至少留一个纯发音按钮，别整块消失。
    const html = phoneticHtml(entry, { speak: true });
    head.push(`<div class="we-phon">${html || speakBtn(entry, 'us', '发音')}</div>`);
  }

  const srcs = (entry.source || '').split('+').filter(Boolean);
  const tags = srcs.map(s => `<span class="tag">${esc(sourceLabel(s))}</span>`);
  if (entry.lang && entry.lang !== 'en') {
    tags.unshift(`<span class="tag blue">${esc(langLabel(entry.lang))}</span>`);
  }
  if (tags.length) head.push(`<div class="we-src">${tags.join('')}</div>`);
  head.push('</div>');

  /* ---------- 正文：先攒成「块」，再决定怎么摆 ----------
     攒成块而不是边写边拼，是因为堆叠与标签两种排布对同一份内容有不同的
     外壳需求。分两趟走，义项渲染就只有一份实现，版式不会分叉。 */
  const blocks = [];
  const addBlock = (id, label, inner) => {
    if (inner) blocks.push({ id, label, inner });
  };

  // 释义：按「这段释文本身是哪种语言写的」分组。
  //
  // 为什么要分组而不是混着画：同一个英语词，有道给中文释义、
  // freedictionaryapi 给英英释义，两条都要留下（用户原话——「仿照有道词典
  // 两者都有，不然我输入英文的时候没有英文解释对吧」）。但混进一条列表里
  // 读起来是「中文、英文、中文…」来回跳。有道词典的做法是拆成「释义」与
  // 「英英释义」两块，这里沿用：母语组在前（用户一眼能读），原文组在后。
  const senses = entry.senses || [];
  const [localSenses, nativeSenses] = splitSensesByScript(senses);

  if (senses.length) {
    const inner = [];
    if (localSenses.length && nativeSenses.length) {
      // 两组都有才需要小标题区分；只有一组时「释义」这个标题由块本身带着，
      // 再来一遍就是重复（有道也是只在多来源时才标区分）。
      inner.push(senseGroup(null, localSenses, o));
      // 原文组的小标题跟着词条语言走：「乌鸦」的原文组本身就是中文，
      // 标题写成「中文释义」既奇怪又和上一块重复
      const title = langLabel(entry.lang || '') === '中文'
        ? '参考释义' : `${langLabel(entry.lang || '')}释义`;
      inner.push(senseGroup(title, nativeSenses, o));
    } else if (localSenses.length) {
      inner.push(senseGroup(null, localSenses, o));
    } else {
      inner.push(senseGroup(null, nativeSenses, o));
    }
    addBlock('def', '释义', inner.join(''));
  } else {
    addBlock('def', '释义', '<div class="muted">暂无释义，可点击「AI 讲解」让本地模型生成。</div>');
  }

  // 变形
  if (o.showInflections && entry.inflections && entry.inflections.length) {
    const inner = ['<div class="infl-grid">'];
    for (const i of entry.inflections) {
      inner.push(`<div class="infl-item"><span class="infl-label">${esc(i.label || '形式')}</span><span class="infl-form clickable" data-word="${esc(i.form)}" title="点击查询 ${esc(i.form)}">${esc(i.form)}</span></div>`);
    }
    inner.push('</div>');
    addBlock('infl', '变形', inner.join(''));
  }

  // 记忆法
  if (o.showMnemonic && entry.mnemonic) {
    addBlock('mnemonic', '记忆法', `<div class="mnemonic-box">${esc(entry.mnemonic)}</div>`);
  }

  // 相关词
  if (o.showRelated && entry.related && entry.related.length) {
    const rels = splitRelated(entry.related);
    if (rels.length) {
      addBlock('related', '相关词',
        '<div class="rel-list">' + rels
          .map(r => `<span class="rel-chip" data-word="${esc(r)}">${esc(r)}</span>`)
          .join('') + '</div>');
    }
  }

  /* ---------- 组装 ---------- */
  const body = o.tabs
    ? tabbedBlocks(blocks)
    : blocks.map(b => `<div class="we-section"><div class="we-section-title">${esc(b.label)}</div>${b.inner}</div>`).join('');

  return `<div class="word-entry${o.tabs ? ' tabbed' : ''}" data-entry-word="${esc(entry.word)}">`
    + head.join('') + body + '</div>';
}

/**
 * 分块卡片 + 底部标签切换。
 *
 * 标签栏夹在头部与内容之间（头部之下、正文之上）—— 用户要的是「先把词头
 * 看全，再按需切到想看的那一块」。面板用 `display:none` 切换而不是重渲染，
 * 这样切标签不会丢掉已经绑好的发音按钮委托，也不会闪。
 *
 * 第一块默认选中：查词的人十有八九是来看释义的，多一次点击就是白费。
 */
function tabbedBlocks(blocks) {
  if (!blocks.length) return '';
  const tabs = blocks.map((b, i) => `<button class="entry-tab${i === 0 ? ' active' : ''}"`
    + ` data-block="${esc(b.id)}" type="button" role="tab">${esc(b.label)}</button>`).join('');
  const panes = blocks.map((b, i) => `<div class="entry-pane${i === 0 ? ' active' : ''}"`
    + ` data-pane="${esc(b.id)}">${b.inner}</div>`).join('');
  return `<div class="entry-tabs" role="tablist">${tabs}</div>`
    + `<div class="entry-body">${panes}</div>`;
}

/** 一段释义里有没有汉字 —— 用来区分「母语释义」与「原文释义」。 */
function hasHan(s) {
  return /[\u3400-\u4dbf\u4e00-\u9fff\uf900-\ufaff]/.test(s || '');
}

/**
 * 把义项按「释文写成的语言」分成两组。
 *
 * @returns {[Array, Array]} `[母语写成的一组, 原文写成的一组]`，任一组可能为空
 */
function splitSensesByScript(senses) {
  const local = [];
  const native = [];
  for (const s of (senses || [])) {
    (hasHan(s.definition) ? local : native).push(s);
  }
  return [local, native];
}

/**
 * 画一组义项。
 *
 * 返回的是**块内容**，外层的卡片/区块由 `renderEntry` 决定 —— 堆叠排布与
 * 标签排布共用这一份实现，才不会出现「同一个词在两处长得不一样」。
 *
 * `title` 传空时不出小标题：只有一个来源的释义时，「释义」这个标题由块自己
 * 带着（标签模式下就是标签本身），再来一遍是重复。
 */
function senseGroup(title, senses, opts = {}) {
  const parts = ['<div class="sense-group">'];
  if (title) parts.push(`<div class="we-section-title">${esc(title)}</div>`);
  for (const s of senses) {
    parts.push('<div class="sense">');
    if (s.pos) parts.push(`<div class="sense-pos">${esc(s.pos)}</div>`);
    parts.push('<div class="sense-def">');
    parts.push(esc(s.definition));
    if (opts.showExamples !== false && s.examples && s.examples.length) {
      for (const ex of s.examples.slice(0, 3)) {
        parts.push('<div class="sense-ex">');
        parts.push(esc(ex.text));
        if (ex.translation) parts.push(`<div class="ex-zh">${esc(ex.translation)}</div>`);
        parts.push('</div>');
      }
    }
    parts.push('</div></div>');
  }
  parts.push('</div>');
  return parts.join('');
}

/**
 * 渲染「双向词条」的另一半：目标语言侧的对应词及其完整词条。
 *
 * 用户需求原文：「这里我是中文转英文，应该下面详细介绍的是 crow 或者
 * 其他能表示乌鸦的单词」「仿照有道词典两者都有，还有其他语言也要类似」。
 *
 * 所以「乌鸦」的主卡片下面是 crow / rook / raven 这几个候选译法各自的
 * 完整英文词条（英文音标、英文释义、词形变化），而不只是一串词。
 *
 * @param {Array} pairs [{ word, lang, entry, via }]
 */
function renderPairs(pairs, opts = {}) {
  const list = (pairs || []).filter(p => p && p.entry);
  if (!list.length) return '';

  const parts = ['<div class="pair-block">'];
  parts.push('<div class="pair-block-title">对应词汇</div>');
  parts.push(`<div class="pair-hint muted">下面是在${esc(langLabel(list[0].lang))}里表示这个词的说法，点击词头可单独查询</div>`);
  for (const p of list) {
    parts.push('<div class="pair-card">');
    parts.push('<div class="pair-card-head">');
    parts.push(`<span class="pair-word clickable" data-word="${esc(p.word)}">${esc(p.word)}</span>`);
    // 词头旁直接给一个 🔊：查到一个生词，第一件想做的事就是听它怎么读。
    // 发音按钮不需要额外绑事件 —— 结果区整块挂了委托（U().speakBind(box)），
    // 按钮自带 data-speak-* 就能取到词。
    parts.push(speakBtn(p.entry, 'us', '发音'));
    parts.push(`<span class="tag blue">${esc(langLabel(p.lang))}</span>`);
    parts.push(p.via === 'translation'
      ? '<span class="tag ok">主译</span>'
      : '<span class="tag">其他译法</span>');
    parts.push('</div>');
    parts.push(renderEntry(p.entry, {
      compactHead: true,
      showInflections: opts.showInflections !== false,
      showExamples: opts.showExamples !== false,
      showRelated: false,
      showMnemonic: false,
      showPhonetic: opts.showPhonetic !== false,
    }));
    parts.push('</div>');
  }
  parts.push('</div>');
  return parts.join('');
}

/** 收集词条中所有例句，供「例句」标签页使用。 */
function collectExamples(entry) {
  const out = [];
  for (const s of (entry.senses || [])) {
    for (const ex of (s.examples || [])) {
      if (ex.text) out.push({ pos: s.pos, ...ex });
    }
  }
  return out;
}

/** 来源 id → 中文名 */
function sourceLabel(id) {
  const map = {
    'free-dictionary': 'Free Dictionary',
    'wiktionary': 'Wiktionary',
    'youdao-suggest': '有道',
    // 下面两个和有道的 suggest 接口同域名，但来源 id 不同。
    // 不补进映射表的话界面上会直接露出 `youdao-newhh` 这种 id ——
    // 用户看到的是「中文 youdao-newhh」，完全读不懂。
    'youdao-jsonapi': '有道释义',
    'youdao-newhh': '现代汉语规范词典',
    'libre-translate': 'LibreTranslate',
    'lmstudio': '本地大模型',
    'builtin-seed': '内置词库',
  };
  return map[id] || id;
}

/** 语言代码 → 中文名 */
function langLabel(code) {
  const map = {
    en: '英语', ja: '日语', ko: '韩语', fr: '法语', de: '德语',
    es: '西班牙语', ru: '俄语', it: '意大利语', pt: '葡萄牙语',
    zh: '中文', ar: '阿拉伯语', hi: '印地语',
  };
  return map[code] || code;
}

/**
 * 规范 Markdown 渲染（AI 讲解专用，仿有道词典讲解版式）。
 *
 * 相比旧版新增：
 *   - 表格（| a | b | 含表头分隔行）
 *   - 多级嵌套列表（按缩进判定层级）
 *   - 引用块 >、分割线 ---
 *   - 代码块 ```（保留原样，不转义内部）
 *   - 软换行：段内单行换行渲染为 <br>
 *   - 行内：**粗体**、*斜体*、`代码`、[文字](链接)、~~删除线~~
 *   - 中文标点规范化：把模型误输出的 Markdown 符号吃掉，避免「符号乱飘」
 */
function renderMarkdown(md) {
  if (!md) return '';
  let src = String(md).replace(/\r\n?/g, '\n');

  // 去掉模型偶发包裹的整段 ```markdown / ``` 外壳
  src = src.replace(/^\s*```[a-zA-Z]*\s*\n/, '').replace(/\n```\s*$/, '');

  const lines = src.split('\n');
  const out = [];

  // ---------- 行内解析 ----------
  const inline = (raw) => {
    let s = esc(raw);
    // 代码（先占位，避免内部被后续规则改写）
    const codes = [];
    s = s.replace(/`([^`]+)`/g, (m, c) => {
      codes.push(c);
      return '\u0000C' + (codes.length - 1) + '\u0000';
    });
    // 图片 ![alt](url) → 链接样式
    s = s.replace(/!\[([^\]]*)\]\(([^)\s]+)\)/g,
      '<a href="$2" target="_blank" rel="noreferrer">$1</a>');
    // 链接 [文字](url)
    s = s.replace(/\[([^\]]+)\]\(([^)\s]+)\)/g,
      '<a href="$2" target="_blank" rel="noreferrer">$1</a>');
    // 粗体 + 斜体
    s = s.replace(/\*\*\*(.+?)\*\*\*/g, '<strong><em>$1</em></strong>');
    s = s.replace(/\*\*(.+?)\*\*/g, '<strong>$1</strong>');
    s = s.replace(/(^|[^*])\*([^*\n]+?)\*(?!\*)/g, '$1<em>$2</em>');
    s = s.replace(/__(.+?)__/g, '<strong>$1</strong>');
    // 删除线
    s = s.replace(/~~(.+?)~~/g, '<del>$1</del>');
    // 还代码
    s = s.replace(/\u0000C(\d+)\u0000/g, (m, i) => '<code>' + codes[+i] + '</code>');
    return s;
  };

  // ---------- 表格检测 ----------
  const isTableRow = (l) => /^\s*\|.*\|\s*$/.test(l);
  const isTableSep = (l) => /^\s*\|?[\s:|-]+\|[\s:|-]*$/.test(l) && l.includes('-');

  const parseTableRow = (l) =>
    l.trim().replace(/^\|/, '').replace(/\|$/, '').split('|').map(c => c.trim());

  // ---------- 列表栈（支持嵌套） ----------
  // 栈元素：{ indent, tag }
  const listStack = [];
  const closeLists = (toIndent = -1) => {
    while (listStack.length && listStack[listStack.length - 1].indent > toIndent) {
      out.push('</li></' + listStack.pop().tag + '>');
    }
  };
  const closeAllLists = () => { closeLists(-1); };

  for (let i = 0; i < lines.length; i++) {
    const raw = lines[i];
    const line = raw.replace(/\s+$/, '');

    // 空行
    if (!line.trim()) { closeAllLists(); continue; }

    // 代码块 ```
    const fence = line.match(/^\s*```(.*)$/);
    if (fence) {
      closeAllLists();
      const buf = [];
      i++;
      while (i < lines.length && !/^\s*```/.test(lines[i])) { buf.push(lines[i]); i++; }
      out.push('<pre class="md-pre"><code>' + esc(buf.join('\n')) + '</code></pre>');
      continue;
    }

    // 分割线
    if (/^\s*([-*_])\s*\1\s*\1[\s\1]*$/.test(line)) {
      closeAllLists();
      out.push('<hr class="md-hr">');
      continue;
    }

    // 标题
    const h = line.match(/^\s*(#{1,6})\s+(.*)$/);
    if (h) {
      closeAllLists();
      const lvl = Math.min(h[1].length + 1, 5);
      out.push(`<h${lvl} class="md-h">${inline(h[2].replace(/#+\s*$/, ''))}</h${lvl}>`);
      continue;
    }

    // 引用块
    if (/^\s*>\s?/.test(line)) {
      closeAllLists();
      const buf = [];
      while (i < lines.length && /^\s*>\s?/.test(lines[i])) {
        buf.push(lines[i].replace(/^\s*>\s?/, ''));
        i++;
      }
      i--;
      out.push('<blockquote class="md-quote">' + inline(buf.join(' ')) + '</blockquote>');
      continue;
    }

    // 表格
    if (isTableRow(line) && i + 1 < lines.length && isTableSep(lines[i + 1])) {
      closeAllLists();
      const head = parseTableRow(line);
      i += 2; // 跳过表头与分隔行
      const rows = [];
      while (i < lines.length && isTableRow(lines[i])) {
        rows.push(parseTableRow(lines[i]));
        i++;
      }
      i--;
      let t = '<div class="md-table-wrap"><table class="md-table"><thead><tr>';
      t += head.map(c => '<th>' + inline(c) + '</th>').join('');
      t += '</tr></thead><tbody>';
      for (const r of rows) {
        t += '<tr>';
        for (let c = 0; c < head.length; c++) {
          t += '<td>' + inline(r[c] === undefined ? '' : r[c]) + '</td>';
        }
        t += '</tr>';
      }
      t += '</tbody></table></div>';
      out.push(t);
      continue;
    }

    // 列表（无序 / 有序，按缩进分层）
    const ul = raw.match(/^(\s*)[-*•]\s+(.*)$/);
    const ol = raw.match(/^(\s*)(\d+)[.)]\s+(.*)$/);
    if (ul || ol) {
      const indent = Math.floor((ul ? ul[1] : ol[1]).length / 2);
      const tag = ol ? 'ol' : 'ul';
      const text = ul ? ul[2] : ol[3];

      // 回退到同级或更浅层级
      closeLists(indent);
      const top = listStack[listStack.length - 1];
      if (!top || top.indent < indent) {
        // 开新层（含类型切换）
        if (top && top.indent === indent && top.tag !== tag) {
          out.push('</li></' + listStack.pop().tag + '>');
        }
        out.push('<' + tag + ' class="md-list">');
        listStack.push({ indent, tag });
      } else if (top.indent === indent && top.tag !== tag) {
        out.push('</li></' + listStack.pop().tag + '>');
        out.push('<' + tag + ' class="md-list">');
        listStack.push({ indent, tag });
      } else {
        out.push('</li>'); // 同级下一条
      }
      out.push('<li>' + inline(text));
      continue;
    }

    // 普通段落：连续行软换行 → <br>
    closeAllLists();
    const buf = [line];
    while (
      i + 1 < lines.length &&
      lines[i + 1].trim() &&
      !/^\s*(#{1,6}\s|[-*•]\s|\d+[.)]\s|>|\||```)/.test(lines[i + 1]) &&
      !isTableSep(lines[i + 1])
    ) {
      buf.push(lines[i + 1].replace(/\s+$/, ''));
      i++;
    }
    out.push('<p class="md-p">' + buf.map(inline).join('<br>') + '</p>');
  }

  closeAllLists();
  return out.join('');
}

/** 格式化日期为「今天/明天/月-日」。 */
function fmtDay(dateStr) {
  const today = new Date();
  const d = new Date(dateStr + 'T00:00:00');
  const diff = Math.round((d - new Date(today.toDateString())) / 86400000);
  if (diff === 0) return '今天';
  if (diff === 1) return '明天';
  if (diff === 2) return '后天';
  if (diff === -1) return '昨天';
  return `${d.getMonth() + 1}月${d.getDate()}日`;
}

/** 相对时间：刚刚 / 5 分钟前 / 3 天前。 */
function timeAgo(ts) {
  if (!ts) return '—';
  const diff = Math.floor(Date.now() / 1000) - ts;
  if (diff < 60) return '刚刚';
  if (diff < 3600) return `${Math.floor(diff / 60)} 分钟前`;
  if (diff < 86400) return `${Math.floor(diff / 3600)} 小时前`;
  return `${Math.floor(diff / 86400)} 天前`;
}

/** 熟练度颜色分级。 */
function masteryClass(v) {
  if (v >= 75) return 'high';
  if (v < 40) return 'low';
  return '';
}

/** 渲染柱状图（近 N 天答题量）。 */
function renderBarChart(container, history) {
  if (!container) return;
  if (!history || !history.length) {
    container.innerHTML = '<div class="muted" style="padding:40px;text-align:center">暂无数据</div>';
    return;
  }
  const max = Math.max(1, ...history.map(h => h.count));
  container.innerHTML = history.map(h => {
    const total = h.count || 0;
    const ok = Math.min(h.correct || 0, total);
    const bad = Math.max(0, total - ok);
    const hOk = total ? Math.round((ok / max) * 108) : 0;
    const hBad = total ? Math.round((bad / max) * 108) : 0;
    const label = h.date ? h.date.slice(5) : '';
    return `<div class="bar-col" title="${esc(h.date)}：${total} 题（对 ${ok} / 错 ${bad}）">
      <div class="bar-stack">
        ${hBad ? `<div class="bar-bad" style="height:${hBad}px"></div>` : ''}
        ${hOk ? `<div class="bar-ok" style="height:${hOk}px"></div>` : ''}
      </div>
      <div class="bar-lbl">${esc(label)}</div>
    </div>`;
  }).join('');
}

/** 侧边栏/主界面的词条详情标签页切换。 */
function switchDetailTab(name) {
  document.querySelectorAll('#dc-tabs .dc-tab').forEach(t => {
    t.classList.toggle('active', t.dataset.tab === name);
  });
  document.querySelectorAll('.dc-pane').forEach(p => {
    p.classList.toggle('active', p.dataset.pane === name);
  });
}

/**
 * 事件目标是否处于「正在打字」的控件里。
 *
 * 用途：全局快捷键（document 级的 keydown）必须先问一句这个，否则
 * 用户在搜索框里输入时按键会被快捷键吞掉——表现为「输入框打不了字、
 * 退格也没用」。这是之前背诵页 A~H / 1~8 快捷键把搜索框吃掉的原因。
 */
function isTypingTarget(t) {
  if (!t || t.nodeType !== 1) return false;
  const tag = (t.tagName || '').toUpperCase();
  if (tag === 'TEXTAREA' || tag === 'SELECT') return true;
  if (tag === 'INPUT') {
    const type = (t.getAttribute('type') || 'text').toLowerCase();
    // 按钮类 input 不参与文本输入
    return !['button', 'submit', 'reset', 'checkbox', 'radio', 'file', 'image', 'range', 'color'].includes(type);
  }
  return !!(t.isContentEditable || t.getAttribute('contenteditable') === 'true');
}

/** 防抖。 */
function debounce(fn, wait = 260) {
  let t = null;
  return (...args) => {
    clearTimeout(t);
    t = setTimeout(() => fn(...args), wait);
  };
}

/**
 * 给一个「已渲染的列表」接上关键词搜索。
 *
 * 采用纯前端过滤：只切换行的 display，不重新请求后端、不重建 DOM，
 * 因此行上已经绑定好的点击事件（开详情卡、移出错词本…）全部保留；
 * 列表被重新渲染后调用返回的 apply() 即可重新套用当前关键词。
 *
 * @param {object} o
 * @param {string|HTMLElement} o.input          搜索输入框
 * @param {string|HTMLElement} o.list           列表容器
 * @param {string} [o.row='.word-row']          参与过滤的行选择器
 * @param {string|HTMLElement} [o.counter]      显示「匹配 N 条」的元素
 * @param {object} [o.empty]                    空状态文案 { icon, title, hint }
 * @param {number} [o.wait=260]                 实时搜索防抖毫秒数
 * @param {Function} [o.onApply](matched, kw)   每次过滤完成后的回调
 * @returns {{apply:Function,set:Function,clear:Function,keyword:string}}
 */
function attachListSearch(o) {
  const input = typeof o.input === 'string' ? document.getElementById(o.input) : o.input;
  const list = typeof o.list === 'string' ? document.getElementById(o.list) : o.list;
  const noop = { apply() {}, set() {}, clear() {}, keyword: '' };
  if (!input || !list) return noop;

  const rowSel = o.row || '.word-row';
  const wait = o.wait || 260;
  const counter = typeof o.counter === 'string' ? document.getElementById(o.counter) : (o.counter || null);
  const cfg = Object.assign({
    icon: '&#128269;',
    title: '没有匹配的结果',
    hint: '换个关键词试试，或清空搜索框查看全部',
  }, o.empty || {});

  let kw = '';
  let timer = null;

  /** 一行的可搜索文本：优先用 data-word / data-name，再兜底整行文本（含释义、标签）。 */
  function haystack(row) {
    const d = row.dataset || {};
    return ((d.word || '') + ' ' + (d.name || '') + ' ' + (row.textContent || '')).toLowerCase();
  }

  function emptyEl() {
    let el = list.querySelector(':scope > .search-empty');
    if (!el) {
      el = document.createElement('div');
      el.className = 'empty-state search-empty hidden';
      el.innerHTML =
        `<div class="es-icon">${cfg.icon}</div>` +
        `<p>${esc(cfg.title)}</p>` +
        `<p class="muted">${esc(cfg.hint)}</p>`;
      list.appendChild(el);
    }
    return el;
  }

  function apply() {
    const rows = Array.prototype.slice.call(list.querySelectorAll(rowSel));
    // 列表本来就是空的（页面自己已渲染空状态），不要叠加「没有匹配的结果」
    if (!rows.length) return;

    if (!kw) {
      rows.forEach(r => { r.style.display = ''; });
      emptyEl().classList.add('hidden');
      if (counter) counter.textContent = '';
      if (o.onApply) o.onApply(rows.length, '');
      return;
    }

    let n = 0;
    rows.forEach(r => {
      const hit = haystack(r).indexOf(kw) >= 0;
      r.style.display = hit ? '' : 'none';
      if (hit) n++;
    });
    emptyEl().classList.toggle('hidden', n > 0);
    if (counter) counter.textContent = n ? `匹配 ${n} 条` : '';
    if (o.onApply) o.onApply(n, kw);
  }

  function run() {
    kw = (input.value || '').trim().toLowerCase();
    apply();
  }

  input.addEventListener('input', () => {
    clearTimeout(timer);
    timer = setTimeout(run, wait);
  });
  input.addEventListener('keydown', (e) => {
    if (e.key === 'Enter') {          // 回车立即生效，不等防抖
      e.preventDefault();
      clearTimeout(timer);
      run();
    } else if (e.key === 'Escape') {  // Esc 清空
      clearTimeout(timer);
      input.value = '';
      run();
    }
  });

  return {
    apply,
    set(v) { input.value = v || ''; clearTimeout(timer); run(); },
    clear() { this.set(''); },
    get keyword() { return kw; },
  };
}

/**
 * 把**纯文本**渲染成 HTML：转义 + 保留段落与换行。
 *
 * 译文不是 Markdown（模型/接口返回的就是平文），直接用 renderMarkdown 会把
 * 译文里的 `#`、`*`、`-` 当成语法，反而破坏内容。段落翻译还需要保留原文的
 * 分行结构，所以按空行切段、段内换行转 <br>。
 */
function renderPlainText(s) {
  const t = String(s == null ? '' : s).replace(/\r\n?/g, '\n').trim();
  if (!t) return '';
  return t
    .split(/\n{2,}/)
    .map(p => `<p>${esc(p).replace(/\n/g, '<br>')}</p>`)
    .join('');
}

window.WW = window.WW || {};
Object.assign(window.WW, {
  esc, toast, loadingHtml, renderEntry, collectExamples, sourceLabel, langLabel,
  splitRelated, phoneticHtml, renderPairs, senseGroup, splitSensesByScript, hasHan,
  renderMarkdown, renderPlainText, fmtDay, timeAgo, masteryClass, renderBarChart, switchDetailTab, debounce,
  attachListSearch, speakBtn, isTypingTarget,
  speak: (word, opts) => (window.Speak ? window.Speak.speak(word, opts) : null),
  speakBind: (root) => { if (window.Speak) window.Speak.bindDelegate(root); },
});

/* 全局兜底：任何 .rel-chip / .infl-form.clickable 点击都能查词，
   即使某条渲染路径忘了单独绑定事件。
   使用捕获阶段 + 已处理标记，避免与局部绑定重复触发。 */
document.addEventListener('click', (e) => {
  const chip = e.target.closest('.rel-chip[data-word], .infl-form.clickable[data-word]');
  if (!chip || chip.dataset.wqHandled === '1') return;
  chip.dataset.wqHandled = '1';
  setTimeout(() => { delete chip.dataset.wqHandled; }, 0);
  const w = chip.dataset.word;
  if (!w) return;
  e.stopPropagation();
  if (window.Lookup && window.Lookup.query) window.Lookup.query(w);
}, true);
