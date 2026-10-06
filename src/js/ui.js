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
 * @param {object} entry  WordEntry
 * @param {object} opts   { compact, showInflections, showExamples, showRelated, showMnemonic, showPhonetic, hideExtra }
 */
function renderEntry(entry, opts = {}) {
  const o = {
    compact: false,
    showInflections: true,
    showExamples: true,
    showRelated: false,
    showMnemonic: true,
    showPhonetic: true,
    ...opts,
  };
  if (!entry) return '<div class="empty-state"><p>无内容</p></div>';

  const parts = [];
  parts.push(`<div class="word-entry" data-entry-word="${esc(entry.word)}">`);

  // 头部：词 + 音标 + 来源标签
  parts.push('<div class="we-head">');
  parts.push(`<div class="we-word">${esc(entry.word)}</div>`);

  if (o.showPhonetic) {
    // 统一走 phoneticHtml：语言标注、斜杠规则、IPA 字体都只有一份实现。
    // 词条一个音标都没有时，至少留一个纯发音按钮，别整块消失。
    const html = phoneticHtml(entry, { speak: true });
    parts.push(`<div class="we-phon">${html || speakBtn(entry, 'us', '发音')}</div>`);
  }

  const srcs = (entry.source || '').split('+').filter(Boolean);
  const tags = srcs.map(s => `<span class="tag">${esc(sourceLabel(s))}</span>`);
  if (entry.lang && entry.lang !== 'en') {
    tags.unshift(`<span class="tag blue">${esc(langLabel(entry.lang))}</span>`);
  }
  if (tags.length) parts.push(`<div class="we-src">${tags.join('')}</div>`);
  parts.push('</div>');

  // 释义
  const senses = entry.senses || [];
  if (senses.length) {
    parts.push('<div class="we-section">');
    parts.push('<div class="we-section-title">释义</div>');
    for (const s of senses) {
      parts.push('<div class="sense">');
      if (s.pos) parts.push(`<div class="sense-pos">${esc(s.pos)}</div>`);
      parts.push('<div class="sense-def">');
      parts.push(esc(s.definition));
      if (o.showExamples && s.examples && s.examples.length) {
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
  } else {
    parts.push('<div class="we-section"><div class="muted">暂无释义，可点击「AI 讲解」让本地模型生成。</div></div>');
  }

  // 变形
  if (o.showInflections && entry.inflections && entry.inflections.length) {
    parts.push('<div class="we-section">');
    parts.push('<div class="we-section-title">单词变形</div>');
    parts.push('<div class="infl-grid">');
    for (const i of entry.inflections) {
      parts.push(`<div class="infl-item"><span class="infl-label">${esc(i.label || '形式')}</span><span class="infl-form clickable" data-word="${esc(i.form)}" title="点击查询 ${esc(i.form)}">${esc(i.form)}</span></div>`);
    }
    parts.push('</div></div>');
  }

  // 记忆法
  if (o.showMnemonic && entry.mnemonic) {
    parts.push('<div class="we-section">');
    parts.push('<div class="we-section-title">记忆法</div>');
    parts.push(`<div class="mnemonic-box">${esc(entry.mnemonic)}</div>`);
    parts.push('</div>');
  }

  // 相关词
  if (o.showRelated && entry.related && entry.related.length) {
    const rels = splitRelated(entry.related);
    if (rels.length) {
      parts.push('<div class="we-section">');
      parts.push('<div class="we-section-title">相关词</div>');
      parts.push('<div class="rel-list">');
      for (const r of rels) {
        parts.push(`<span class="rel-chip" data-word="${esc(r)}">${esc(r)}</span>`);
      }
      parts.push('</div></div>');
    }
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
  splitRelated, phoneticHtml,
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
