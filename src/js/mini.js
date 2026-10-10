/* ============================================================
   mini.js —— 迷你悬浮窗（桌面小窗背词）

   参照 ToastFish / 摸鱼背词那一类「桌面悬浮背词条」：常驻屏幕角落、
   置顶、无边框，一屏只做一件事 —— 出一个词，等用户说「认识 / 不认识」，
   然后自动上下一个，并把下次复习间隔和今日进度留在窗面上。

   几个关键取舍：

   - **不新建任何数据通道**。主窗口与迷你窗是同一个进程里的两个 webview，
     共用同一个 `AppState` 和同一个 SQLite 文件，词库、SRS 进度天然一致；
     所谓「共享学习进度」在这里是**物理事实**，不需要写同步逻辑。

   - **必须传 `kind: 'mini'`**。会话槽位按 kind 选（见 `state.rs`），
     传漏了就会落回主窗口的背诵槽 —— 于是「一开小窗，主窗口正在背的
     那一轮就被整体覆写」。侧边栏以前踩的正是这个坑。

   - **偏好存 localStorage 而不是配置表**：尺寸/透明度/置顶是**这个窗口**
     的观感偏好，跟用户数据无关，也不该被「保存设置」整份覆盖。

   - 「认识 / 不认识」就是 SRS 的 good / wrong 两档，不加中间态：
     小窗的使用场景是快速过词，多一个档位就多一次犹豫。
   ============================================================ */

