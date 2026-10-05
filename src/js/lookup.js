/* ============================================================
   lookup.js —— 查词与 AI 讲解（需求 2、6）
   - 联网多源聚合查词，带联想
   - AI 流式讲解（打字机效果）
   - 单词详情卡（仿有道词典）
   ============================================================ */

/* ---------------- 辞书跳转链接（Detail 与 Lookup 共用） ---------------- */

/**
 * 把「权威辞书」跳转按钮渲染进指定容器。
 *
 * ★ 必须是**模块顶层函数**，不能放进 Detail 或 Lookup 任意一个 IIFE 里：
 *   两边都在用它（详情卡底部 `dc-dictlinks`、查词页 `lk-dictlinks`）。
 *   放进其中一个闭包，另一处调用就会
 *   `ReferenceError: loadDictLinksInline is not defined`。
 */
async function loadDictLinksInline(containerId, word, lang) {
  const { API } = window.WordWiseAPI;
  const U = window.WW;
  const box = document.getElementById(containerId);
  if (!box) return;
  try {
    const links = await API.dictLinks(word, lang);
    if (!links || !links.length) { box.innerHTML = ''; return; }
    box.innerHTML =
      '<div class="we-section-title">权威辞书</div>' +
      '<div class="dict-links">' +
      links.map(l =>
        `<button class="dict-link${l.cn_friendly ? ' cn' : ''}" data-url="${U.esc(l.url)}" title="${U.esc(l.note)}">
          <span class="dl-name">${U.esc(l.name)}</span>
          <span class="dl-note">${U.esc(l.note)}</span>
        </button>`).join('') +
      '</div>';
    box.querySelectorAll('.dict-link').forEach(b => {
      b.addEventListener('click', async () => {
        try { await API.openUrl(b.dataset.url); }
        catch (e) { U.toast('打开链接失败：' + e.message, 'err'); }
      });
    });
  } catch (e) {
    box.innerHTML = '';
  }
}

/* ---------------- 详情卡 ---------------- */

