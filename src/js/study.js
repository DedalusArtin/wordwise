/* ============================================================
   study.js —— 背诵引擎（需求 1、3、4、5）
   - 六种模式：看英选中 / 看中选英 / 拼写 / 例句选义 / 例句识词 / 听音拼写
   - 按后端下发的顺序出题（遗忘曲线排程）
   - 答对自动发音（需求 4）
   - 答错自动弹出详情卡（仿有道词典）
   - 记录每题耗时，供 SRS 判定难度
   - 可按词库出题（需求 5）
   ============================================================ */

const Study = (() => {
  const { API } = window.WordWiseAPI;
  const U = () => window.WW;

  // 后端 QuizMode 使用 #[serde(rename_all = "snake_case")]，
  // 因此前端的短横线 id 与后端枚举名一一对应，可直接透传。
  const MODE_TO_BACKEND = {
    en_to_zh: 'en_to_zh',
    zh_to_en: 'zh_to_en',
    spelling: 'spelling',
    ex_to_zh: 'ex_to_zh',
    ex_pick_word: 'ex_pick_word',
    listen_spell: 'listen_spell',
  };
  const toBackend = (mode) => MODE_TO_BACKEND[mode] || 'en_to_zh';

  /** 各模式的界面文案与行为 */
  const MODE_META = {
    en_to_zh: { label: '请选择正确的中文释义', typing: false, example: false, audio: false },
    zh_to_en: { label: '请选择正确的英文单词', typing: false, example: false, audio: false },
    spelling: { label: '看释义，拼出这个单词', typing: true, example: false, audio: false },
    ex_to_zh: { label: '看例句，选择正确的释义', typing: false, example: true, audio: false },
    ex_pick_word: { label: '看例句，选出正确的单词', typing: false, example: true, audio: false },
    listen_spell: { label: '听发音，拼出这个单词', typing: true, example: false, audio: true },
  };

  const state = {
    mode: 'en_to_zh',
    running: false,
    card: null,          // 当前题目
    startTs: 0,          // 本题开始时间
    answered: false,
    config: null,
    bookId: '',          // 指定词库（空 = 全库）
    hintLevel: 0,        // 拼写提示等级
    learnedCount: 0,     // 本轮已答对计数（用于自动发音）
  };

  /* ---------------- 启动与结束 ---------------- */

  async function ensureConfig() {
    if (!state.config) {
      try { state.config = await API.getConfig(); } catch (e) { state.config = null; }
    }
    return state.config;
  }

  function setMode(mode) {
    state.mode = mode;
    document.querySelectorAll('#mode-seg .seg-btn').forEach(b => {
      b.classList.toggle('active', b.dataset.mode === mode);
    });
  }

  /** 同步词库下拉选择（多个 select 保持一致）。 */
  function setBook(bookId) {
    state.bookId = bookId || '';
    ['study-book', 'study-book2'].forEach(id => {
      const el = document.getElementById(id);
      if (el) el.value = state.bookId;
    });
  }

  /** 从词库页跳进来：直接按该词库开始。 */
  async function startWithBook(bookId) {
    setBook(bookId);
    if (window.Pages && window.Pages.go) window.Pages.go('study');
    await start();
  }

  async function start(opts = {}) {
    const cfg = await ensureConfig();
    const size = opts.size
      || parseInt(document.getElementById('opt-batch')?.value, 10)
      || (cfg && cfg.study && cfg.study.batch_size)
      || 20;
    const leechOnly = opts.leechOnly !== undefined
      ? opts.leechOnly
      : !!document.getElementById('opt-leech-only')?.checked;

    const bookId = opts.bookId !== undefined ? opts.bookId : state.bookId;
    const backendMode = toBackend(state.mode);

    let info;
    try {
      if (bookId) {
        info = await API.startBookSession(bookId, backendMode, size);
      } else {
        info = await API.startSession(backendMode, size, leechOnly);
      }
    } catch (e) {
      U().toast(e.message, 'err');
      return;
    }

    state.running = true;
    state.learnedCount = 0;
    document.getElementById('study-idle').classList.add('hidden');
    document.getElementById('study-run').classList.remove('hidden');
    updateCounters(info.correct, info.wrong);
    updateProgress(0, info.total);
    await nextQuestion();
  }

  async function end() {
    try { await API.endSession(); } catch (e) { /* 忽略 */ }
    state.running = false;
    state.card = null;
    document.getElementById('study-idle').classList.remove('hidden');
    document.getElementById('study-run').classList.add('hidden');
    if (window.Pages && window.Pages.refreshStudy) window.Pages.refreshStudy();
  }

  /* ---------------- 出题 ---------------- */

  function meta() {
    return MODE_META[state.mode] || MODE_META.en_to_zh;
  }

  async function nextQuestion() {
    state.answered = false;
    state.hintLevel = 0;
    let card;
    try {
      // 进阶模式用专用命令，保证例句/拼写字段齐全
      const m = meta();
      if (m.typing || m.example) {
        card = await API.buildAdvancedCard(toBackend(state.mode));
        // 后端无会话时退回普通接口
        if (!card) card = await API.currentQuestion();
      } else {
        card = await API.currentQuestion();
      }
    } catch (e) {
      U().toast(e.message, 'err');
      return end();
    }

    if (!card) {
      U().toast('本轮已完成，休息一下吧', 'ok');
      return end();
    }

    state.card = card;
    state.startTs = Date.now();
    renderQuestion(card);
  }

  function renderQuestion(card) {
    const cfg = state.config;
    const study = (cfg && cfg.study) || {};
    const m = meta();

    document.getElementById('q-label').textContent = m.label;

    // 题面：例句模式展示挖空例句；拼写模式展示释义
    const promptEl = document.getElementById('q-prompt');
    if (m.example && card.example_masked) {
      promptEl.innerHTML = `<span class="ex-prompt">${U().esc(card.example_masked)}</span>`;
    } else {
      promptEl.textContent = card.prompt;
    }

    // 音标：仅「看英选中」显示，且听音拼写时隐藏
    const phonEl = document.getElementById('q-phonetic');
    phonEl.innerHTML = '';
    if (state.mode === 'en_to_zh' && study.show_phonetic !== false && card.entry.phonetic) {
      const p = card.entry.phonetic;
      phonEl.textContent = [p.uk, p.us].filter(Boolean).join('  ');
    } else if (state.mode === 'listen_spell') {
      // 听音模式：放一个「播放」按钮
      phonEl.innerHTML = `<button class="icon-btn speak-btn big" id="q-play" title="播放发音">&#128266; 播放</button>`;
      const play = document.getElementById('q-play');
      if (play) play.addEventListener('click', () => speakCurrent());
    }

    document.getElementById('qc-leech').classList.toggle('hidden', !card.is_leech);

    const box = document.getElementById('q-options');
    const actions = document.getElementById('q-actions');
    document.getElementById('q-feedback').classList.add('hidden');
    actions.innerHTML = '';

    if (m.typing) {
      renderTypingInput(box, actions, card, m);
    } else {
      renderOptions(box, card);
    }

    // 听音拼写：进入题目后自动读一遍
    if (state.mode === 'listen_spell') {
      setTimeout(() => speakCurrent(), 260);
    }
  }

  function renderOptions(box, card) {
    const opts = [...(card.options || [])];
    if (!opts.includes(card.answer)) opts.push(card.answer);
    shuffle(opts);

    box.innerHTML = opts.map((o, i) => `
      <button class="opt-btn" data-value="${encodeURIComponent(o)}">
        <span class="opt-key">${String.fromCharCode(65 + i)}</span>
        <span>${U().esc(o)}</span>
      </button>
    `).join('');

    box.querySelectorAll('.opt-btn').forEach(btn => {
      btn.addEventListener('click', () => onChoose(btn, decodeURIComponent(btn.dataset.value)));
    });

    if (opts.length < 2) {
      document.getElementById('q-actions').innerHTML =
        '<button class="grade-btn g-good" id="btn-reveal">显示答案</button>';
      document.getElementById('btn-reveal').onclick = () => onChoose(null, null, true);
    }
  }

  function renderTypingInput(box, actions, card, m) {
    const hint = card.spell_hint || card.answer.replace(/./g, '_').split('').join(' ');
    box.innerHTML = `
      <div class="spell-wrap">
        <div class="spell-hint" id="spell-hint">${U().esc(hint)}</div>
        <input type="text" id="spell-input" class="spell-input"
          placeholder="在此输入单词…" autocomplete="off" autocapitalize="off"
          spellcheck="false" ${m.audio ? 'autofocus' : ''} />
        <div class="spell-tip muted" id="spell-tip">按 Enter 提交</div>
      </div>`;
    actions.innerHTML = `
      <button class="primary-btn" id="spell-submit">提交</button>
      <button class="ghost-btn" id="spell-hintbtn">提示</button>
      <button class="ghost-btn" id="spell-skip">跳过</button>`;

    const input = document.getElementById('spell-input');
    input?.addEventListener('keydown', (e) => {
      if (e.key === 'Enter') { e.preventDefault(); submitSpelling(); }
    });
    document.getElementById('spell-submit')?.addEventListener('click', submitSpelling);
    document.getElementById('spell-hintbtn')?.addEventListener('click', showMoreHint);
    document.getElementById('spell-skip')?.addEventListener('click', () => onChoose(null, null, true));
    input?.focus();
  }

  async function showMoreHint() {
    if (state.answered || !state.card) return;
    state.hintLevel = Math.min(3, state.hintLevel + 1);
    try {
      const h = await API.spellHint(state.card.answer, state.hintLevel);
      document.getElementById('spell-hint').textContent = h;
      const tip = document.getElementById('spell-tip');
      if (tip) tip.textContent = state.hintLevel >= 3
        ? '已给出首尾字母，再想想'
        : `已提示 ${state.hintLevel} 个首字母`;
    } catch (e) { /* 忽略 */ }
  }

  async function submitSpelling() {
    if (state.answered || !state.card) return;
    const input = document.getElementById('spell-input');
    const val = (input?.value || '').trim();
    if (!val) { U().toast('请输入单词', 'err'); return; }

    let res = { correct: false, matched_prefix: 0 };
    try {
      res = await API.checkSpelling(val, state.card.answer);
    } catch (e) { /* 用本地兜底 */ 
      res.correct = val.toLowerCase() === state.card.answer.toLowerCase();
    }

    // 逐字符高亮
    const hint = document.getElementById('spell-hint');
    if (hint) {
      hint.innerHTML = state.card.answer.split('').map((c, i) =>
        `<span class="${i < res.matched_prefix ? 'ok' : 'miss'}">${U().esc(c)}</span>`
      ).join('');
    }
    state.answered = true;

    const elapsed = Date.now() - state.startTs;
    const grade = res.correct ? (elapsed > 12000 ? 'hard' : 'good') : 'wrong';

    // 锁定输入
    if (input) input.disabled = true;
    document.getElementById('spell-submit')?.setAttribute('disabled', 'disabled');
    document.getElementById('spell-hintbtn')?.setAttribute('disabled', 'disabled');
    document.getElementById('spell-skip')?.setAttribute('disabled', 'disabled');

    await finishAnswer(res.correct, grade, elapsed);
  }

  function shuffle(a) {
    for (let i = a.length - 1; i > 0; i--) {
      const j = Math.floor(Math.random() * (i + 1));
      [a[i], a[j]] = [a[j], a[i]];
    }
    return a;
  }

  /* ---------------- 判分 ---------------- */

  async function onChoose(btn, value, reveal = false) {
    if (state.answered) return;
    state.answered = true;

    const card = state.card;
    const elapsed = Date.now() - state.startTs;
    const correct = reveal ? false : value === card.answer;

    document.querySelectorAll('#q-options .opt-btn').forEach(b => {
      b.disabled = true;
      const v = decodeURIComponent(b.dataset.value || '');
      if (v === card.answer) b.classList.add('correct');
      else if (b === btn) b.classList.add('wrong');
    });

    let grade = 'wrong';
    if (correct) grade = elapsed > 8000 ? 'hard' : 'good';

    await finishAnswer(correct, grade, elapsed);
  }

  /** 答题后的统一处理：发反馈、提交后端、自动发音、下一题。 */
  async function finishAnswer(correct, grade, elapsed) {
    const card = state.card;

    showFeedback(correct, card, grade);

    // 答对自动发音（需求 4）
    if (correct) {
      state.learnedCount++;
      try { speakCurrent(); } catch (e) {}
    }

    let res = null;
    try {
      res = await API.submitAnswer(card.entry.word, grade, elapsed);
    } catch (e) {
      U().toast('保存学习记录失败：' + e.message, 'err');
    }

    if (res) {
      updateCounters(res.correct_count, res.wrong_count);
      if (res.total) updateProgress(res.correct_count + res.wrong_count, res.total);

      const cfg = state.config;
      const autoPopup = !cfg || !cfg.study || cfg.study.auto_popup_on_wrong !== false;
      if (!correct && autoPopup) {
        setTimeout(() => {
          Detail.open(res.entry || card.entry, { reason: '答错了，看一下完整词条' });
        }, 620);
      }
    }

    const delay = correct ? 900 : 1500;
    setTimeout(async () => {
      const info = await safeSessionInfo();
      if (!info) {
        const cc = res ? res.correct_count : 0;
        const wc = res ? res.wrong_count : 0;
        const total = res ? res.total : 0;
        U().toast(`本轮完成！共 ${total} 题，答对 ${cc} 题，答错 ${wc} 题`, 'ok');
        if (total) updateProgress(total, total);
        return end();
      }
      updateProgress(info.index, info.total);
      await nextQuestion();
    }, delay);
  }

  async function safeSessionInfo() {
    try {
      const m = meta();
      const card = (m.typing || m.example)
        ? (await API.buildAdvancedCard(toBackend(state.mode)))
        : (await API.currentQuestion(state.lang));
      if (!card) return null;
      return {
        index: card.index || 0,
        total: card.total || 0,
        correct: card.correct_count || 0,
        wrong: card.wrong_count || 0,
        card,
      };
    } catch (e) {
      return null;
    }
  }

  function speakCurrent() {
    const card = state.card;
    if (!card) return;
    try {
      if (window.Speak) {
        window.Speak.speak(card.entry.word, {
          audio: card.entry.phonetic && card.entry.phonetic.audio,
          lang: card.entry.lang || 'en',
        });
      }
    } catch (e) { /* 忽略 */ }
  }

  function showFeedback(correct, card, grade) {
    const el = document.getElementById('q-feedback');
    el.classList.remove('hidden', 'ok', 'bad');
    el.classList.add(correct ? 'ok' : 'bad');

    const entry = card.entry;
    const def = (entry.senses && entry.senses[0])
      ? `${entry.senses[0].pos || ''} ${entry.senses[0].definition || ''}`.trim()
      : '';

    if (correct) {
      const pace = grade === 'hard' ? '答对了，但有点犹豫，已按「较难」安排下次复习。' : '答对了！';
      el.innerHTML = `<span class="fb-icon">&#10004;</span>
        <div>${pace}
        <div style="margin-top:4px;font-size:12.5px;opacity:.85">
          <b>${U().esc(entry.word)}</b> —— ${U().esc(def)}
        </div></div>`;
    } else {
      el.innerHTML = `<span class="fb-icon">&#10008;</span>
        <div><b>${U().esc(entry.word)}</b> —— ${U().esc(def)}
        <div style="margin-top:4px;font-size:12.5px;opacity:.85">该词已加入强化记忆，稍后会再考你一次。</div></div>`;
    }
    el.classList.remove('hidden');
  }

  function updateCounters(correct, wrong) {
    const c = document.getElementById('q-correct');
    const w = document.getElementById('q-wrong');
    if (c) c.textContent = correct || 0;
    if (w) w.textContent = wrong || 0;
  }

  function updateProgress(index, total) {
    const pct = total ? Math.min(100, (index / total) * 100) : 0;
    const fill = document.getElementById('q-progress');
    if (fill) fill.style.width = pct + '%';
    const text = document.getElementById('q-progress-text');
    if (text) text.textContent = `${Math.min(index, total)} / ${total}`;
  }

  /* ---------------- 词库下拉 ---------------- */

  async function loadBookOptions() {
    let books = [];
    try { books = await API.listWordbooks(); } catch (e) { return; }
    // 只保留叶子（可背的）词库 + 全库
    const leafs = books.filter(b => b.level >= 2 || b.id === 'root');
    const html = leafs.map(b =>
      `<option value="${U().esc(b.id)}">${U().esc(b.name)}（${b.word_count || 0}）</option>`
    ).join('');
    ['study-book', 'study-book2'].forEach(id => {
      const el = document.getElementById(id);
      if (!el) return;
      el.innerHTML = '<option value="">全部词库</option>' + html;
      el.value = state.bookId || '';
      el.onchange = () => setBook(el.value);
    });
  }

  /* ---------------- 键盘操作 ---------------- */

  function bindKeys() {
    document.addEventListener('keydown', (e) => {
      // ① 焦点在输入框/文本域里时，键盘完全属于该控件。
      //    缺了这个判断，会话运行期间在「查词 / 词库」的搜索框里打字会被
      //    下面的 A~H / 1~8 逻辑吃掉并 preventDefault —— 现象就是
      //    「搜索框打不了字、退格也没用」（A~H 全吞，单词只剩残字）。
      if (U().isTypingTarget(e.target)) return;
      // ② 只有「背诵」页处于激活状态时才接管键盘。
      //    用户切到别的页面后会话仍在 running，但快捷键不该继续生效，
      //    否则会偷偷把后台的题答掉。
      const page = document.getElementById('page-study');
      if (!page || !page.classList.contains('active')) return;

      if (!state.running || state.answered || !state.card) return;
      if (meta().typing) return; // 拼写模式由输入框自己处理
      const key = e.key.toUpperCase();
      const idx = '12345678'.indexOf(key) >= 0
        ? parseInt(key, 10) - 1
        : 'ABCDEFGH'.indexOf(key);
      if (idx >= 0) {
        const btns = document.querySelectorAll('#q-options .opt-btn');
        if (btns[idx]) { e.preventDefault(); btns[idx].click(); }
      }
      if (e.key === ' ' && state.mode === 'listen_spell') {
        e.preventDefault();
        speakCurrent();
      }
    });
  }

  function bind() {
    document.getElementById('btn-start')?.addEventListener('click', () => start());
    document.getElementById('btn-start2')?.addEventListener('click', () => start());
    document.getElementById('btn-end-session')?.addEventListener('click', end);
    document.getElementById('btn-leech-quiz')?.addEventListener('click', () => {
      window.Pages.go('study');
      start({ leechOnly: true, bookId: '' });
    });

    document.querySelectorAll('#mode-seg .seg-btn').forEach(b => {
      b.addEventListener('click', () => setMode(b.dataset.mode));
    });

    bindKeys();
    loadBookOptions();
  }

  return { bind, start, end, setMode, setBook, startWithBook, loadBookOptions, state };
})();

window.Study = Study;
