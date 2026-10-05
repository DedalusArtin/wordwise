/* ============================================================
   sidebar.js —— 长挂侧边栏（需求 7）
   独立窗口，置顶、无边框、随时查词与快速背词
   ============================================================ */

const Sidebar = (() => {
  const { API } = window.WordWiseAPI;
  const U = () => window.WW;

  let currentEntry = null;
  let suggestTimer = null;
  let explaining = '';
  let unlisten = null;

  function bind() {
    const input = document.getElementById('sb-input');
    const go = document.getElementById('sb-go');

    go?.addEventListener('click', () => query(input.value));
    input?.addEventListener('keydown', (e) => {
      if (e.key === 'Enter') { e.preventDefault(); query(input.value); }
      if (e.key === 'Escape') { input.value = ''; clearSuggest(); }
    });
    input?.addEventListener('input', () => {
      clearTimeout(suggestTimer);
      const v = input.value.trim();
      if (v.length < 2) { clearSuggest(); return; }
      suggestTimer = setTimeout(() => loadSuggest(v), 340);
    });

    document.getElementById('sb-hide')?.addEventListener('click', () => API.sidebarHide());
    document.getElementById('sb-main')?.addEventListener('click', () => API.mainShow());

    document.getElementById('sb-quiz')?.addEventListener('click', quickQuiz);
    document.getElementById('sb-explain')?.addEventListener('click', () => {
      const w = currentEntry ? currentEntry.word : (input?.value || '').trim();
      if (!w) { U().toast('请先输入或查询一个单词', 'err'); return; }
      explain(w);
    });

    // 侧边栏内的相关词/变形词点击
    document.getElementById('sb-body')?.addEventListener('click', (e) => {
      const chip = e.target.closest('.rel-chip');
      if (chip && chip.dataset.word) { query(chip.dataset.word); return; }
      const inf = e.target.closest('.infl-form.clickable');
      if (inf && inf.dataset.word) { query(inf.dataset.word); return; }
      const tab = e.target.closest('[data-sbtab]');
      if (tab) switchPane(tab.dataset.sbtab);
    });
    // 发音委托（作用于整个侧边栏体）
    document.getElementById('sb-body')?.addEventListener('click', (e) => {
      const b = e.target.closest('.speak-btn');
      if (!b) return;
      e.stopPropagation();
      const host = b.closest('[data-entry-word]');
      const w = b.dataset.speakWord || (host && host.dataset.entryWord) || (currentEntry && currentEntry.word) || '';
      if (w) U().speak(w, { accent: b.dataset.speakAccent || 'us', audio: b.dataset.speakAudio || '', lang: b.dataset.speakLang || 'en' });
    });
  }

  function clearSuggest() {
    const box = document.getElementById('sb-suggest');
    if (box) box.innerHTML = '';
  }

  async function loadSuggest(q) {
    try {
      const list = await API.suggest(q);
      const box = document.getElementById('sb-suggest');
      if (!box) return;
      if (!list || !list.length) { box.innerHTML = ''; return; }
      box.innerHTML = list.slice(0, 8).map(s =>
        `<span class="suggest-chip" data-word="${U().esc(s.word)}">${U().esc(s.word)}</span>`).join('');
      box.querySelectorAll('.suggest-chip').forEach(c => {
        c.addEventListener('click', () => query(c.dataset.word));
      });
    } catch (e) { /* 静默 */ }
  }

  async function query(word) {
    word = (word || '').trim();
    if (!word) { U().toast('请输入单词', 'err'); return; }

    const input = document.getElementById('sb-input');
    if (input) input.value = word;
    clearSuggest();

    const box = document.getElementById('sb-body');
    box.innerHTML = U().loadingHtml('查询中…');

    let res;
    try {
      res = await API.lookup(word);
    } catch (e) {
      box.innerHTML = `<div class="muted" style="padding:16px;line-height:1.8">
        <p style="color:#e5484d;margin-bottom:8px">未查到该词</p>
        <p>${U().esc(e.message)}</p>
      </div>`;
      currentEntry = null;
      return;
    }

    currentEntry = res.entry;
    render(res.entry, res);
  }

  function render(entry, res) {
    const box = document.getElementById('sb-body');
    const cfg = window.App && window.App.config;
    const study = (cfg && cfg.study) || {};

    const defs = (entry.senses || []).map(s => `
      <div class="sense">
        ${s.pos ? `<div class="sense-pos">${U().esc(s.pos)}</div>` : ''}
        <div class="sense-def">${U().esc(s.definition)}
          ${study.show_examples !== false && s.examples && s.examples.length
            ? s.examples.slice(0, 2).map(ex => `
                <div class="sense-ex">${U().esc(ex.text)}
                  ${ex.translation ? `<div class="ex-zh">${U().esc(ex.translation)}</div>` : ''}
                </div>`).join('')
            : ''}
        </div>
      </div>`).join('') || '<div class="muted">暂无释义</div>';

    const ph = entry.phonetic || {};
    const phon = [ph.uk, ph.us].filter(Boolean);

    box.innerHTML = `
      <div class="sb-entry" data-entry-word="${U().esc(entry.word)}">
        <div class="we-head" style="padding-bottom:10px;margin-bottom:10px">
          <div class="we-word">${U().esc(entry.word)}</div>
          ${study.show_phonetic !== false
            ? `<div class="we-phon" style="font-size:12px">${phon.map(U().esc).join('  ')}${U().speakBtn(entry, 'us', '发音')}</div>`
            : `<div class="we-phon" style="font-size:12px">${U().speakBtn(entry, 'us', '发音')}</div>`}
          <div class="we-src" style="margin-top:8px">
            ${(entry.source || '').split('+').filter(Boolean).map(s =>
              `<span class="tag">${U().esc(U().sourceLabel(s))}</span>`).join('')}
          </div>
        </div>

        <div class="dc-tabs" style="padding:0 0 8px;margin-bottom:10px">
          <button class="dc-tab active" data-sbtab="def">释义</button>
          ${study.show_inflections !== false && entry.inflections && entry.inflections.length
            ? '<button class="dc-tab" data-sbtab="infl">变形</button>' : ''}
          ${study.show_related === true && entry.related && entry.related.length
            ? '<button class="dc-tab" data-sbtab="rel">相关</button>' : ''}
          <button class="dc-tab" data-sbtab="ai">AI 讲解</button>
        </div>

        <div class="dc-pane active" data-sbpane="def">
          ${defs}
          ${study.show_mnemonic !== false && entry.mnemonic
            ? `<div class="mnemonic-box" style="margin-top:10px;font-size:12.5px">${U().esc(entry.mnemonic)}</div>` : ''}
        </div>

        <div class="dc-pane" data-sbpane="infl">
          ${(entry.inflections || []).map(i =>
            `<div class="infl-item" style="margin-bottom:6px">
              <span class="infl-label">${U().esc(i.label || '形式')}</span>
              <span class="infl-form clickable" data-word="${U().esc(i.form)}" title="点击查询 ${U().esc(i.form)}">${U().esc(i.form)}</span></div>`).join('')
            || '<div class="muted">暂无变形信息</div>'}
        </div>

        <div class="dc-pane" data-sbpane="rel">
          <div class="rel-list">
            ${(entry.related || []).map(r =>
              `<span class="rel-chip" data-word="${U().esc(r)}">${U().esc(r)}</span>`).join('')
              || '<div class="muted">暂无相关词</div>'}
          </div>
        </div>

        <div class="dc-pane" data-sbpane="ai" id="sb-ai-pane">
          <p class="muted">点击底部「AI 讲解」让本地模型讲解这个单词。</p>
        </div>

        <div class="btn-row" style="margin-top:14px">
          <button class="ghost-btn xs" id="sb-add">加入词库</button>
          <button class="ghost-btn xs" id="sb-open-main">在主界面查看</button>
        </div>
      </div>`;

    document.getElementById('sb-add')?.addEventListener('click', async () => {
      try {
        await API.addWord(entry);
        U().toast('已加入词库', 'ok');
      } catch (e) { U().toast(e.message, 'err'); }
    });

    document.getElementById('sb-open-main')?.addEventListener('click', async () => {
      await API.mainShow();
    });
  }

  function switchPane(name) {
    document.querySelectorAll('#sb-body .dc-tab').forEach(t => {
      t.classList.toggle('active', t.dataset.sbtab === name);
    });
    document.querySelectorAll('#sb-body .dc-pane').forEach(p => {
      p.classList.toggle('active', p.dataset.sbpane === name);
    });
  }

  async function explain(word) {
    switchPane('ai');
    const pane = document.getElementById('sb-ai-pane');
    if (!pane) return;

    explaining = '';
    pane.innerHTML = '<div class="loading" style="padding:16px"><div class="spinner"></div><span>调用本地模型…</span></div>';

    if (unlisten) { try { unlisten(); } catch (e) {} }
    unlisten = await window.WordWiseAPI.listen('explain://delta', (p) => {
      if (!p || p.word !== word) return;
      explaining += p.delta || '';
      pane.innerHTML = U().renderMarkdown(explaining) + '<span class="cursor"></span>';
      pane.scrollTop = pane.scrollHeight;
    });

    try {
      // ★ 后端返回 ExplainResult 结构体（兼容旧的字符串形态）
      const raw = await API.aiExplain(word);
      const res = (raw && typeof raw === 'object')
        ? raw
        : { text: (typeof raw === 'string' && raw) || explaining, original: explaining,
            lang: '', translated: false };
      pane.innerHTML =
        `<div class="ai-lang-bar"><span class="ai-lang-meta">${
          U().esc(res.translated
            ? `已自动翻译为 ${window.WordWiseAPI.explainLangLabel(res.lang)}`
            : `讲解语言：${window.WordWiseAPI.explainLangLabel(res.lang)}`)
        }</span></div>` +
        U().renderMarkdown(res.text);
    } catch (e) {
      pane.innerHTML = `<div class="muted" style="color:#e5484d;line-height:1.8">讲解失败：${U().esc(e.message)}</div>`;
    } finally {
      if (unlisten) { try { unlisten(); } catch (err) {} unlisten = null; }
    }
  }

  /** 侧边栏快速背词：只做一问一答，轻量不打断。 */
  async function quickQuiz() {
    const box = document.getElementById('sb-body');
    box.innerHTML = U().loadingHtml('准备题目…');

    try {
      await API.startSession('en_to_zh', 1, false);
      const card = await API.currentQuestion();
      if (!card) {
        box.innerHTML = '<div class="muted" style="padding:16px">词库为空，请先导入单词</div>';
        return;
      }
      renderQuickCard(card);
    } catch (e) {
      box.innerHTML = `<div class="muted" style="padding:16px;line-height:1.8">
        <p style="color:#e5484d">${U().esc(e.message)}</p></div>`;
    }
  }

  function renderQuickCard(card) {
    const box = document.getElementById('sb-body');
    const opts = [...(card.options || [])];
    if (!opts.includes(card.answer)) opts.push(card.answer);
    for (let i = opts.length - 1; i > 0; i--) {
      const j = Math.floor(Math.random() * (i + 1));
      [opts[i], opts[j]] = [opts[j], opts[i]];
    }

    box.innerHTML = `
      <div style="padding:4px">
        <div class="qc-label" style="text-align:left;margin-bottom:10px">
          ${card.is_leech ? '<span class="tag orange">强化记忆</span> ' : ''}选择正确释义
        </div>
        <div style="font-size:26px;font-weight:700;margin-bottom:16px;word-break:break-word">${U().esc(card.prompt)}</div>
        <div style="display:grid;gap:8px">
          ${opts.map((o, i) => `
            <button class="opt-btn" style="padding:11px 14px;font-size:13.5px" data-v="${encodeURIComponent(o)}">
              <span class="opt-key">${String.fromCharCode(65 + i)}</span>
              <span>${U().esc(o)}</span>
            </button>`).join('')}
        </div>
        <div id="sb-fb" class="muted" style="margin-top:12px;font-size:12.5px"></div>
        <div class="btn-row" style="margin-top:12px">
          <button class="ghost-btn xs" id="sb-next">再来一题</button>
          <button class="ghost-btn xs" id="sb-goto-main">完整模式</button>
        </div>
      </div>`;

    box.querySelectorAll('.opt-btn').forEach(btn => {
      btn.addEventListener('click', async () => {
        const v = decodeURIComponent(btn.dataset.v);
        const correct = v === card.answer;
        box.querySelectorAll('.opt-btn').forEach(b => {
          b.disabled = true;
          const bv = decodeURIComponent(b.dataset.v || '');
          if (bv === card.answer) b.classList.add('correct');
          else if (b === btn) b.classList.add('wrong');
        });

        const fb = document.getElementById('sb-fb');
        if (fb) {
          fb.innerHTML = correct
            ? '<span style="color:#17a673">✓ 答对了</span>'
            : `<span style="color:#e5484d">✗ 正确答案：${U().esc(card.answer)}</span>`;
        }

        try {
          await API.submitAnswer(card.entry.word, correct ? 'good' : 'wrong', 0);
        } catch (e) { /* 忽略 */ }

        if (!correct) {
          setTimeout(() => { renderDetailInSidebar(card.entry); }, 700);
        }
      });
    });

    document.getElementById('sb-next')?.addEventListener('click', quickQuiz);
    document.getElementById('sb-goto-main')?.addEventListener('click', async () => {
      await API.mainShow();
      if (window.Pages) window.Pages.go('study');
    });
  }

  function renderDetailInSidebar(entry) {
    currentEntry = entry;
    render(entry, null);
  }

  return { bind, query, explain, quickQuiz, get current() { return currentEntry; } };
})();

window.Sidebar = Sidebar;
