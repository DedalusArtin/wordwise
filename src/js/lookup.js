/* ============================================================
   lookup.js —— 查词与 AI 讲解（需求 2、6）
   - 联网多源聚合查词，带联想
   - AI 流式讲解（打字机效果）
   - 单词详情卡（仿有道词典）
   ============================================================ */

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

    // 音标
    const ph = entry.phonetic || {};
    const phItems = [];
    if (ph.uk) phItems.push(`<span><span class="phon-tag">英</span>${U().esc(ph.uk)}</span>`);
    if (ph.us) phItems.push(`<span><span class="phon-tag">美</span>${U().esc(ph.us)}</span>`);
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
       <span class="infl-form">${U().esc(i.form)}</span></div>`).join('') + '</div>';
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
      if (e.key === 'Escape') close();
    });

    document.querySelectorAll('#dc-tabs .dc-tab').forEach(t => {
      t.addEventListener('click', () => U().switchDetailTab(t.dataset.tab));
    });

    // 相关词点击 → 直接查词
    document.getElementById('dc-pane-rel')?.addEventListener('click', (e) => {
      const chip = e.target.closest('.rel-chip');
      if (chip) Lookup.query(chip.dataset.word);
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
      const full = await API.aiExplain(word, null, question);
      const text = full || explainBuffer;
      box.innerHTML = U().renderMarkdown(text);
    } catch (e) {
      box.innerHTML = `<div class="muted" style="color:#e5484d">讲解失败：${U().esc(e.message)}</div>`;
    } finally {
      if (unlistenDelta) { try { unlistenDelta(); } catch (err) {} unlistenDelta = null; }
    }
  }

  return { open, close, bind, explainInto, get current() { return current; } };
})();

/* ---------------- 查词页 ---------------- */

const Lookup = (() => {
  const { API } = window.WordWiseAPI;
  const U = () => window.WW;

  let lastResult = null;
  let suggestTimer = null;

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

    // AI 追问
    const ask = document.getElementById('ai-send');
    const aiInput = document.getElementById('ai-input');
    ask?.addEventListener('click', () => sendQuestion());
    aiInput?.addEventListener('keydown', (e) => {
      if (e.key === 'Enter') { e.preventDefault(); sendQuestion(); }
    });

    // 最近搜索
    loadRecent();
  }

  let askTarget = '';

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
    box.innerHTML = U().loadingHtml('正在联网查询…');

    let res;
    try {
      res = await API.lookup(word, null, forceRefresh);
    } catch (e) {
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

    lastResult = res;
    askTarget = res.word;

    let study = {};
    try {
      const cfg = await API.getConfig();
      study = (cfg && cfg.study) || {};
    } catch (e) {}

    const srcLine = res.sources && res.sources.length
      ? `<div class="muted" style="margin-top:10px;font-size:12px">数据来源：${res.sources.map(U().esc).join(' · ')}${res.from_cache ? '（缓存）' : ''}${res.from_llm ? '（本地模型生成）' : ''}</div>`
      : '';

    box.innerHTML = U().renderEntry(res.entry, {
      showInflections: study.show_inflections !== false,
      showExamples: study.show_examples !== false,
      showRelated: study.show_related === true,
      showMnemonic: study.show_mnemonic !== false,
      showPhonetic: study.show_phonetic !== false,
    }) + srcLine + `
    <div class="btn-row" style="margin-top:16px">
      <button class="primary-btn sm" id="lk-ai">AI 讲解</button>
      <button class="ghost-btn sm" id="lk-add">加入词库</button>
      <button class="ghost-btn sm" id="lk-detail">查看详情卡</button>
    </div>`;

    box.querySelectorAll('.rel-chip').forEach(c => {
      c.addEventListener('click', () => query(c.dataset.word));
    });
    document.getElementById('lk-add')?.addEventListener('click', async () => {
      try {
        await API.addWord(res.entry);
        U().toast('已加入词库', 'ok');
      } catch (e) { U().toast(e.message, 'err'); }
    });
    document.getElementById('lk-detail')?.addEventListener('click', () => Detail.open(res.entry));
    document.getElementById('lk-ai')?.addEventListener('click', () => {
      Detail.explainInto('ai-body', res.word);
    });

    // 自动开始讲解（若开启）
    if (study.ai_explain !== false) {
      Detail.explainInto('ai-body', res.word);
    }
  }

  return { bind, query, get last() { return lastResult; } };
})();

window.Detail = Detail;
window.Lookup = Lookup;