const Mini = (() => {
  const { API } = window.WordWiseAPI;
  const U = () => window.WW;

  const KEY = {
    alpha: 'ww.mini.alpha',
    size: 'ww.mini.size',
    top: 'ww.mini.top',
  };

  /** 三档尺寸（逻辑像素；下界与 `windows.rs` 的 min_inner_size 对齐）。 */
  const SIZES = [[300, 170], [360, 220], [460, 300]];
  /** 四档不透明度：100% / 90% / 78% / 66%。 */
  const ALPHAS = [1, 0.9, 0.78, 0.66];

  const $ = (id) => document.getElementById(id);

  let started = false;
  let card = null;
  let shownAt = 0;
  let busy = false;
  let sizeIdx = 1;
  let alphaIdx = 0;
  let pinned = true;

  /* ---------------- 偏好 ---------------- */

  function loadPrefs() {
    try {
      const si = parseInt(localStorage.getItem(KEY.size) || '1', 10);
      if (!Number.isNaN(si)) sizeIdx = Math.min(SIZES.length - 1, Math.max(0, si));
      const ai = parseInt(localStorage.getItem(KEY.alpha) || '0', 10);
      if (!Number.isNaN(ai)) alphaIdx = Math.min(ALPHAS.length - 1, Math.max(0, ai));
      pinned = localStorage.getItem(KEY.top) !== '0';
    } catch (e) { /* 隐私模式下读不到就用默认值 */ }
    applyPrefs();
  }

  function savePrefs() {
    try {
      localStorage.setItem(KEY.size, String(sizeIdx));
      localStorage.setItem(KEY.alpha, String(alphaIdx));
      localStorage.setItem(KEY.top, pinned ? '1' : '0');
    } catch (e) { /* 存不了不影响用 */ }
  }

  /** 透明度走 CSS：比窗口级透明更稳，也不会把文字渲染搞花。 */
  function applyPrefs() {
    if (document.body) document.body.style.opacity = String(ALPHAS[alphaIdx] ?? 1);
    const pin = $('mn-pin');
    if (pin) pin.classList.toggle('off', !pinned);
  }

  /* ---------------- 出题与渲染 ---------------- */

  async function ensureSession() {
    if (started) return;
    await API.startSession('en_to_zh', 200, false, null, null, 'mini');
    started = true;
  }

  async function next() {
    const defEl = $('mn-def');
    try {
      await ensureSession();
      let c = await API.currentQuestion(null, 'mini');
      // 一轮背完：重开一轮再取。只重试一次，绝不在这里转圈。
      if (!c) {
        started = false;
        await ensureSession();
        c = await API.currentQuestion(null, 'mini');
      }
      if (!c) {
        card = null;
        if ($('mn-word')) $('mn-word').textContent = '—';
        if (defEl) defEl.textContent = '词库里还没有可背的词。先在主窗口导入或下载一本词库。';
        if ($('mn-phon')) $('mn-phon').innerHTML = '';
        if ($('mn-meta')) $('mn-meta').textContent = '';
        return;
      }
      card = c;
      shownAt = Date.now();
      render(c);
      refreshMeta(c.entry.word, c.entry.lang);
      // 出题卡片带的词条常常是「占位 / 单薄」的（只有词名和一条释义）——
      // 小窗信息本来就少，这里再按词查一次本地库，把更完整的词条换上
      // （含例句、变形、相关词）。纯本地读，不联网，代价可以忽略。
      void refreshEntry(c.entry.word, c.entry.lang);
    } catch (e) {
      card = null;
      if (defEl) defEl.textContent = `取词失败：${e && e.message ? e.message : e}`;
    }
  }

  let curWord = '';

  function render(c) {
    const entry = (c && c.entry) || null;
    const word = (entry && entry.word) || '';
    curWord = word;
    if ($('mn-word')) $('mn-word').textContent = word || '—';

    renderPhon(entry, word);
    renderDefs(entry, c);
    renderExamples(entry, word);
    renderTags(entry);
  }

  /**
   * 音标行 + 发音按钮。
   *
   * ★ 必须把**词 / 语言 / 音频**写到容器上（`data-entry-word` 等）。
   *   发音按钮的事件委托是「按钮 → 最近的 [data-entry-word] 容器 → 最近的
   *   [data-speak-text] 容器」三级回退取词；容器上什么都没写的话，
   *   委托取到空词直接 return —— 表现就是**小窗的喇叭点了毫无反应**
   *   （详情卡当初踩的是同一个坑）。按钮自己也可带 data-speak-word，
   *   这里两条路都留上，任一条都能取到词。
   */
  function renderPhon(entry, word) {
    const phon = $('mn-phon');
    if (!phon) return;
    if (!entry) {
      phon.innerHTML = '';
      return;
    }
    // 音标 + 发音按钮走统一实现（语言标注、斜杠规则、IPA 字体只有一份）
    const html = U().phoneticHtml(entry, { speak: true });
    phon.innerHTML = html || U().speakBtn(entry, 'us', '发音');
    phon.dataset.entryWord = word || '';
    phon.dataset.speakLang = entry.lang || 'en';
    phon.dataset.speakAudio = (entry.phonetic && entry.phonetic.audio) || '';
    // 兜底：把词直接挂到每个按钮上（委托的第一优先来源就是它）
    phon.querySelectorAll('.speak-btn').forEach((b) => {
      b.dataset.speakWord = word || '';
      if (!b.dataset.speakLang) b.dataset.speakLang = entry.lang || 'en';
    });
  }

  /** 释义：列**多条义项**（最多 3 条），母语（中文）释义优先。 */
  function renderDefs(entry, c) {
    const el = $('mn-def');
    if (!el) return;
    const senses = ((entry && entry.senses) || []).filter((s) => (s.definition || '').trim());
    let list = [];
    if (senses.length) {
      // 背英语词时中文释义最有用：有中文义项就只列中文那组，
      // 否则退回原文义项（日语/法语词库就靠这条）
      const [local, native] = U().splitSensesByScript(senses);
      list = (local.length ? local : native).slice(0, 3);
    } else if (c && c.answer) {
      list = [{ pos: '', definition: c.answer }];
    }
    if (!list.length) {
      el.innerHTML = '<span class="muted">这个词还没有释义 —— 点右上角「补全」让 AI 生成完整词条。</span>';
      return;
    }
    const jump = senses.length > 3
      ? `<span class="mn-more muted">…共 ${senses.length} 个义项</span>` : '';
    el.innerHTML = list.map((s) => `<div class="mn-sense">${
      s.pos ? `<i class="mn-pos">${U().esc(s.pos)}</i>` : ''
    }<span class="mn-def-text">${U().esc(s.definition)}</span></div>`).join('') + jump;
  }

  /** 例句：最多 2 条，中英对照，每句可单独朗读。 */
  function renderExamples(entry, word) {
    const el = $('mn-ex');
    if (!el) return;
    const exs = entry ? U().collectExamples(entry).filter((x) => (x.text || '').trim()).slice(0, 2) : [];
    if (!exs.length) {
      el.innerHTML = '';
      return;
    }
    el.innerHTML = exs.map((ex) => `
      <div class="mn-ex-item">
        <div class="mn-ex-head">
          <span class="mn-ex-en">${U().esc(ex.text)}</span>
          <button class="speak-btn" type="button" title="朗读例句"
                  data-speak-word="${U().esc(ex.text)}"
                  data-speak-lang="${U().esc((entry && entry.lang) || 'en')}"></button>
        </div>
        ${ex.translation ? `<div class="mn-ex-zh">${U().esc(ex.translation)}</div>` : ''}
      </div>`).join('');
    el.dataset.speakWord = word || '';
  }

  /** 词组 / 变形：有数据才显示（变形取 label+form，相关词取前 6 个）。 */
  function renderTags(entry) {
    const el = $('mn-tags');
    if (!el || !entry) return;
    const chips = [];
    for (const i of (entry.inflections || []).slice(0, 4)) {
      if (i && i.form) chips.push(`<span class="mn-tag"><i>${U().esc(i.label || '变形')}</i>${U().esc(i.form)}</span>`);
    }
    for (const r of (entry.related || []).slice(0, 6)) {
      if (r) chips.push(`<span class="mn-tag mn-tag-rel">${U().esc(r)}</span>`);
    }
    el.innerHTML = chips.length
      ? `<div class="mn-tags-title muted">词组 / 变形</div><div class="mn-tags-row">${chips.join('')}</div>`
      : '';
  }

  /** 复习间隔 / 熟练度：异步补，不挡出题。 */
  async function refreshMeta(word, lang) {
    const el = $('mn-meta');
    if (!el || !word) return;
    el.textContent = '';
    try {
      const st = await API.wordState(word, lang || null);
      if (!st) {
        el.textContent = '还没背过';
        return;
      }
      const acc = st.state && (st.state.correct_count + st.state.wrong_count) > 0
        ? Math.round((st.state.correct_count / (st.state.correct_count + st.state.wrong_count)) * 100)
        : 0;
      el.textContent = `熟练度 ${st.state ? st.state.mastery : 0}% · 正确率 ${acc}% · 下次复习 ${st.due_text || '—'}`;
    } catch (e) { /* 状态取不到就算了，不干扰背词 */ }
  }

  /**
   * 用本地库里更完整的词条替换当前展示（例句/变形/相关词都靠它）。
   *
   * 只在「新的更全」时才换 —— 出题卡片里的 entry 可能带着会话相关的字段，
   * 不能被一条更瘦的库记录覆盖回去。
   */
  async function refreshEntry(word, lang) {
    if (!word || !card || !card.entry || card.entry.word !== word) return;
    if (typeof API.getWord !== 'function') return;
    try {
      const full = await API.getWord(word, lang || null);
      // 期间可能已经换词了，或者新的反而更瘦 → 都不动
      if (!full || !card || (card.entry || {}).word !== word) return;
      const before = (card.entry.senses || []).length;
      const after = (full.senses || []).length;
      if (after > before) {
        card.entry = full;
        render(card);
      }
    } catch (e) { /* 补不上就照现状显示 */ }
  }

  /** 让 AI/联网把这条词条补全（例句、变形、相关词），补完自动重渲染。 */
  async function enrichCur() {
    const word = curWord;
    if (!word) return;
    const btn = $('mn-enrich');
    if (btn) { btn.disabled = true; btn.textContent = '补全中…'; }
    try {
      const r = await API.enrichWord(word, (card && card.entry && card.entry.lang) || null);
      const ok = !!(r && (r.enriched === undefined || r.enriched));
      if (ok) {
        U().toast(`已补全「${word}」的词条`, 'ok');
      } else {
        U().toast('没补出新内容（可先在设置页部署本地大模型）', 'err');
      }
      await refreshEntry(word, (card && card.entry && card.entry.lang) || null);
      render(card || {});
    } catch (e) {
      U().toast((e && e.message) || '补全失败', 'err');
    } finally {
      if (btn) { btn.disabled = false; btn.textContent = '补全'; }
    }
  }

  async function refreshProgress() {
    const el = $('mn-progress');
    if (!el) return;
    try {
      const s = await API.stats(null, null);
      el.textContent = `今日 ${s.reviewed_today || 0} · 待复习 ${s.due_today || 0}`;
    } catch (e) {
      el.textContent = '';
    }
  }

  /* ---------------- 认识 / 不认识 ---------------- */

  async function answer(ok) {
    if (!card || busy) return;
    busy = true;
    const word = card.entry ? card.entry.word : '';
    const ms = Math.max(0, Date.now() - shownAt);
    const meta = $('mn-meta');
    try {
      const res = await API.submitAnswer(word, ok ? 'good' : 'wrong', ms, null, 'mini');
      const due = res && res.schedule ? res.schedule.due_text : '';
      if (meta) meta.textContent = `${ok ? '认识' : '不认识'} · 下次复习 ${due || '很快'}`;
      refreshProgress();
      if (res && res.became_mastered) U().toast(`「${word}」已掌握`, 'ok');
    } catch (e) {
      U().toast(e && e.message ? e.message : '提交失败', 'err');
    } finally {
      busy = false;
      // 留一点时间看反馈（间隔 / 熟��度），再上下一词
      setTimeout(() => { void next(); }, 520);
    }
  }

  /* ---------------- 窗口偏好交互 ---------------- */

  async function cycleSize() {
    sizeIdx = (sizeIdx + 1) % SIZES.length;
    savePrefs();
    try {
      await API.miniSetSize(SIZES[sizeIdx][0], SIZES[sizeIdx][1]);
    } catch (e) { /* 窗口命令在非 Tauri 环境下不存在，忽略 */ }
    U().toast(`尺寸 ${SIZES[sizeIdx][0]}×${SIZES[sizeIdx][1]}`, 'ok');
  }

  function cycleAlpha() {
    alphaIdx = (alphaIdx + 1) % ALPHAS.length;
    savePrefs();
    applyPrefs();
    U().toast(`不透明度 ${Math.round((ALPHAS[alphaIdx] ?? 1) * 100)}%`, 'ok');
  }

  async function togglePin() {
    pinned = !pinned;
    savePrefs();
    applyPrefs();
    try {
      await API.miniSetAlwaysOnTop(pinned);
    } catch (e) { /* 同上 */ }
    U().toast(pinned ? '已置顶' : '已取消置顶', 'ok');
  }

  /* ---------------- 绑定 ---------------- */

  function bind() {
    loadPrefs();
    U().speakBind(document.getElementById('mini-shell') || document.body);

    $('mn-close')?.addEventListener('click', () => {
      API.miniHide().catch(() => {});
    });
    $('mn-main')?.addEventListener('click', () => {
      API.mainShow().catch(() => {});
    });
    $('mn-know')?.addEventListener('click', () => { void answer(true); });
    $('mn-unknown')?.addEventListener('click', () => { void answer(false); });
    $('mn-size')?.addEventListener('click', () => { void cycleSize(); });
    $('mn-alpha')?.addEventListener('click', cycleAlpha);
    $('mn-pin')?.addEventListener('click', () => { void togglePin(); });
    $('mn-enrich')?.addEventListener('click', () => { void enrichCur(); });

    // 后台增强完成（设置页/其它入口触发的也一样）→ 当前词的词条变厚了就换上来
    try {
      API.onEnriched?.((p) => {
        const w = p && (p.word || p.word_);
        if (w && w === curWord) void refreshEntry(w, (card && card.entry && card.entry.lang) || null);
      });
    } catch (e) { /* 没这个事件也不影响 */ }

    // 键盘：空格/回车 = 认识，Backspace = 不认识，Esc = 隐藏。
    // 小窗没有输入框，所以不需要 isTypingTarget 那种守卫。
    document.addEventListener('keydown', (e) => {
      if (e.key === 'Enter' || e.key === ' ') {
        e.preventDefault();
        void answer(true);
      } else if (e.key === 'Backspace') {
        e.preventDefault();
        void answer(false);
      } else if (e.key === 'Escape') {
        API.miniHide().catch(() => {});
      }
    });
  }

  return {
    bind,
    next,
    refreshProgress,
    get card() { return card; },
  };
})();

window.Mini = Mini;
