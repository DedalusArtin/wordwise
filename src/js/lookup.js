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

    // 朗读按钮的数据挂载。
    //
    // 详情卡是 index.html 里的**静态**结构，`#dc-audio` 与音标行里的 🔇 都
    // 不在任何 `[data-entry-word]` 容器里 —— 所以委托取不到词，点了没反应
    // （这正是「朗读按钮点不动」的来源之一）。这里把词 / 音频 / 语言直接
    // 写到按钮和音标容器上，并在 bind() 里给整个 overlay 挂一次委托。
    const audioBtn = document.getElementById('dc-audio');
    if (audioBtn) {
      audioBtn.dataset.speakWord = entry.word || '';
      audioBtn.dataset.speakAudio = (entry.phonetic && entry.phonetic.audio) || '';
      audioBtn.dataset.speakLang = entry.lang || 'en';
    }

    // 音标：统一走 phoneticHtml —— 语言标注（英/美/拼音/读音/罗马音）、
    // 斜杠规则、IPA 字体都只有一份实现，改样式只改一处。
    const phonEl = document.getElementById('dc-phonetic');
    const phonHtml = U().phoneticHtml(entry, { speak: true });
    // 一个音标都没有时也留一个纯发音按钮（TTS 兜底），别整块空着
    phonEl.innerHTML = phonHtml
      || `<span class="phon-item">${U().speakBtn(entry, 'us', '发音')}</span>`;
    // 让音标行里的 🔊 也能取到词（委托会找最近的 [data-entry-word]）
    phonEl.setAttribute('data-entry-word', entry.word || '');

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

  /**
   * 详情卡的「释义」面板。
   *
   * 义项渲染**复用** `U().senseGroup` 与 `U().splitSensesByScript`，不在这里
   * 另写一份：查词结果与详情卡显示的是同一个词条，两边一旦各写一套，
   * 表现就是「结果页中文在前英文在后，详情卡却中英混排」。用户按哪个页
   * 面记住的排版，另一个页面就反过来打脸。
   */
  function renderDefs(entry, study) {
    if (!entry.senses || !entry.senses.length) {
      return '<div class="muted">暂无释义。可点击「AI 讲解」让本地模型生成完整词条。</div>';
    }
    const showEx = study.show_examples !== false;
    const showMn = study.show_mnemonic !== false;
    const o = { showExamples: showEx };

    // 与 renderEntry 同一套分组：母语组在前，原文组在后；只有一组时不加小标题
    const [local, native] = U().splitSensesByScript(entry.senses);
    let html = '';
    if (local.length && native.length) {
      const title = U().langLabel(entry.lang || '') === '中文'
        ? '参考释义' : `${U().langLabel(entry.lang || '')}释义`;
      html += U().senseGroup(null, local, o) + U().senseGroup(title, native, o);
    } else {
      html += U().senseGroup(null, local.length ? local : native, o);
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
    const rels = U().splitRelated(entry.related);
    if (!rels.length) return '<div class="muted">暂无相关词。</div>';
    return '<div class="rel-list">' + rels.map(r =>
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
    // 详情卡内的发音按钮委托（整卡只挂一次，bind 只跑一次）
    U().speakBind(document.getElementById('detail-overlay'));
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
   * 每个容器的存档状态：`{ saved }`。
   *
   * 讲解**在生成时后端就已自动落库**（`cmd_ai_explain` 里顺手存的），
   * 所以这里不记「有没有存档」，只记「有没有进一步并入词库」。
   */
  const explainSaved = {};

  /**
   * 渲染一次讲解结果。
   *
   * 顶部固定一条语言提示条：
   * - 说明本次用的讲解语言、是否经过自动翻译、失败提示；
   * - 译文与原文不一致时给出「查看原文 / 返回译文」开关（需求 4）；
   * - 「并入词库」：把讲解正文交给模型整理成结构化词条写进词库（需求 C）。
   */
  function renderExplain(box, res, showOriginal) {
    if (!box) return;
    const body = (showOriginal ? res.original : res.text) || '';
    const label = window.WordWiseAPI.explainLangLabel(res.lang);

    const meta = [res.translated ? `已自动翻译为 ${label}` : `讲解语言：${label}`];
    if (res.note) meta.push(res.note);

    const differs = !!(res.original && res.original !== res.text);
    // ★ 这里必须用 class 而不是 id：同一个页面里 #ai-body 与 #dc-pane-ai 可能
    //   同时存在，用 id 会绑到错误的那个容器上，按钮永远点不动。
    const origBtn = differs
      ? `<button class="ghost-btn xs ai-orig-toggle">${showOriginal ? '返回译文' : '查看原文'}</button>`
      : '';
    // 存档直出时给一个「重新讲解」出口，否则用户没法刷新内容
    const regenBtn = res.fromArchive
      ? '<button class="ghost-btn xs ai-regen">重新讲解</button>'
      : '';

    const saved = !!explainSaved[box.id];
    const bookBtn = saved
      ? '<button class="ghost-btn xs is-disabled" disabled>已并入词库</button>'
      : '<button class="ghost-btn xs ai-to-book">并入词库</button>';

    box.innerHTML =
      `<div class="ai-lang-bar"><span class="ai-lang-meta">${U().esc(meta.join(' · '))}</span>` +
      `<span class="ai-lang-actions">${origBtn}${regenBtn}${bookBtn}</span></div>` +
      U().renderMarkdown(body);

    // 用 box.querySelector 而不是 getElementById：查词页的 #ai-body 与详情卡的
    // #dc-pane-ai 可能同时存在，按 id 取会绑到错误的那一个。
    box.querySelector('.ai-orig-toggle')?.addEventListener('click', () => {
      renderExplain(box, res, !showOriginal);
    });

    box.querySelector('.ai-regen')?.addEventListener('click', () => {
      // 显式要求跳过存档，强制重新调用模型
      explainInto(box.id, res.word, null, { preferArchive: false });
    });

    box.querySelector('.ai-to-book')?.addEventListener('click', async (ev) => {
      const b = ev.currentTarget || ev.target;
      if (b) { b.disabled = true; b.textContent = '整理中…'; }
      try {
        const entry = await API.explainToEntry(res.word, null, res.lang, res.text);
        explainSaved[box.id] = true;
        U().toast(`已按讲解把「${entry.word}」并入词库`, 'ok');
        renderExplain(box, res, showOriginal);
      } catch (e) {
        if (b) { b.disabled = false; b.textContent = '并入词库'; }
        U().toast(e && e.message ? e.message : String(e), 'err');
      }
    });
  }

  /**
   * 在指定容器里流式输出 AI 讲解。
   *
   * `opts.preferArchive`（默认 true）：先用本地讲解存档，有就直接渲染、不调模型。
   *
   * 为什么默认走存档：一次讲解是一次完整的模型调用，本地小模型动辄十几秒。
   * 同一个词第二次点开还要再等十几秒，用户会以为软件卡了。存档的存在就是
   * 为了「第二次秒回」；想刷新内容，提示条上有「重新讲解」。
   */
  async function explainInto(containerId, word, question, opts) {
    const box = document.getElementById(containerId);
    if (!box) return;

    const isFollowUp = !!(question && String(question).trim());
    // 追问不覆盖主讲解，也就不参与存档，先把上一次的状态清掉
    if (isFollowUp) explainSaved[containerId] = false;

    // ---- 1) 优先读存档 ----
    if (!isFollowUp && (!opts || opts.preferArchive !== false)) {
      try {
        const row = await API.getExplain(word, null, null);
        if (row && (row.text || row.original)) {
          const res = {
            word: row.word || word,
            text: row.text || row.original,
            original: row.original || row.text,
            lang: row.explain_lang,
            translated: !!row.translated,
            fromArchive: true,
            note: `来自本地讲解存档（${U().timeAgo(row.updated_at)}）`,
          };
          explainCache[containerId] = res;
          explainSaved[containerId] = !!row.saved;
          renderExplain(box, res, false);
          return res;
        }
      } catch (e) {
        // 读存档失败不该挡路，照常走模型
        console.warn('[lookup] 读取讲解存档失败', e);
      }
    }

    // ---- 2) 调用模型 ----
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

      // 存档：后端在首次讲解时已自动落库，这里再调一次是为了**拿回存档行**
      // （带 `saved` 标记）—— 否则用户在别处把这条讲解并进词库后，重新打开
      // 面板会又显示成「并入词库」，点了才发现已经并过。
      if (!isFollowUp) {
        try {
          const row = await API.saveExplain(
            res.word || word, null, res.lang, res.text, res.original, res.translated);
          explainSaved[containerId] = !!(row && row.saved);
        } catch (e) {
          // 存档失败不影响讲解本身，但要说清「并入词库」依然可用
          console.warn('[lookup] 讲解存档失败', e);
        }
      }

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
    // 供词库页展示「讲解存档」时复用同一套渲染 + 「并入词库」按钮逻辑：
    // 存档正文直接画出来，不重新调用模型。
    cacheExplain: (id, res) => { explainCache[id] = res; },
    setExplainSaved: (id, saved) => { explainSaved[id] = !!saved; },
    get current() { return current; },
  };
})();

/* ---------------- 查词页 ---------------- */

const Lookup = (() => {
  const { API } = window.WordWiseAPI;
  const U = () => window.WW;

  let lastResult = null;
  let suggestTimer = null;
  let reqSeq = 0;      // 请求序号：只有最后一次查询能写界面，避免旧请求覆盖新结果
  let transSeq = 0;    // 词级译文也有自己的序号，防止切换语言时旧译文盖住新译文

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
    document.getElementById('lk-back')?.addEventListener('click', goBack);
    // Alt+← / Alt+→：仿浏览器的历史前进后退
    document.addEventListener('keydown', (e) => {
      if (!e.altKey) return;
      if (U().isTypingTarget(e.target)) return;
      if (e.key === 'ArrowLeft') { e.preventDefault(); goBack(); }
    });
    syncBackBtn();

    // 只绑搜索引擎。语言方向改由顶部的方向选择器（DirPicker）统一管理，
    // 它同时驱动查词、AI 讲解、在线搜索三个面板。
    document.getElementById('lk-engine')?.addEventListener('change', saveEngine);

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
      const eng = document.getElementById('lk-engine');
      if (eng) eng.value = d.search_engine || 'bing';
      updateWebEngineLabel();
    } catch (e) { /* 忽略 */ }

    // 查询语言已由顶部方向选择器统一管理，这里只负责搜索方式与占位提示
    syncLookupPlaceholder();
    // 方向一变：占位提示、待重查的标记、在线搜索语言都要跟着走（需求 4）
    window.DirPicker?.onChange(() => {
      syncLookupPlaceholder();
      if (lastResult && lastResult.word) {
        loadWebResults(lastResult.word);
        // 目标语言变了 → 词级译文必须重取，否则会显示上一个语言的译文（需求 3）
        loadWordTranslation(lastResult);
      }
    });
  }

  async function saveEngine() {
    const eng = document.getElementById('lk-engine')?.value || 'bing';
    try {
      await API.setDirection(null, null, eng);
      updateWebEngineLabel();
      U().toast(`搜索引擎已切到「${engineLabel()}」`, 'ok');
      if (lastResult && lastResult.word) loadWebResults(lastResult.word);
    } catch (e) { /* 忽略 */ }
  }

  /** 按当前互译方向刷新输入框占位提示（需求 4）。 */
  function syncLookupPlaceholder() {
    const input = document.getElementById('lk-input');
    const D = window.DirPicker;
    if (!input || !D) return;
    const to = U().langLabel(D.to);
    input.placeholder = D.from === D.AUTO
      ? `输入单词，自动识别语种 → 给出${to}释义与译文…`
      : `输入${U().langLabel(D.from)}单词，查看释义与${to}译文…`;

    // AI 追问的占位也要跟着语言走
    const ai = document.getElementById('ai-input');
    if (ai) ai.placeholder = `追问，例如：这个词和 rely 有什么区别？（用${U().langLabel(D.to)}回答）`;
  }

  function engineLabel() {
    const sel = document.getElementById('lk-engine');
    return sel && sel.options[sel.selectedIndex] ? sel.options[sel.selectedIndex].textContent : '必应';
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

  async function query(word, forceRefresh = false, opts = {}) {
    word = (word || '').trim();
    if (!word) { U().toast('请输入要查询的单词', 'err'); return; }

    const input = document.getElementById('lk-input');
    if (input) input.value = word;
    document.getElementById('lk-suggest').innerHTML = '';

    if (!opts.fromNav) nav.push({ kind: 'word', text: word });

    // 从一开始就不像词条的整句：直接走翻译，别让用户白等一轮必然失败的词典查询
    if (looksLikeSentence(word)) {
      querySentence(word, { fromNav: true });
      return;
    }

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
      // 词典查不到 + 输入本来就像整句 → 自动改走整句翻译，而不是甩一个「查询失败」
      if (looksLikeSentence(word)) {
        querySentence(word, { fromNav: true });
        return;
      }
      renderFail(box, word, e.message);
      askTarget = word;
      const aiBox = document.getElementById('ai-body');
      if (aiBox) {
        aiBox.innerHTML =
          `<p class="muted">所有词典源都没给出这个词的释义，可点击下方按钮让本地模型直接讲解「${U().esc(word)}」。</p>`;
      }
      return;
    }

    clearTimeout(stageTimer);
    if (!alive()) return;

    // 查到了「空壳词条」（没有任何释义）而且输入像整句 → 翻译更有用
    if (looksLikeSentence(word)) {
      const hasSense = res && res.entry && (res.entry.senses || []).some(s => (s.definition || '').trim());
      if (!hasSense) { querySentence(word, { fromNav: true }); return; }
    }

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

    // 词级译文（需求 2）：与词典查询并行，不阻塞上面已经画好的词条。
    //
    // 译完之后再交给「双向词条」当**种子**：它有道接口限频很严，
    // 两个功能各自去翻一次会平白多一次请求，还容易撞上 429。
    // 翻译失败也照样往下走 —— 对应词条自己会补一次翻译。
    loadWordTranslation(res)
      .then(seed => loadPairs(res, seed))
      .catch(() => loadPairs(res, null));

    // 自动开始讲解（若开启）
    if (study.ai_explain !== false) {
      Detail.explainInto('ai-body', res.word).catch(() => {});
    }

    // 在线搜索（需求 6）：替代维基百科
    loadWebResults(res.word);
  }

  /**
   * 词级译文（需求 2 / 3）。
   *
   * 「查词」本身是词典查询，返回的是**词条释义**；用户要的
   * 「选日语得到 こんにちは」属于**翻译**，所以这里额外补一次翻译，
   * 放在词条最上方最显眼的位置。
   *
   * 目标语言与词条语言相同时不必翻译 —— 否则「中文词条、目标也是中文」
   * 会白白多打一次在线接口（那接口限频很严）。
   */
  async function loadWordTranslation(res) {
    const box = document.getElementById('lk-trans');
    const D = window.DirPicker;
    if (!box || !D || !res) return;

    const to = D.to;
    if (!to || res.lang === to) {
      box.innerHTML = '';
      box.classList.add('hidden');
      return;
    }

    box.classList.remove('hidden');
    box.innerHTML = U().loadingHtml('正在翻译…');

    const my = ++transSeq;
    try {
      const t = await API.translate(res.word, res.lang, to, false);
      if (my !== transSeq) return;                 // 已有更新的查询
      if (!t || !t.text) {
        box.innerHTML = '';
        box.classList.add('hidden');
        return;
      }

      const alts = (t.alternatives || []).filter(Boolean);
      box.innerHTML = `
        <div class="lk-trans-head">
          <span class="lk-trans-lang">${U().esc(U().langLabel(t.to))}</span>
          <span class="lk-trans-text">${U().esc(t.text)}</span>
          <button class="ghost-btn xs" data-lk-trans-speak title="朗读译文">朗读</button>
        </div>
        ${t.phonetic ? `<div class="lk-trans-phonetic">读音 ${U().esc(t.phonetic)}</div>` : ''}
        ${alts.length ? `<div class="lk-trans-alt">其他译法：${alts.map(U().esc).join(' · ')}</div>` : ''}
        ${t.note ? `<div class="lk-trans-note">${U().esc(t.note)}</div>` : ''}
      `;

      box.querySelector('[data-lk-trans-speak]')?.addEventListener('click', () => {
        if (t.tts_url && window.Speak && window.Speak.playUrl) {
          window.Speak.playUrl(t.tts_url, 'lk-trans');
        } else if (window.Speak && window.Speak.playTts) {
          window.Speak.playTts(t.text, t.to, 0, 'lk-trans');
        }
      });

      // 把译文往外递，给「双向词条」当种子，省得它再去打一次翻译接口
      return { translated: t.text, alternatives: alts };
    } catch (e) {
      if (my !== transSeq) return null;
      // 翻译失败不该影响已经渲染好的词条，安静地隐藏即可
      box.innerHTML = '';
      box.classList.add('hidden');
      console.warn('[lookup] 词级译文获取失败', e);
      return null;
    }
  }

  /** 双向词条的拉取序号：新查询会作废上一次还没回来的请求。 */
  let pairsSeq = 0;

  /**
   * 双向词条：**目标语言侧的对应词及其完整词条**。
   *
   * 用户场景（原话）：「这里我是中文转英文，应该下面详细介绍的是 crow
   * 或者其他能表示乌鸦的单词」「仿照有道词典两者都有，还有其他语言也要类似」。
   *
   * 主词条给的是「乌鸦」的中文解释，但用户真正要学的是「乌鸦用英语怎么说、
   * crow 怎么读、怎么用」。所以这里再按目标语言回查一次词典，把 crow 这类
   * 候选译法的完整词条挂到主卡片下面。
   *
   * `seed` 由「词级译文」那一步顺手递过来（`{ translated, alternatives }`）：
   * 翻译接口限频很严，同一条链路里不发第二遍请求。
   *
   * 与主查询并行发出、且**失败一律静默**：没有对应词只是少一块内容，
   * 主词条本身是完整的，不该弹出任何错误。
   */
  async function loadPairs(res, seed = null) {
    const box = document.getElementById('lk-pairs');
    const D = window.DirPicker;
    if (!box || !D || !res) return;

    // 目标语言与词条语言相同 → 主词条已经是目标语言的了，没有「对应词」可言
    const to = D.to;
    if (!to || res.lang === to) {
      box.innerHTML = '';
      box.classList.add('hidden');
      return;
    }

    box.classList.remove('hidden');
    box.innerHTML = U().loadingHtml('正在查询对应词条…');

    const my = ++pairsSeq;
    try {
      const pairs = await API.lookupPairs(
        res.word,
        res.lang,
        to,
        seed && seed.translated ? seed.translated : null,
        seed && seed.alternatives ? seed.alternatives : null,
      );
      if (my !== pairsSeq) return;
      if (!pairs || !pairs.length) {
        box.innerHTML = '';
        box.classList.add('hidden');
        return;
      }

      box.innerHTML = U().renderPairs(pairs, {
        showInflections: true,
        showExamples: true,
        showPhonetic: true,
      });

      // 对应词卡里的词头可点击单独查询
      box.querySelectorAll('.pair-word.clickable').forEach(el => {
        el.addEventListener('click', () => query(el.dataset.word));
      });
      // 卡内的变形词条也要能点（沿用主卡片的 class）
      box.querySelectorAll('.infl-form.clickable').forEach(f => {
        f.addEventListener('click', () => query(f.dataset.word));
      });
      U().speakBind(box);
    } catch (e) {
      if (my !== pairsSeq) return;
      box.innerHTML = '';
      box.classList.add('hidden');
      console.warn('[lookup] 对应词条获取失败', e);
    }
  }

  /** 把查到的结果画到 lk-result 面板上（与交互绑定放在一起，便于整体 try/catch）。 */
  function renderResult(res, study) {
    const box = document.getElementById('lk-result');

    lastResult = res;
    askTarget = res.word;

    const srcLine = res.sources && res.sources.length
      ? `<div class="muted" style="margin-top:10px;font-size:12px">数据来源：${res.sources.map(U().esc).join(' · ')}${res.from_cache ? '（缓存）' : ''}${res.from_llm ? '（本地模型生成）' : ''}${res.degraded ? ' · 启发式解析' : ''}</div>`
      : '';

    // 语言被书写系统判定纠正过 → 如实告诉用户（需求 3）
    const langNote = res.lang_note
      ? `<div class="lk-lang-note">${U().esc(res.lang_note)}</div>`
      : '';

    // 降级提示条：**查询是成功的**，只是某项「增强能力」没参与（最常见的是
    // 本地大模型没开 / 超时）。用户明确要求「不能因为 AI 服务器没开就显示
    // 查询失败」，所以这类信息一律走这条提示，绝不进失败态。
    const degradeNote = res.note
      ? `<div class="lk-degrade-note">${U().esc(res.note)}</div>`
      : '';

    // 词级译文容器（需求 2）：放**目标语言的实际文字**（「你好」→「こんにちは」），
    // 读音只是它下面的附属标注。查询后由 loadWordTranslation 填充。
    const transBox = '<div id="lk-trans" class="lk-trans hidden"></div>';

    // 双向词条容器：目标语言侧的对应词详解（「乌鸦」→ crow / rook / raven 的完整英文词条）。
    // 查询后由 loadPairs 填充 —— 与主词条并行拉取，绝不拖慢主结果的出现。
    const pairsBox = '<div id="lk-pairs" class="lk-pairs hidden"></div>';

    // 结果区采用「分块卡片 + 标签切换」排布（`tabs: true`）。
    // 「对应词」不进标签：它是异步补齐的另一个容器（#lk-pairs），
    // 硬塞进标签面板会让「标签已经画好、内容还没回来」变成空面板。
    box.innerHTML = langNote + degradeNote + transBox + U().renderEntry(res.entry, {
      tabs: true,
      showInflections: study.show_inflections !== false,
      showExamples: study.show_examples !== false,
      showRelated: study.show_related === true,
      showMnemonic: study.show_mnemonic !== false,
      showPhonetic: study.show_phonetic !== false,
    }) + pairsBox + '<div id="lk-dictlinks" class="we-section"></div>' + srcLine + `
    <div class="btn-row" style="margin-top:16px">
      <button class="primary-btn sm" id="lk-ai">AI 讲解</button>
      <button class="ghost-btn sm" id="lk-add">加入词库</button>
      <button class="ghost-btn sm" id="lk-detail">查看详情卡</button>
    </div>`;

    // 标签切换：一次只显示一块。
    //
    // 用委托而不是逐个绑：标签栏会被下一次查询整段替换，逐个绑必然泄漏
    // 且漏绑（新标签点不动）——这和朗读按钮那条坑是同一个道理。
    const tabsBar = box.querySelector('.entry-tabs');
    if (tabsBar) {
      tabsBar.addEventListener('click', (e) => {
        const btn = e.target.closest('.entry-tab');
        if (!btn || !btn.dataset.block) return;
        const id = btn.dataset.block;
        tabsBar.querySelectorAll('.entry-tab').forEach(b => {
          b.classList.toggle('active', b.dataset.block === id);
        });
        box.querySelectorAll('.entry-body .entry-pane').forEach(p => {
          p.classList.toggle('active', p.dataset.pane === id);
        });
      });
    }

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

  /* ---------------- 整句查询（句子也能查，且词词可点） ---------------- */

  /**
   * 查询历史栈（仿浏览器前进/后退）。
   *
   * 每次成功查询都把 `{kind, text}` 压栈；`back()` 只移动游标、不弹栈，
   * 所以「后退再点新词」时会把后面的分支截断 —— 与浏览器行为一致。
   */
  const nav = (() => {
    const stack = [];
    let cursor = -1;
    return {
      push(item) {
        const cur = stack[cursor];
        if (cur && cur.kind === item.kind && cur.text === item.text) return;
        stack.splice(cursor + 1);
        stack.push(item);
        if (stack.length > 80) stack.shift();
        cursor = stack.length - 1;
        syncBackBtn();
      },
      back() {
        if (cursor <= 0) return null;
        cursor -= 1;
        syncBackBtn();
        return stack[cursor];
      },
      canBack() { return cursor > 0; },
      reset() { stack.length = 0; cursor = -1; syncBackBtn(); },
    };
  })();

  /** 同步「← 返回」按钮的可用状态。 */
  function syncBackBtn() {
    const b = document.getElementById('lk-back');
    if (b) {
      const can = nav.canBack();
      b.disabled = !can;
      b.classList.toggle('is-disabled', !can);
    }
  }

  /** 回到栈里的上一个查询。 */
  function goBack() {
    const prev = nav.back();
    if (!prev) { U().toast('已经是最早的查询了', ''); return; }
    if (prev.kind === 'sentence') querySentence(prev.text, { fromNav: true });
    else query(prev.text, false, { fromNav: true });
  }

  /**
   * 判断输入更像「单句 / 短语」而不是一个词条。
   *
   * 用途：词典查不到时**不要直接甩「查询失败」**，而是自动改走整句翻译。
   * 这是用户截图里那个报错的根源 —— 输入是一整句英文，任何词典源都不可能命中，
   * 于是四层链路（本地词库 / 缓存 / 在线词典 / AI 兜底）全空，最后只能报失败。
   *
   * 注意：现在「AI 没开」已经**不会**单独导致失败了（见 v0.40.0 的判定重构），
   * 这里的拦截依然必要 —— 它是为了把整句引到翻译那条更合适的路上，
   * 而不是为了掩盖失败。
   */
  function looksLikeSentence(text) {
    const t = String(text || '').trim();
    if (!t) return false;
    // CJK 没有空格，靠长度与句末标点判断
    if (/[\u3040-\u30ff\u4e00-\u9fff\uac00-\ud7af]/.test(t)) {
      return t.length >= 10 || /[。！？；]/.test(t);
    }
    const tokens = t.split(/\s+/).filter(Boolean);
    if (tokens.length >= 4) return true;
    if (tokens.length >= 2 && /[.!?]/.test(t)) return true;
    return false;
  }

  /**
   * 把一句原文切成「可点词 / 分隔符」。
   *
   * 拉丁词按 `[A-Za-z][A-Za-z'’-]*` 切；日语/中文按**汉字串与假名串分开**
   * （日语里这正好大致对应「词」，比整段吞要好用得多）。
   */
  function tokenizeSentence(text) {
    const t = String(text || '');
    const re = /[A-Za-z][A-Za-z'’-]*|[\u3040-\u30ff]+|[\u4e00-\u9fff]+|[\uac00-\ud7af]+|\s+|[^\sA-Za-z\u3040-\u30ff\u4e00-\u9fff\uac00-\ud7af]+/g;
    const out = [];
    for (const piece of (t.match(re) || [])) {
      if (/^\s+$/.test(piece)) { out.push({ t: piece, word: false }); continue; }
      const isWord = /^[A-Za-z\u3040-\u30ff\u4e00-\u9fff\uac00-\ud7af]/.test(piece);
      out.push({ t: piece, word: isWord });
    }
    return out;
  }

  /** 整句翻译并渲染（每个词可点击）。 */
  async function querySentence(text, opts = {}) {
    const box = document.getElementById('lk-result');
    const input = document.getElementById('lk-input');
    if (input) input.value = text;
    const sug = document.getElementById('lk-suggest');
    if (sug) sug.innerHTML = '';

    if (!opts.fromNav) nav.push({ kind: 'sentence', text });

    const my = ++reqSeq;
    const alive = () => my === reqSeq;
    box.innerHTML = U().loadingHtml('这是一整句，正在翻译…');

    const D = window.DirPicker;
    const to = (D && D.to) || 'zh';
    let res;
    try {
      res = await API.translate(text, null, to, false);
    } catch (e) {
      if (!alive()) return;
      renderFail(box, text, `整句翻译失败：${e.message || e}`);
      return;
    }
    if (!alive()) return;
    if (!res || !res.text) { renderFail(box, text, '翻译服务没有返回结果'); return; }

    renderSentence(res, text, to);
  }

  /** 渲染整句结果：原文按词可点 + 译文 + 朗读。 */
  function renderSentence(res, srcText, toLang) {
    const box = document.getElementById('lk-result');
    const srcLang = res.from || 'en';
    const tokens = tokenizeSentence(srcText);
    const srcHtml = tokens.map(tk => (tk.word
      ? `<span class="sent-word" data-word="${U().esc(tk.t)}" title="点击查询「${U().esc(tk.t)}」">${U().esc(tk.t)}</span>`
      : U().esc(tk.t))).join('');

    const back = nav.canBack()
      ? '<button class="ghost-btn xs" id="lk-sent-back">&#8592; 返回上一个</button>'
      : '';

    box.innerHTML = `
      <div class="sent-card">
        <div class="sent-bar">
          ${back}
          <button class="ghost-btn xs speak-btn" data-speak-text="${U().esc(srcText)}"
                  data-speak-lang="${U().esc(srcLang)}" data-speak-rate="0.9"
                  title="朗读原文">&#128266; 朗读原文</button>
          <span class="sent-note">词典未收录为词条，已按整句翻译 · 点击任意单词可查词</span>
        </div>
        <div class="sent-src" data-speak-text="${U().esc(srcText)}" data-speak-lang="${U().esc(srcLang)}">${srcHtml}</div>
        <div class="sent-dst">${U().esc(res.text)}</div>
        ${res.phonetic ? `<div class="sent-phonetic">读音 ${U().esc(res.phonetic)}</div>` : ''}
        ${(res.alternatives || []).filter(a => a && a !== res.text).length
          ? `<div class="sent-alt">其他译法：${res.alternatives.filter(a => a && a !== res.text).map(U().esc).join(' · ')}</div>` : ''}
        ${res.note ? `<div class="sent-note-2">${U().esc(res.note)}</div>` : ''}
      </div>`;

    // 每个词点开查词（会压栈，所以能一路退回来）
    box.querySelectorAll('.sent-word').forEach(el => {
      el.addEventListener('click', () => query(el.dataset.word));
    });
    document.getElementById('lk-sent-back')?.addEventListener('click', goBack);
    U().speakBind(box);
    lastResult = null;
    askTarget = '';
    syncBackBtn();
  }

  /**
   * 统一的失败态渲染。
   *
   * 只有「本地词库 / 缓存 / 在线词典 / AI 兜底」**全都**没结果时才会走到这里
   * ——只要有一个词典源能返回内容，后端就会返回成功（必要时带降级提示），
   * 不会进这个页面。所以文案不再把「没启动 AI 服务器」当成失败原因。
   */
  function renderFail(box, word, msg) {
    box.innerHTML = `<div class="empty-state">
      <div class="es-icon">&#128533;</div>
      <p>未查到「${U().esc(word)}」</p>
      <p class="muted">${U().esc(msg)}</p>
      <p class="muted" style="margin-top:12px">
        只要有一个词典源返回结果就会显示成功，所以这里多半是所有源都没收录该词。
      </p>
      <div class="btn-row" style="margin-top:14px">
        <button class="ghost-btn sm" id="lk-fail-sent">改按整句翻译</button>
        <button class="ghost-btn sm" id="lk-fail-ai">用 AI 讲解</button>
      </div>
    </div>`;
    document.getElementById('lk-fail-sent')?.addEventListener('click', () => querySentence(word));
    document.getElementById('lk-fail-ai')?.addEventListener('click', () => {
      askTarget = word;
      Detail.explainInto('ai-body', word).catch(() => {});
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
    bind, query, querySentence, goBack, looksLikeSentence, loadWebResults,
    loadWordTranslation, syncLookupPlaceholder,
    initLayout, setSplit, setCollapsed, isCollapsed,
    get currentSplit() { return currentSplit(); },
    get last() { return lastResult; },
  };
})();

window.Detail = Detail;
window.Lookup = Lookup;
