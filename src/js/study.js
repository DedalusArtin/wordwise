/* ============================================================
   study.js —— 背诵引擎（需求 1、3、4、5）
   - 双向模式：看英选中 / 看中选英
   - 按后端下发的顺序出题（遗忘曲线排程）
   - 答错自动弹出详情卡（仿有道词典）
   - 记录每题耗时，供 SRS 判定难度
   ============================================================ */

const Study = (() => {
  const { API } = window.WordWiseAPI;
  const U = () => window.WW;

  const state = {
    mode: 'en_to_zh',
    running: false,
    card: null,          // 当前题目
    startTs: 0,          // 本题开始时间
    answered: false,
    config: null,
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

  async function start(opts = {}) {
    const cfg = await ensureConfig();
    const size = opts.size
      || parseInt(document.getElementById('opt-batch')?.value, 10)
      || (cfg && cfg.study && cfg.study.batch_size)
      || 20;
    const leechOnly = opts.leechOnly !== undefined
      ? opts.leechOnly
      : !!document.getElementById('opt-leech-only')?.checked;

    let info;
    try {
      info = await API.startSession(state.mode, size, leechOnly);
    } catch (e) {
      U().toast(e.message, 'err');
      return;
    }

    state.running = true;
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

  async function nextQuestion() {
    state.answered = false;
    let card;
    try {
      card = await API.currentQuestion();
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
    const en = card.mode === 'en_to_zh';

    document.getElementById('q-label').textContent = en
      ? '请选择正确的中文释义'
      : '请选择正确的英文单词';
    document.getElementById('q-prompt').textContent = card.prompt;

    // 音标仅在看英选中时展示（否则等于送答案）
    const phonEl = document.getElementById('q-phonetic');
    if (en && study.show_phonetic !== false && card.entry.phonetic) {
      const p = card.entry.phonetic;
      phonEl.textContent = [p.uk, p.us].filter(Boolean).join('  ');
    } else {
      phonEl.textContent = '';
    }

    document.getElementById('qc-leech').classList.toggle('hidden', !card.is_leech);

    // 选项：正确答案 + 干扰项，随机打乱
    const opts = [...(card.options || [])];
    if (!opts.includes(card.answer)) opts.push(card.answer);
    shuffle(opts);

    const box = document.getElementById('q-options');
    box.innerHTML = opts.map((o, i) => `
      <button class="opt-btn" data-value="${encodeURIComponent(o)}">
        <span class="opt-key">${String.fromCharCode(65 + i)}</span>
        <span>${U().esc(o)}</span>
      </button>
    `).join('');

    box.querySelectorAll('.opt-btn').forEach(btn => {
      btn.addEventListener('click', () => onChoose(btn, decodeURIComponent(btn.dataset.value)));
    });

    document.getElementById('q-feedback').classList.add('hidden');
    document.getElementById('q-actions').innerHTML = '';

    // 若选项不足（词库太小），提供「显示答案」
    if (opts.length < 2) {
      document.getElementById('q-actions').innerHTML =
        '<button class="grade-btn g-good" id="btn-reveal">显示答案</button>';
      document.getElementById('btn-reveal').onclick = () => onChoose(null, null, true);
    }
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

    // 锁定选项并高亮
    document.querySelectorAll('#q-options .opt-btn').forEach(b => {
      b.disabled = true;
      const v = decodeURIComponent(b.dataset.value || '');
      if (v === card.answer) b.classList.add('correct');
      else if (b === btn) b.classList.add('wrong');
    });

    // 难度判定：答对但犹豫超过 8 秒记为 hard
    let grade = 'wrong';
    if (correct) grade = elapsed > 8000 ? 'hard' : 'good';

    // 即时反馈
    showFeedback(correct, card, grade);

    // 提交后端，驱动记忆调度
    let res = null;
    try {
      res = await API.submitAnswer(card.entry.word, grade, elapsed);
    } catch (e) {
      U().toast('保存学习记录失败：' + e.message, 'err');
    }

    // 更新计数
    if (res) {
      updateCounters(res.correct_count, res.wrong_count);
      if (res.total) {
        updateProgress(res.correct_count + res.wrong_count, res.total);
      }

      // 答错时自动弹出完整详情卡（需求 5）
      const cfg = state.config;
      const autoPopup = !cfg || !cfg.study || cfg.study.auto_popup_on_wrong !== false;
      if (!correct && autoPopup) {
        setTimeout(() => {
          Detail.open(res.entry || card.entry, { reason: '答错了，看一下完整词条' });
        }, 620);
      }
    }

    // 下一题 / 结束
    const delay = correct ? 900 : 1400;
    setTimeout(async () => {
      const info = await safeSessionInfo();
      // info 为 null 表示后端已无下一题 → 本轮结束
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
    // 后端在 QuizCard 中直接返回本轮进度（index/total/correct/wrong）
    try {
      const card = await API.currentQuestion(state.lang);
      if (!card) return null; // 没有下一题 → 本轮结束
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

  function showFeedback(correct, card, grade) {
    const el = document.getElementById('q-feedback');
    el.classList.remove('hidden', 'ok', 'bad');
    el.classList.add(correct ? 'ok' : 'bad');

    if (correct) {
      const pace = grade === 'hard' ? '答对了，但有点犹豫，已按「较难」安排下次复习。' : '答对了！';
      el.innerHTML = `<span class="fb-icon">&#10004;</span><div>${pace}</div>`;
    } else {
      const entry = card.entry;
      const def = (entry.senses && entry.senses[0])
        ? `${entry.senses[0].pos} ${entry.senses[0].definition}`
        : '';
      el.innerHTML = `<span class="fb-icon">&#10008;</span>
        <div><b>${U().esc(entry.word)}</b> —— ${U().esc(def)}
        <div style="margin-top:4px;font-size:12.5px;opacity:.85">该词已加入强化记忆，稍后会再考你一次。</div></div>`;
    }
    document.getElementById('q-feedback').classList.remove('hidden');
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

  /* ---------------- 键盘操作 ---------------- */

  function bindKeys() {
    document.addEventListener('keydown', (e) => {
      if (!state.running || state.answered || !state.card) return;
      // 数字键 / 字母键选择
      const key = e.key.toUpperCase();
      const idx = '12345678'.indexOf(key) >= 0
        ? parseInt(key, 10) - 1
        : 'ABCDEFGH'.indexOf(key);
      if (idx >= 0) {
        const btns = document.querySelectorAll('#q-options .opt-btn');
        if (btns[idx]) { e.preventDefault(); btns[idx].click(); }
      }
    });
  }

  function bind() {
    document.getElementById('btn-start')?.addEventListener('click', () => start());
    document.getElementById('btn-start2')?.addEventListener('click', () => start());
    document.getElementById('btn-end-session')?.addEventListener('click', end);
    document.getElementById('btn-leech-quiz')?.addEventListener('click', () => {
      window.Pages.go('study');
      start({ leechOnly: true });
    });

    document.querySelectorAll('#mode-seg .seg-btn').forEach(b => {
      b.addEventListener('click', () => setMode(b.dataset.mode));
    });

    bindKeys();
  }

  return { bind, start, end, setMode, state };
})();

window.Study = Study;
