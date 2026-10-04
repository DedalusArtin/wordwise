/* ============================================================
   ui.js —— 通用界面工具
   词条渲染、提示、标签页、Markdown 轻解析等
   ============================================================ */

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
  parts.push('<div class="word-entry">');

  // 头部：词 + 音标 + 来源标签
  parts.push('<div class="we-head">');
  parts.push(`<div class="we-word">${esc(entry.word)}</div>`);

  if (o.showPhonetic) {
    const ph = entry.phonetic || {};
    const items = [];
    if (ph.uk) items.push(`<span><span class="phon-tag">英</span>${esc(ph.uk)}</span>`);
    if (ph.us) items.push(`<span><span class="phon-tag">美</span>${esc(ph.us)}</span>`);
    if (items.length) parts.push(`<div class="we-phon">${items.join('')}</div>`);
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
      parts.push(`<div class="infl-item"><span class="infl-label">${esc(i.label || '形式')}</span><span class="infl-form">${esc(i.form)}</span></div>`);
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
    parts.push('<div class="we-section">');
    parts.push('<div class="we-section-title">相关词</div>');
    parts.push('<div class="rel-list">');
    for (const r of entry.related) {
      parts.push(`<span class="rel-chip" data-word="${esc(r)}">${esc(r)}</span>`);
    }
    parts.push('</div></div>');
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
 * 极简 Markdown 渲染（覆盖 AI 讲解常用的语法）。
 * 支持：# 标题、- 列表、**粗体**、`代码`、段落。
 */
function renderMarkdown(md) {
  if (!md) return '';
  const lines = String(md).split(/\r?\n/);
  const out = [];
  let inList = false;

  const inline = (s) => esc(s)
    .replace(/\*\*(.+?)\*\*/g, '<strong>$1</strong>')
    .replace(/`([^`]+)`/g, '<code>$1</code>')
    .replace(/\*(.+?)\*/g, '<em>$1</em>');

  const closeList = () => { if (inList) { out.push('</ul>'); inList = false; } };

  for (let raw of lines) {
    const line = raw.trimEnd();
    if (!line.trim()) { closeList(); continue; }

    // 标题
    const h = line.match(/^(#{1,6})\s+(.*)$/);
    if (h) {
      closeList();
      const lvl = Math.min(h[1].length + 1, 4);
      out.push(`<h${lvl}>${inline(h[2])}</h${lvl}>`);
      continue;
    }

    // 无序列表
    const li = line.match(/^\s*[-*•]\s+(.*)$/);
    if (li) {
      if (!inList) { out.push('<ul>'); inList = true; }
      out.push(`<li>${inline(li[1])}</li>`);
      continue;
    }

    // 有序列表
    const ol = line.match(/^\s*\d+[.)]\s+(.*)$/);
    if (ol) {
      if (!inList) { out.push('<ul>'); inList = true; }
      out.push(`<li>${inline(ol[1])}</li>`);
      continue;
    }

    closeList();
    out.push(`<p>${inline(line)}</p>`);
  }
  closeList();
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

/** 防抖。 */
function debounce(fn, wait = 260) {
  let t = null;
  return (...args) => {
    clearTimeout(t);
    t = setTimeout(() => fn(...args), wait);
  };
}

window.WW = window.WW || {};
Object.assign(window.WW, {
  esc, toast, loadingHtml, renderEntry, collectExamples, sourceLabel, langLabel,
  renderMarkdown, fmtDay, timeAgo, masteryClass, renderBarChart, switchDetailTab, debounce,
});