const Detail = (() => {
  const { API } = window.WordWiseAPI;
  const U = () => window.WW;

  let current = null;
  let explainBuffer = '';
  let unlistenDelta = null;

  async function open(entry, opts = {}) {
    current = entry;
    const overlay = document.getElementById('detail-overlay');
    overlay.classList.remove('hidden');

    document.getElementById('dc-word').textContent = entry.word || '';

    // 音标（带英/美发音按钮）
    const ph = entry.phonetic || {};
    const phItems = [];
    if (ph.uk) {
      phItems.push(`<span class="phon-item"><span class="phon-tag">英</span>${U().esc(ph.uk)}${U().speakBtn(entry, 'uk', '英音')}</span>`);
    }
    if (ph.us) {
      phItems.push(`<span class="phon-item"><span class="phon-tag">美</span>${U().esc(ph.us)}${U().speakBtn(entry, 'us', '美音')}</span>`);
    }
    // 没有任何音标也允许朗读（TTS 兜底）
    if (!phItems.length) {
      phItems.push(`<span class="phon-item">${U().speakBtn(entry, 'us', '发音')}</span>`);
    }
    document.getElementById('dc-phonetic').innerHTML = phItems.join('');

    // 标签
    const tags = [];
    if (opts.reason) tags.push(`<span class="tag orange">${U().esc(opts.reason)}</span>`);
    (entry.source || '').split('+').filter(Boolean).forEach(s => {
      tags.push(`<span class="tag">${U().esc(U().sourceLabel(s))}</span>`);
    });
    if (entry.lang && entry.lang !== 'en') {
      tags.push(`<span class="tag blue">${U().esc(U().langLabel(entry.lang))}</span>`);
    }
    document.getElementById('dc-tags').innerHTML = tags.join('');

    // 依赖配置的显示开关（需求 3）
    let study = {};
    try {
      const cfg = await API.getConfig();
      study = (cfg && cfg.study) || {};
    } catch (e) { /* 用默认 */ }

    // 各标签页内容
    document.getElementById('dc-pane-def').innerHTML = renderDefs(entry, study);
    document.getElementById('dc-pane-infl').innerHTML = renderInfl(entry);
    document.getElementById('dc-pane-ex').innerHTML = renderEx(entry);
    document.getElementById('dc-pane-rel').innerHTML = renderRel(entry);
    document.getElementById('dc-pane-ai').innerHTML =
      '<p class="muted">点击下方「AI 讲解」按钮，让本地大模型讲解这个单词。</p>';

    // 辞书链接（详情卡底部）
    loadDictLinksInline('dc-dictlinks', entry.word, entry.lang);

    // 学习状态
    loadState(entry.word);

    // 默认选中第一个有内容的标签
    const first = pickFirstTab(entry, study);
    U().switchDetailTab(first);
  }

  function pickFirstTab(entry, study) {
    if (entry.senses && entry.senses.length) return 'def';
    if (study.show_inflections !== false && entry.inflections && entry.inflections.length) return 'infl';
    return 'def';
  }

  function renderDefs(entry, study) {
    if (!entry.senses || !entry.senses.length) {
      return '<div class="muted">暂无释义。可点击「AI 讲解」让本地模型生成完整词条。</div>';
    }
    const showEx = study.show_examples !== false;
    const showMn = study.show_mnemonic !== false;
    let html = '';
    for (const s of entry.senses) {
      html += '<div class="sense">';
      if (s.pos) html += `<div class="sense-pos">${U().esc(s.pos)}</div>`;
      html += `<div class="sense-def">${U().esc(s.definition)}`;
      if (showEx && s.examples && s.examples.length) {
        for (const ex of s.examples.slice(0, 3)) {
          html += `<div class="sense-ex">${U().esc(ex.text)}`;
          if (ex.translation) html += `<div class="ex-zh">${U().esc(ex.translation)}</div>`;
          html += '</div>';
        }
      }
      html += '</div></div>';
    }
    if (showMn && entry.mnemonic) {
      html += `<div class="we-section"><div class="we-section-title">记忆法</div>
        <div class="mnemonic-box">${U().esc(entry.mnemonic)}</div></div>`;
    }
    return html;
  }

  function renderInfl(entry) {
    if (!entry.inflections || !entry.inflections.length) {
      return '<div class="muted">该词暂无变形信息。可在设置中开启后重新联网查询。</div>';
    }
    return '<div class="infl-grid">' + entry.inflections.map(i =>
      `<div class="infl-item"><span class="infl-label">${U().esc(i.label || '形式')}</span>
       <span class="infl-form clickable" data-word="${U().esc(i.form)}" title="点击查询 ${U().esc(i.form)}">${U().esc(i.form)}</span></div>`).join('') + '</div>';
  }

  function renderEx(entry) {
    const exs = U().collectExamples(entry);
    if (!exs.length) return '<div class="muted">暂无例句。</div>';
    return exs.map(ex => `
      <div class="sense" style="padding:10px 0">
        <div class="sense-pos">${U().esc(ex.pos || '')}</div>
        <div class="sense-def">
          ${U().esc(ex.text)}
          ${ex.translation ? `<div class="ex-zh" style="margin-top:4px">${U().esc(ex.translation)}</div>` : ''}
        </div>
      </div>`).join('');
  }

  function renderRel(entry) {
    if (!entry.related || !entry.related.length) return '<div class="muted">暂无相关词。</div>';
    return '<div class="rel-list">' + entry.related.map(r =>
      `<span class="rel-chip" data-word="${U().esc(r)}">${U().esc(r)}</span>`).join('') + '</div>';
  }

  async function loadState(word) {
    const el = document.getElementById('dc-state');
    try {
      const st = await API.wordState(word);
      if (!st) {
        el.innerHTML = '<span class="strong">尚未学习</span>';
        return;
      }
      const s = st.state;
      const acc = (s.correct_count + s.wrong_count) > 0
        ? Math.round((s.correct_count / (s.correct_count + s.wrong_count)) * 100)
        : 0;
      el.innerHTML = `<span class="strong">熟练度 ${s.mastery}%</span> · 正确率 ${acc}% ·
        下次复习 ${U().esc(st.due_text)}${s.is_leech ? ' · <span style="color:#f5a623">强化记忆</span>' : ''}`;
    } catch (e) {
      el.innerHTML = '';
    }
  }

  function close() {
    document.getElementById('detail-overlay').classList.add('hidden');
    if (unlistenDelta) { try { unlistenDelta(); } catch (e) {} unlistenDelta = null; }
  }

  /* 事件绑定 */
  function bind() {
    document.getElementById('dc-close')?.addEventListener('click', close);
    document.getElementById('detail-overlay')?.addEventListener('click', (e) => {
      if (e.target.id === 'detail-overlay') close();
    });
    document.addEventListener('keydown', (e) => {
      // 正在输入框里按 Esc 时不要顺手把详情卡也关了
      if (U().isTypingTarget(e.target)) return;
      if (e.key === 'Escape') close();
    });

    document.querySelectorAll('#dc-tabs .dc-tab').forEach(t => {
      t.addEventListener('click', () => U().switchDetailTab(t.dataset.tab));
    });

    // 相关词点击 → 直接查词
    document.getElementById('dc-pane-rel')?.addEventListener('click', (e) => {
      const chip = e.target.closest('.rel-chip');
      if (chip && chip.dataset.word) Lookup.query(chip.dataset.word);
    });

    // 变形点击 → 查询该变形词
    document.getElementById('dc-pane-infl')?.addEventListener('click', (e) => {
      const f = e.target.closest('.infl-form.clickable');
      if (f && f.dataset.word) Lookup.query(f.dataset.word);
    });

    // 例句中的词点击（可选，双击才触发，避免误触）
    document.getElementById('dc-pane-ex')?.addEventListener('dblclick', (e) => {
      const sel = window.getSelection && window.getSelection();
      const picked = sel ? String(sel).trim() : '';
      if (picked && /^[a-zA-Z][a-zA-Z'-]*$/.test(picked)) Lookup.query(picked.toLowerCase());
    });

    document.getElementById('dc-add')?.addEventListener('click', async () => {
      if (!current) return;
      try {
        await API.addWord(current);
        U().toast(`已把「${current.word}」加入词库`, 'ok');
      } catch (e) {
        U().toast(e.message, 'err');
      }
    });

    document.getElementById('dc-explain')?.addEventListener('click', () => {
      if (!current) return;
      U().switchDetailTab('ai');
      explainInto('dc-pane-ai', current.word);
    });
  }

  /* ---- AI 讲解语言：结果缓存与「查看原文」对照 ---- */

  /** 每个容器最近一次讲解结果。切换语言 / 查看原文都要用到。 */
  const explainCache = {};

  /**
   * 渲染一次讲解结果。
   *
   * 顶部固定一条语言提示条：
   * - 说明本次用的讲解语言、是否经过自动翻译、失败提示；
   * - 译文与原文不一致时给出「查看原文 / 返回译文」开关（需求 4）。
   */
  function renderExplain(box, res, showOriginal) {
    if (!box) return;
    const body = (showOriginal ? res.original : res.text) || '';
    const label = window.WordWiseAPI.explainLangLabel(res.lang);

    const meta = [res.translated ? `已自动翻译为 ${label}` : `讲解语言：${label}`];
    if (res.note) meta.push(res.note);

    const differs = !!(res.original && res.original !== res.text);
    const btn = differs
      ? `<button class="ghost-btn xs" id="ai-orig-toggle">${showOriginal ? '返回译文' : '查看原文'}</button>`
      : '';

    box.innerHTML =
      `<div class="ai-lang-bar"><span class="ai-lang-meta">${U().esc(meta.join(' · '))}</span>${btn}</div>` +
      U().renderMarkdown(body);

    // 用 box.querySelector 而不是 getElementById：查词页的 #ai-body 与详情卡的
    // #dc-pane-ai 可能同时存在，按 id 取会绑到错误的那一个。
    box.querySelector('.ai-orig-toggle')?.addEventListener('click', () => {
      renderExplain(box, res, !showOriginal);
    });
  }

  /** 在指定容器里流式输出 AI 讲解。 */
  async function explainInto(containerId, word, question) {
    const box = document.getElementById(containerId);
    if (!box) return;

    explainBuffer = '';
    box.innerHTML = '<div class="loading"><div class="spinner"></div><span>正在调用本地模型…</span></div>';

    // 订阅流式增量
    if (unlistenDelta) { try { unlistenDelta(); } catch (e) {} }
    unlistenDelta = await window.WordWiseAPI.listen('explain://delta', (payload) => {
      if (!payload || payload.word !== word) return;
      explainBuffer += payload.delta || '';
      box.innerHTML = U().renderMarkdown(explainBuffer) + '<span class="cursor"></span>';
      box.scrollTop = box.scrollHeight;
    });

    try {
      // ★ 后端返回的已是 ExplainResult 结构体，不再是可直接渲染的字符串。
      //   兼容旧后端：拿不到对象时退回字符串形态。
      const raw = await API.aiExplain(word, null, question);
      const res = (raw && typeof raw === 'object')
        ? raw
        : { word, text: (typeof raw === 'string' && raw) || explainBuffer,
            original: explainBuffer, lang: '', translated: false };

      explainCache[containerId] = res;
      renderExplain(box, res, false);
      return res;
    } catch (e) {
      box.innerHTML = `<div class="muted" style="color:#e5484d">讲解失败：${U().esc(e.message)}</div>`;
    } finally {
      if (unlistenDelta) { try { unlistenDelta(); } catch (err) {} unlistenDelta = null; }
    }
  }

  return {
    open, close, bind, explainInto, renderExplain,
    lastExplain: (id) => explainCache[id] || null,
    cacheExplain: (id, res) => { explainCache[id] = res; },
    get current() { return current; },
  };
})();

/* ---------------- 查词页 ---------------- */

const Lookup = (() => {
  const { API } = window.WordWiseAPI;
  const U = () => window.WW;

  let lastResult = null;
  let suggestTimer = null;
  let reqSeq = 0;   // 请求序号：只有最后一次查询能写界面，避免旧请求覆盖新结果

  /** 给任意 Promise 套一个硬超时，防止 invoke 长时间 pending 时界面无限转圈。 */
  function withTimeout(p, ms, msg) {
    return new Promise((resolve, reject) => {
      const t = setTimeout(() => reject(new Error(msg)), ms);
      Promise.resolve(p).then(
        (v) => { clearTimeout(t); resolve(v); },
        (e) => { clearTimeout(t); reject(e); }
      );
    });
  }

  function bind() {
    const input = document.getElementById('lk-input');
    const go = document.getElementById('lk-go');
    const refresh = document.getElementById('lk-refresh');

    go?.addEventListener('click', () => query(input.value));
    input?.addEventListener('keydown', (e) => {
      if (e.key === 'Enter') {
        e.preventDefault();
        query(input.value);
      }
    });
    input?.addEventListener('input', () => {
      clearTimeout(suggestTimer);
      const v = input.value.trim();
      if (v.length < 2) {
        document.getElementById('lk-suggest').innerHTML = '';
        return;
      }
      suggestTimer = setTimeout(() => loadSuggest(v), 320);
    });
    refresh?.addEventListener('click', () => query(input.value, true));

    // 查询语言 + 搜索引擎。原来这里还有「源语言」下拉和 ⇄ 交换按钮，
    // 与「目标语言」选项完全重复，已合并为一个下拉（用户反馈）。
    document.getElementById('lk-dst-lang')?.addEventListener('change', saveDirection);
    document.getElementById('lk-engine')?.addEventListener('change', saveDirection);

    // AI 追问
    const ask = document.getElementById('ai-send');
    const aiInput = document.getElementById('ai-input');
    ask?.addEventListener('click', () => sendQuestion());
    aiInput?.addEventListener('keydown', (e) => {
      if (e.key === 'Enter') { e.preventDefault(); sendQuestion(); }
    });

    // 最近搜索
    loadRecent();
    initDirection();
    initLayout();
  }

  /* ---- 两栏布局：拖动分栏 + 收起右栏 ---- */

  const LS_SPLIT = 'ww.lookup.split';   // 左栏占比 0.30 ~ 0.78
  const LS_RIGHT = 'ww.lookup.right';   // '0' = 右栏已收起
  const SPLIT_MIN = 0.30;
  const SPLIT_MAX = 0.78;
  const LEFT_MIN_PX = 300;              // 拖动时左栏最小像素宽
  const RIGHT_MIN_PX = 300;             // 拖动时右栏最小像素宽

  function lsGet(k) { try { return window.localStorage.getItem(k); } catch (e) { return null; } }
  function lsSet(k, v) { try { window.localStorage.setItem(k, v); } catch (e) {} }

  function colsEl() { return document.querySelector('.lookup-cols'); }

  /**
   * 应用左栏占比（0~1），clamp 到 SPLIT_MIN~SPLIT_MAX。**不写本地存储**。
   * 拖动时每次 pointermove 都会调用，localStorage 是同步 IO，不能每次都写。
   */
  function applySplit(ratio) {
    const cols = colsEl();
    if (!cols) return SPLIT_MIN;
    const r = Math.min(SPLIT_MAX, Math.max(SPLIT_MIN, Number(ratio) || 0.52));
    cols.style.setProperty('--lk-left', (r * 100).toFixed(2) + '%');
    cols.dataset.split = String(r);
    return r;
  }

  /** 对外接口：设置比例并持久化（拖动结束、双击复位、方向键微调时用）。 */
  function setSplit(ratio) {
    const r = applySplit(ratio);
    lsSet(LS_SPLIT, String(r));
    return r;
  }

  function currentSplit() {
    const cols = colsEl();
    const v = cols ? parseFloat(cols.dataset.split) : NaN;
    return Number.isFinite(v) ? v : 0.52;
  }

  /** 收起/展开右栏；按钮文案与本地存储一起同步。 */
  function setCollapsed(on) {
    const cols = colsEl();
    if (!cols) return;
    cols.classList.toggle('collapsed', !!on);
    const btn = document.getElementById('lk-ai-toggle');
    if (btn) {
      btn.textContent = on ? '展开右栏' : '收起右栏';
      btn.title = on
        ? '展开 AI 讲解与在线搜索面板'
        : '收起右侧 AI 讲解与在线搜索面板，让词条区占满宽度';
    }
    lsSet(LS_RIGHT, on ? '0' : '1');
  }

  function isCollapsed() {
    const cols = colsEl();
    return !!(cols && cols.classList.contains('collapsed'));
  }

  /** 初始化分栏：恢复上次的宽度与收起状态，并接上鼠标/键盘操作。 */
  function initLayout() {
    const cols = colsEl();
    if (!cols) return;

    const saved = parseFloat(lsGet(LS_SPLIT));
    applySplit(Number.isFinite(saved) ? saved : 0.52);
    setCollapsed(lsGet(LS_RIGHT) === '0');

    const rz = document.getElementById('lk-resizer');
    if (rz) {
      let dragging = false;

      rz.addEventListener('pointerdown', (e) => {
        if (e.button !== 0) return;
        dragging = true;
        document.body.classList.add('col-resizing');
        // 指针捕获：鼠标移出拖动条甚至移出窗口也不会丢事件
        try { rz.setPointerCapture(e.pointerId); } catch (err) {}
        e.preventDefault();
      });

      rz.addEventListener('pointermove', (e) => {
        if (!dragging) return;
        const rect = cols.getBoundingClientRect();
        if (rect.width <= 0) return;
        const left = Math.max(
          LEFT_MIN_PX,
          Math.min(rect.width - RIGHT_MIN_PX, e.clientX - rect.left)
        );
        applySplit(left / rect.width);   // 拖动过程中不写盘
        e.preventDefault();
      });

      const endDrag = (e) => {
        if (!dragging) return;
        dragging = false;
        document.body.classList.remove('col-resizing');
        try { rz.releasePointerCapture(e.pointerId); } catch (err) {}
        lsSet(LS_SPLIT, String(currentSplit()));
      };
      rz.addEventListener('pointerup', endDrag);
      rz.addEventListener('pointercancel', endDrag);

      // 双击复位
      rz.addEventListener('dblclick', () => setSplit(0.52));

      // 键盘可达：聚焦后 ← → 微调
      rz.addEventListener('keydown', (e) => {
        if (e.key !== 'ArrowLeft' && e.key !== 'ArrowRight') return;
        e.preventDefault();
        setSplit(currentSplit() + (e.key === 'ArrowRight' ? 0.03 : -0.03));
      });
    }

    document.getElementById('lk-ai-toggle')?.addEventListener('click', () => {
      setCollapsed(!isCollapsed());
    });
    // 注：AI 面板右上角原来还有一个「›」收起图标，与上方头部的「收起右栏」
    // 是同一个动作的重复控件，容易造成「× 是关闭吗」的误导，已移除。

    // 讲解语言下拉：切换即时生效 + 立刻重译当前讲解（不重新生成）
    bindExplainLang();
    initExplainLang();

    // 窗口尺寸变化：窄屏交给 CSS 堆叠；宽屏按像素下限重新收敛一次，
    // 否则把窗口拉窄后百分比不变、左栏会挤到不可用。
    window.addEventListener('resize', () => {
      const c = colsEl();
      if (!c) return;
      if (window.innerWidth <= 1080) { c.style.removeProperty('--lk-left'); return; }
      const total = c.getBoundingClientRect().width;
      if (total <= 0) return;
      const left = Math.max(
        LEFT_MIN_PX,
        Math.min(total - RIGHT_MIN_PX, currentSplit() * total)
      );
      applySplit(left / total);
    });
  }

  /* ---- 语言转换方向（需求 7） ---- */

  async function initDirection() {
    // 引擎下拉
    try {
      const engines = await API.searchEngines();
      const sel = document.getElementById('lk-engine');
      if (sel && engines) {
        sel.innerHTML = engines.map(e =>
          `<option value="${U().esc(e.id)}">${U().esc(e.label)}</option>`).join('');
      }
    } catch (e) { /* 忽略 */ }

    try {
      const d = await API.getDirection();
      const dst = document.getElementById('lk-dst-lang');
      const eng = document.getElementById('lk-engine');
      // 老配置里 target_lang 可能是空串，给个可靠的兜底
      if (dst) dst.value = d.target_lang || 'en';
      if (eng) eng.value = d.search_engine || 'bing';
      updateWebEngineLabel();
    } catch (e) { /* 忽略 */ }
  }

  async function saveDirection() {
    const dst = document.getElementById('lk-dst-lang')?.value || 'en';
    const eng = document.getElementById('lk-engine')?.value || 'bing';
    try {
      await API.setDirection(null, dst, eng);
      updateWebEngineLabel();
      U().toast(`查询语言已切到「${U().langLabel(dst)}」`, 'ok');
    } catch (e) { /* 忽略 */ }
  }

  function updateWebEngineLabel() {
    const sel = document.getElementById('lk-engine');
    const label = document.getElementById('lk-web-engine');
    if (sel && label) {
      label.textContent = sel.options[sel.selectedIndex]
        ? sel.options[sel.selectedIndex].textContent : '必应';
    }
  }

  /* ---- AI 讲解语言（需求 1 / 4 / 5） ---- */

  /**
   * 讲解语言下拉。它和上方「语言」（查哪种语言的词）是两件事：
   * - 上方下拉决定「查哪个语种的词」；
   * - 这个下拉决定「AI 用哪种语言回答」，对所有讲解与追问生效。
   *
   * 切换流程（选择后即时生效、无需二次触发）：
   *   1. 立刻写入配置并落盘 → 下次讲解/追问自动用新语言；
   *   2. 若当前已有讲解，拿缓存的**模型原文**重译一次并替换显示，
   *      不重新生成，所以内容不变、几乎是秒回。
   */
  function fillExplainLang(lang) {
    const code = (lang || 'zh').toLowerCase();
    ['ai-lang', 'set-explain-lang'].forEach(id => {
      const sel = document.getElementById(id);
      if (!sel) return;
      sel.innerHTML = window.WordWiseAPI.explainLangOptions(code);
      sel.value = code;
    });
  }

  async function initExplainLang() {
    let cfg = (window.App && window.App.config) || null;
    if (!cfg) {
      try { cfg = await API.getConfig(); } catch (e) { return; }
    }
    fillExplainLang(cfg.explain_lang || 'zh');
  }

  function bindExplainLang() {
    const onchange = async (e) => {
      const lang = e.target.value;
      fillExplainLang(lang);
      try {
        await API.setExplainLang(lang);
      } catch (err) {
        U().toast(err.message || '保存讲解语言失败', 'err');
        return;
      }
      U().toast(`AI 讲解语言已切到「${window.WordWiseAPI.explainLangLabel(lang)}」`, 'ok');
      await retranslateCurrent(lang);
    };
    document.getElementById('ai-lang')?.addEventListener('change', onchange);
    document.getElementById('set-explain-lang')?.addEventListener('change', onchange);
  }

  /** 把已显示的讲解按新语言重译（拿模型原文译，不重新生成）。 */
  async function retranslateCurrent(lang) {
    for (const id of ['ai-body', 'dc-pane-ai']) {
      const box = document.getElementById(id);
      const res = Detail.lastExplain(id);
      if (!box || !res || !res.original) continue;

      box.innerHTML = '<div class="loading"><div class="spinner"></div><span>正在翻译…</span></div>';
      try {
        const out = await API.translateText(res.original, lang);
        const next = {
          word: res.word,
          text: out.text || res.original,
          original: res.original,   // 原文始终保留，随时可切回去
          lang: out.lang || lang,
          translated: !!out.translated,
          note: out.note,
        };
        Detail.cacheExplain(id, next);
        Detail.renderExplain(box, next, false);
      } catch (e) {
        Detail.renderExplain(box, res, false);
        U().toast(`翻译失败，已保留上一次显示：${e.message}`, 'err');
      }
    }
  }

  let askTarget = '';

  /** 加载辞书跳转链接（需求 14）：本地查不到时给用户权威辞书入口。 */
  async function loadDictLinks(word, lang) {
    return loadDictLinksInline('lk-dictlinks', word, lang);
  }

  function sendQuestion() {
    const aiInput = document.getElementById('ai-input');
    const q = aiInput.value.trim();
    if (!q) return;
    const word = askTarget || (lastResult && lastResult.word);
    if (!word) { U().toast('请先查询一个单词', 'err'); return; }
    aiInput.value = '';
    Detail.explainInto('ai-body', word, q);
  }

  async function loadSuggest(q) {
    try {
      const list = await API.suggest(q);
      const box = document.getElementById('lk-suggest');
      if (!list || !list.length) { box.innerHTML = ''; return; }
      box.innerHTML = list.slice(0, 10).map(s => `
        <span class="suggest-chip" data-word="${U().esc(s.word)}">
          ${U().esc(s.word)}${s.gloss ? `<span class="sc-src">${U().esc(s.gloss.slice(0, 18))}</span>` : ''}
        </span>`).join('');
      box.querySelectorAll('.suggest-chip').forEach(c => {
        c.addEventListener('click', () => query(c.dataset.word));
      });
    } catch (e) {
      /* 联想失败静默处理，不打扰用户 */
    }
  }

  async function loadRecent() {
    try {
      const list = await API.recentSearches();
      if (!list || !list.length) return;
      const box = document.getElementById('lk-suggest');
      if (box && !box.innerHTML.trim()) {
        box.innerHTML = list.slice(0, 8).map(w =>
          `<span class="suggest-chip" data-word="${U().esc(w)}">${U().esc(w)}</span>`).join('');
        box.querySelectorAll('.suggest-chip').forEach(c => {
          c.addEventListener('click', () => query(c.dataset.word));
        });
      }
    } catch (e) { /* 忽略 */ }
  }

  async function query(word, forceRefresh = false) {
    word = (word || '').trim();
    if (!word) { U().toast('请输入要查询的单词', 'err'); return; }

    const input = document.getElementById('lk-input');
    if (input) input.value = word;
    document.getElementById('lk-suggest').innerHTML = '';

    const box = document.getElementById('lk-result');
    const my = ++reqSeq;                 // 本次请求的序号
    const alive = () => my === reqSeq;   // 期间用户又发起新查询则放弃写界面

    // 配置优先用启动时的缓存，避免每次查词都多一次往返、把渲染挡在 await 后面
    let cfg = (window.App && window.App.config) || null;
    if (!cfg) { try { cfg = await API.getConfig(); } catch (e) {} }
    const study = (cfg && cfg.study) || {};

    // 看门狗：后端最坏会走到「本地大模型兜底」（默认 120 秒），
    // 这里给一个明确上限，超时就渲染失败态，绝不让界面一直转圈。
    const netTimeout = (cfg && cfg.network && cfg.network.lookup_timeout_secs) || 8;
    const budgetMs = (netTimeout + 12) * 1000;

    // 分阶段提示：超过单源预算还没回来，说明在线词典没成，正在等本地模型
    let stageTimer = setTimeout(() => {
      if (alive()) box.innerHTML = U().loadingHtml('在线词典未响应，正在用本地大模型生成…');
    }, netTimeout * 1000);

    box.innerHTML = U().loadingHtml('正在联网查询…');

    let res;
    try {
      res = await withTimeout(
        API.lookup(word, null, forceRefresh),
        budgetMs,
        `查询超时：后端 ${Math.round(budgetMs / 1000)} 秒未返回结果`
      );
    } catch (e) {
      clearTimeout(stageTimer);
      if (!alive()) return;
      box.innerHTML = `<div class="empty-state">
        <div class="es-icon">&#128533;</div>
        <p>查询失败</p>
        <p class="muted">${U().esc(e.message)}</p>
        <p class="muted" style="margin-top:12px">建议：检查网络连接，或在设置中确认本地模型服务已启动</p>
      </div>`;
      // 仍然允许用 AI 讲解这个词
      askTarget = word;
      document.getElementById('ai-body').innerHTML =
        `<p class="muted">词典查询失败，可点击下方按钮让本地模型直接讲解「${U().esc(word)}」。</p>`;
      return;
    }

    clearTimeout(stageTimer);
    if (!alive()) return;

    // 渲染出错也要给出提示，而不是把 loading 留在屏幕上
    try {
      renderResult(res, study);
    } catch (e) {
      box.innerHTML = `<div class="empty-state">
        <div class="es-icon">&#128533;</div>
        <p>结果渲染失败</p>
        <p class="muted">${U().esc(e && e.message ? e.message : String(e))}</p>
        <p class="muted" style="margin-top:12px">词条已取回（来源：${U().esc((res.sources || []).join(' · '))}），但界面渲染出错，请按 Ctrl+Shift+I 打开控制台查看详情。</p>
      </div>`;
      console.error('[lookup] 渲染结果失败', e);
      return;
    }

    loadDictLinks(res.word, res.lang);

    // 自动开始讲解（若开启）
    if (study.ai_explain !== false) {
      Detail.explainInto('ai-body', res.word).catch(() => {});
    }

    // 在线搜索（需求 6）：替代维基百科
    loadWebResults(res.word);
  }

  /** 把查到的结果画到 lk-result 面板上（与交互绑定放在一起，便于整体 try/catch）。 */
  function renderResult(res, study) {
    const box = document.getElementById('lk-result');

    lastResult = res;
    askTarget = res.word;

    const srcLine = res.sources && res.sources.length
      ? `<div class="muted" style="margin-top:10px;font-size:12px">数据来源：${res.sources.map(U().esc).join(' · ')}${res.from_cache ? '（缓存）' : ''}${res.from_llm ? '（本地模型生成）' : ''}</div>`
      : '';

    box.innerHTML = U().renderEntry(res.entry, {
      showInflections: study.show_inflections !== false,
      showExamples: study.show_examples !== false,
      showRelated: study.show_related === true,
      showMnemonic: study.show_mnemonic !== false,
      showPhonetic: study.show_phonetic !== false,
    }) + '<div id="lk-dictlinks" class="we-section"></div>' + srcLine + `
    <div class="btn-row" style="margin-top:16px">
      <button class="primary-btn sm" id="lk-ai">AI 讲解</button>
      <button class="ghost-btn sm" id="lk-add">加入词库</button>
      <button class="ghost-btn sm" id="lk-detail">查看详情卡</button>
    </div>`;

    box.querySelectorAll('.rel-chip').forEach(c => {
      c.addEventListener('click', () => query(c.dataset.word));
    });
    // 变形词点击 → 查询该变形
    box.querySelectorAll('.infl-form.clickable').forEach(f => {
      f.addEventListener('click', () => query(f.dataset.word));
    });
    // 发音按钮委托（作用于本次结果区）
    U().speakBind(box);
    document.getElementById('lk-add')?.addEventListener('click', async () => {
      try {
        await API.addWord(res.entry);
        U().toast('已加入词库', 'ok');
      } catch (e) { U().toast(e.message, 'err'); }
    });
    document.getElementById('lk-detail')?.addEventListener('click', () => Detail.open(res.entry));
    document.getElementById('lk-ai')?.addEventListener('click', () => {
      Detail.explainInto('ai-body', res.word).catch(() => {});
    });
  }

  /** 在线搜索（需求 6）。 */
  async function loadWebResults(word) {
    const box = document.getElementById('lk-web');
    if (!box) return;
    let cfg = (window.App && window.App.config) || null;
    if (!cfg) { try { cfg = await API.getConfig(); } catch (e) {} }
    if (cfg && cfg.web_search_enabled === false) {
      box.innerHTML = '<p class="muted">在线搜索已在设置中关闭。</p>';
      return;
    }
    const engine = document.getElementById('lk-engine')?.value || (cfg && cfg.search_engine) || 'bing';
    box.innerHTML = U().loadingHtml('正在搜索…');
    let results;
    try {
      // 同样加看门狗，避免这个面板一直转圈
      results = await withTimeout(
        API.webSearch(word, engine, 8, false),
        ((cfg && cfg.network && cfg.network.timeout_secs) || 30) * 1000 + 5000,
        '在线搜索超时'
      );
    } catch (e) {
      box.innerHTML = `<p class="muted">搜索失败：${U().esc(e.message)}</p>`;
      return;
    }
    if (!results || !results.length) {
      box.innerHTML = '<p class="muted">未找到相关网络结果，可尝试切换搜索引擎。</p>';
      return;
    }
    box.innerHTML = results.map(r => `
      <div class="web-item" data-url="${U().esc(r.url)}">
        <div class="wi-title">${U().esc(r.title)}</div>
        <div class="wi-url">${U().esc(r.url)}</div>
        ${r.snippet ? `<div class="wi-snippet">${U().esc(r.snippet)}</div>` : ''}
      </div>`).join('');
    box.querySelectorAll('.web-item').forEach(it => {
      it.addEventListener('click', async () => {
        try { await API.openUrl(it.dataset.url); }
        catch (e) { U().toast('打开失败：' + e.message, 'err'); }
      });
    });
  }

  return {
    bind, query, loadWebResults,
    initLayout, setSplit, setCollapsed, isCollapsed,
    get currentSplit() { return currentSplit(); },
    get last() { return lastResult; },
  };
})();

window.Detail = Detail;
window.Lookup = Lookup;
