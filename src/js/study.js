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

  /* SRS「hard」判定耗时阈值（P7：原来是散落的魔法数 8000/12000，
     两处口径对不上账还以为是 bug）。两值**有意不同**：
     选择题只需读选项 → 8 秒算犹豫；拼写要打完一个词 → 给 12 秒。
     改阈值只改这里。 */
  const HARD_MS_CHOICE = 8000;
  const HARD_MS_TYPING = 12000;

  /** 各模式的界面文案与行为 */
  const MODE_META = {
    en_to_zh: { label: '请选择正确的中文释义', typing: false, example: false, audio: false },
    zh_to_en: { label: '请选择正确的英文单词', typing: false, example: false, audio: false },
    spelling: { label: '看释义，拼出这个单词', typing: true, example: false, audio: false },
    ex_to_zh: { label: '看例句，选择正确的释义', typing: false, example: true, audio: false },
    ex_pick_word: { label: '看例句，选出正确的单词', typing: false, example: true, audio: false },
    listen_spell: { label: '听发音，拼出这个单词', typing: true, example: false, audio: true },
  };

  /* ---------------- 词库语言 → 界面文案 ----------------
     背日语教材时把「看英选中」「请选择正确的英文单词」原样摆着是错的 ——
     学生会以为软件把日语当成英语。这里按**所选词库的语言**改写所有
     与语种相关的字样，并下线对该语言没有意义的模式。 */
  const LANG_INFO = {
    en: { name: '英语', word: '英', full: '英文单词' },
    zh: { name: '中文', word: '中', full: '中文词' },
    ja: { name: '日语', word: '日', full: '日语单词' },
    ko: { name: '韩语', word: '韩', full: '韩语单词' },
    fr: { name: '法语', word: '法', full: '法语单词' },
    de: { name: '德语', word: '德', full: '德语单词' },
    es: { name: '西班牙语', word: '西', full: '西班牙语单词' },
    ru: { name: '俄语', word: '俄', full: '俄语单词' },
    it: { name: '意大利语', word: '意', full: '意大利语单词' },
    pt: { name: '葡萄牙语', word: '葡', full: '葡萄牙语单词' },
    ar: { name: '阿拉伯语', word: '阿', full: '阿拉伯语单词' },
    th: { name: '泰语', word: '泰', full: '泰语单词' },
  };

  /** 取语言信息，未知语种退化为「XX语」。 */
  function langInfo(lang) {
    const code = String(lang || 'en').toLowerCase().split(/[-_]/)[0];
    if (LANG_INFO[code]) return { ...LANG_INFO[code], code };
    return { name: code.toUpperCase() + '语', word: code.toUpperCase(), full: '单词', code };
  }

  /** 模式按钮文字（随语言变化）。 */
  function modeButtonText(mode, info) {
    const w = info.word;
    switch (mode) {
      case 'en_to_zh': return info.code === 'zh' ? '看词选义' : `看${w}选中`;
      case 'zh_to_en': return `看中选${w}`;
      case 'spelling': return '拼写';
      case 'ex_to_zh': return '例句选义';
      case 'ex_pick_word': return '例句识词';
      case 'listen_spell': return '听音拼写';
      default: return mode;
    }
  }

  /** 题面提示文字（随语言变化）。 */
  function modePromptLabel(mode, info) {
    switch (mode) {
      case 'en_to_zh': return '请选择正确的中文释义';
      case 'zh_to_en': return `请选择正确的${info.full}`;
      case 'spelling': return `看释义，拼出这个${info.full}`;
      case 'ex_to_zh': return '看例句，选择正确的释义';
      case 'ex_pick_word': return `看例句，选出正确的${info.full}`;
      case 'listen_spell': return `听发音，拼出这个${info.full}`;
      default: return MODE_META[mode] ? MODE_META[mode].label : '';
    }
  }

  /** 该语言下不适用的模式（例如中文词库没有「看中选中」的意义）。 */
  function disabledModes(info) {
    const off = [];
    if (info.code === 'zh') off.push('zh_to_en');
    return off;
  }

  const state = {
    mode: 'en_to_zh',
    running: false,
    /* 本轮属于哪个会话槽位（需求 14：背诵与复习不共用容器）。
       'study'  = 背诵（新词 / 指定词库 / 常错词攻坚）
       'review' = 今日复习（只出今天到期的词，数量绝不补足）
       ★ 这个字段必须跟着**每一次**后端调用走：漏传一处的后果是
         「题从复习槽出、答案记进背诵槽」—— 计数与判分静默错位，
         界面看起来一切正常，极难发现。 */
    kind: 'study',
    card: null,          // 当前题目
    startTs: 0,          // 本题开始时间
    answered: false,
    config: null,
    bookId: '',          // 指定词库（空 = 全库）
    bookLang: '',        // 当前词库语言（en / ja / zh…）
    defLang: 'zh',       // 题面释义语言：zh 中文 / src 原文 / '' 不限
    hintLevel: 0,        // 拼写提示等级
    learnedCount: 0,     // 本轮已答对计数（用于自动发音）
    bookLangById: {},    // 词库 id → 语言

    /* ---- 手感：连对计数 + 错词攻坚 ---- */
    // 连对次数。答错立即归零。它存在的意义不是统计，是**给反馈**：
    // 一个数字在往上走，比任何「加油」都更能让人多背几个。
    streak: 0,
    bestStreak: 0,       // 本轮最高连对，结束时一并告知
    // 攻坚队列：word → 这个词在答错之后又连对了几次。
    // ★ 为什么是「答错后才入队」：不强求全库错词本与这里同步，
    //   本轮答错 = 现在确实不会，立刻就地盯住它，反馈最直接。
    grind: {},
    GRIND_TARGET: 3,     // 连对这么多次就出队（3 次是「记住了」的最低可接受值）
    // 下一题的定时器与「立即推进」函数。回车 / 空格能抢在定时器前面走，
    // 这是键盘流最关键的一下 —— 否则背得快的用户只能干等 0.9 秒。
    advanceTimer: null,
    advanceNow: null,

    /* ---- 底部区域：上一个单词 + 已背单词折叠列表 ---- */
    // 本轮已作答的单词（含完整词条，所以列表里能直接看释义、点详情、听发音）。
    // 只存内存：它是「这一轮的过程」，重开一轮就该清空。
    history: [],
    learnedScope: 'session',   // session（本轮） | recent（最近背过，跨会话）
    recent: null,              // 懒加载：第一次切到「最近背过」才去查库
    recentLoading: false,
    /* ---- 分词库容器（已背列表按词库分组）---- */
    bookNameById: {},          // 书 id → 显示名（组头文案）
    _bookRefs: {},             // `lang|word` → [{id, name}] 归属缓存（查过一次不再查）
    _renderGen: 0,             // 列表渲染代际：await 归属查询回来后，过期的渲染直接丢弃
    _recs: [],                 // 当前列表渲染出来的记录（点「详情」时按下标回查）

    /* ---- 学习目标（需求 15） ---- */
    goal: null,                // 后端 StudyGoal 视图；null = 还没拉到
    goalLoaded: false,         // 是否已成功拉过一次（失败时不显示空壳）
    goalCelebrated: '',        // 已经弹过祝贺卡的那个「日期|目标数」，用于去重
  };

  /* ---------------- 语言自适应 ---------------- */

  /** 按语言改写模式按钮、题面文案、语言标记、释义语言默认值。 */
  function applyLangUI(lang) {
    const info = langInfo(lang);
    state.bookLang = info.code;

    // 1) 模式按钮文字 + 可用性
    const off = disabledModes(info);
    document.querySelectorAll('#mode-seg .seg-btn').forEach(b => {
      const m = b.dataset.mode;
      b.textContent = modeButtonText(m, info);
      const disabled = off.includes(m);
      b.disabled = disabled;
      b.title = disabled ? `「${info.name}」词库不支持这个模式` : b.textContent;
    });
    // 当前模式被下线 → 回到默认
    if (off.includes(state.mode)) setMode('en_to_zh');

    // 2) 语言标记
    const chip = document.getElementById('study-lang-chip');
    if (chip) {
      chip.textContent = info.name;
      chip.classList.toggle('hidden', !lang);
    }

    // 3) 释义语言：默认中文释义（背外语看中文天经地义），可切「原文释义」做英英/日日。
    //    用户的手工选择按语言分别记住。
    let saved = '';
    try { saved = localStorage.getItem('ww.study.deflang.' + info.code) || ''; } catch (e) { /* 忽略 */ }
    state.defLang = saved || 'zh';
    const sel = document.getElementById('study-def-lang');
    if (sel) sel.value = state.defLang;
    const hint = document.getElementById('study-def-lang-hint');
    if (hint) {
      hint.textContent = state.defLang === 'src'
        ? `题面使用词典给出的${info.name}原文释义`
        : '题面使用中文释义（背外语时更直观）';
    }

    // 4) 题面提示（若当前正在答题也同步刷新）
    if (state.running && state.card) {
      const lab = document.getElementById('q-label');
      if (lab) lab.textContent = modePromptLabel(state.mode, info);
    }
  }

  /** 读取当前释义语言偏好（下拉的 change 处理）。 */
  function setDefLang(v) {
    state.defLang = v || '';
    try { localStorage.setItem('ww.study.deflang.' + state.bookLang, state.defLang); } catch (e) { /* 忽略 */ }
    applyLangUI(state.bookLang);
  }

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
    // 题面提示随模式与词库语言一起变
    const lab = document.getElementById('q-label');
    if (lab) lab.textContent = modePromptLabel(mode, langInfo(state.bookLang));
  }

  /** 未指定词库时用配置里的目标语言。 */
  function defaultLang() {
    const cfg = state.config || (window.App && window.App.config) || null;
    return (cfg && cfg.target_lang) || 'en';
  }

  /** 同步词库下拉选择（多个 select 保持一致）。 */
  function setBook(bookId) {
    state.bookId = bookId || '';
    // ★ 选库后四张统计卡与「开始今日复习（N 词）」的 N 必须跟着书变 ——
    //   refreshStudy 会按 state.bookId 去取 stats（cmd_stats 现支持按书口径）。
    try { window.Pages && window.Pages.refreshStudy && window.Pages.refreshStudy(); } catch (e) { /* 概览不在时忽略 */ }
    // 已背列表的分组排序也以「所选词库优先」—— 切库后原地重分组一次
    try { renderLearned(); } catch (e) { /* 列表不在时忽略 */ }
    ['study-book', 'study-book2'].forEach(id => {
      const el = document.getElementById(id);
      if (el) el.value = state.bookId;
    });
    // ★ 界面语言跟着**所选词库**走：选 JLPT 就切到日语文案与中文释义，
    //   切回「全部词库」则回到配置里的目标语言。
    const lang = state.bookId
      ? (state.bookLangById[state.bookId] || '')
      : '';
    applyLangUI(lang || defaultLang());
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
        const bl = state.bookLangById[bookId] || null;
        info = await API.startBookSession(bookId, backendMode, size, bl, state.defLang);
      } else {
        // kind='study'：写背诵槽位，绝不碰复习槽位
        // ★ P1：带上学习语言 —— 后端会在建队列时把它锁进 Session，
        //   之后出题/判分/调度都不再依赖前端传参；这里仍显式传，
        //   兜底旧会话（无 lang 字段）的情况。
        info = await API.startSession(backendMode, size, leechOnly, state.bookLang || null, state.defLang, 'study');
      }
    } catch (e) {
      U().toast(e.message, 'err');
      return;
    }

    beginRun(info, 'study');
  }

  /**
   * 开始一轮**今日复习**（需求 14）。
   *
   * 与 [`start`] 的三点不同，每一点都是用户明确提过的：
   *   1. 队列**只由今天到期的词构成，绝不补足**到「每轮题量」——
   *      入口按钮写「12 个」，点进去就一定是 12 个。补成 20 会让按钮上的
   *      数字变成谎话，这比少背几个词严重得多。
   *   2. 写**独立槽位**（kind='review'），不会把正在进行的背诵会话清掉。
   *   3. 一个到期词都没有时 `total === 0`：这是正常状态，不是错误 ——
   *      提示一句「今天没有到期的词」就返回，绝不进答题界面。
   */
  async function startToday() {
    await ensureConfig();
    let info;
    try {
      info = await API.startReviewSession(
        toBackend(state.mode), null, state.bookLang || null, state.defLang,
        // ★ 与顶部词库下拉同口径：选了书就只背该书的到期词。
        //   此前不传 → 队列按语言全库混背，与「开始背诵」的按书行为矛盾。
        state.bookId || null);
    } catch (e) {
      U().toast(e.message, 'err');
      return;
    }
    if (!info || !info.total) {
      // ★ 两种「0」必须分开说，否则会和按钮上的数字当众打架：
      //   ① 概览上也写着 0 —— 真的没有到期词，这是正常状态，给一句轻松的提示；
      //   ② 概览上写着 N>0、队列却是空的 —— 说明这些词的词条在当前语言下取不到
      //      （历史数据里确实存在「学习状态记在 A、词条只存在于 B」的记录）。
      //      此时还说「没有到期的词」就是假话 —— 用户截图里正是
      //      「开始今日复习（1 词）」与「今天没有到期的词了」同屏出现。
      const dueEl = document.getElementById('s-due');
      const shown = parseInt((dueEl && dueEl.textContent) || '0', 10) || 0;
      U().toast(
        shown > 0
          ? `有 ${shown} 个词到期，但它们的词条不在当前词库里，已跳过`
          : '今天没有到期的词了，可以背点新词或去错词本转转',
        shown > 0 ? 'err' : 'ok');
      // 顺手把概览刷一遍：用户看到的所有数字都来自同一次统计
      if (window.Pages && window.Pages.refreshStudy) window.Pages.refreshStudy();
      return;
    }
    beginRun(info, 'review');
  }

  /** 进入答题界面。`kind` 决定后续所有后端调用落在哪个会话槽位。 */
  function beginRun(info, kind) {
    if (!info) return;
    clearAdvance();
    state.kind = kind === 'review' ? 'review' : 'study';
    state.running = true;
    state.learnedCount = 0;
    // 新的一轮 = 新的过程记录。上一轮的词不该再出现在「上一个」里。
    state.history = [];
    // 连对与攻坚都是「本轮」的概念：上一轮的连对不该延续到这一轮，
    // 攻坚队列同理（新的一轮重新开始盯，才盯得住）。
    state.streak = 0;
    state.bestStreak = 0;
    state.grind = {};
    // 进入答题界面后就不再显示祝贺卡：它属于「概览」这个场景
    hideGoalDone();
    updateStreakUI();
    document.getElementById('study-idle')?.classList.add('hidden');
    document.getElementById('study-run')?.classList.remove('hidden');
    updateCounters(info.correct, info.wrong);
    updateProgress(0, info.total);
    renderFoot();
    return nextQuestion();
  }

  async function end() {
    try {
      // 只结束**当前这一轮**的槽位：结束复习不该把背诵会话一起清掉（需求 14）
      await API.endSession(state.kind);
    } catch (e) { /* 忽略 */ }
    clearAdvance();
    state.running = false;
    state.card = null;
    document.getElementById('study-idle')?.classList.remove('hidden');
    document.getElementById('study-run')?.classList.add('hidden');
    // 结束之后目标进度可能刚好达标 → 刷新概览 + 刷新目标，再判断要不要弹祝贺卡
    if (window.Pages && window.Pages.refreshStudy) window.Pages.refreshStudy();
    loadGoal().then(() => maybeCelebrate());
  }

  /* ---------------- 出题 ---------------- */

  function meta() {
    const base = MODE_META[state.mode] || MODE_META.en_to_zh;
    return { ...base, label: modePromptLabel(state.mode, langInfo(state.bookLang)) };
  }

  async function nextQuestion() {
    // 上一题的推进定时器理论上已被 clearAdvance 清掉，这里再兜一次：
    // 漏清的后果是「正在答第 N 题，定时器把第 N+1 题推上来」。
    clearAdvance();
    state.answered = false;
    state.hintLevel = 0;
    let card;
    try {
      // 进阶模式用专用命令，保证例句/拼写字段齐全
      const m = meta();
      if (m.typing || m.example) {
        card = await API.buildAdvancedCard(toBackend(state.mode), state.bookLang || null, state.kind);
        // 后端无会话时退回普通接口
        if (!card) card = await API.currentQuestion(state.bookLang || null, state.kind);
      } else {
        card = await API.currentQuestion(state.bookLang || null, state.kind);
      }
    } catch (e) {
      U().toast(e.message, 'err');
      return end();
    }

    if (!card) {
      U().toast(state.kind === 'review' ? '今日复习完成，休息一下吧' : '本轮已完成，休息一下吧', 'ok');
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

    // 题面朗读按钮。
    //
    // 只在「题面上摆着的就是能读的东西」时才出现：
    //   en_to_zh   题面是单词本身 → 读单词
    //   ex_to_zh   题面是完整例句 → 读句子
    // 其余模式题面是释义、「看中选英」这类，读出来毫无意义；
    // 而 ex_pick_word 的答案就是这个词，读出来等于直接报答案。
    const say = document.getElementById('q-say');
    if (say) {
      const sayWord = state.mode === 'en_to_zh' ? (card.entry.word || '') : '';
      const sayText = state.mode === 'ex_to_zh' ? (card.example_raw || '') : '';
      say.dataset.speakWord = sayWord;
      // closest() 会包含元素自身，所以 [data-speak-text] 挂在按钮上也能被委托读到
      say.dataset.speakText = sayText;
      say.dataset.speakLang = card.entry.lang || 'en';
      say.dataset.speakAudio = (card.entry.phonetic && card.entry.phonetic.audio) || '';
      say.classList.toggle('hidden', !sayWord && !sayText);
    }

    // 音标
    const phonEl = document.getElementById('q-phonetic');
    // 容器带上词条信息：里面所有 .speak-btn 靠事件委托就能取到词与音频，
    // 不需要给每个按钮单独绑一次。
    phonEl.dataset.entryWord = card.entry.word || '';
    phonEl.dataset.speakLang = card.entry.lang || 'en';
    phonEl.dataset.speakAudio = (card.entry.phonetic && card.entry.phonetic.audio)
      || card.audio || '';
    phonEl.innerHTML = '';
    if (state.mode === 'listen_spell') {
      // 听音拼写：音标就是答案，绝对不能显示，只给一个播放按钮
      phonEl.innerHTML = `<button class="speak-btn big" id="q-play" title="播放发音（空格键）">`
        + `${U().icon('sound')} 播放</button>`;
    } else if (state.mode === 'en_to_zh' && study.show_phonetic !== false) {
      // 统一走 phoneticHtml：英/美分标、斜杠规则、IPA 字体只有一份实现
      phonEl.innerHTML = U().phoneticHtml(card.entry, { speak: true })
        || `<button class="speak-btn" id="q-play" title="播放发音">${U().icon('sound')} 播放</button>`;
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

    // 底部「上一个单词」：此刻 history 的最后一笔就是上一题
    renderPrev();
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
      if (e.key !== 'Enter') return;
      // ★ 已作答后**不能**再 preventDefault：这时回车属于「进入下一题」，
      //   必须让它继续冒泡到 document 上的快捷键处理里。
      //   原来的代码无条件 `e.preventDefault(); submitSpelling();`，
      //   submitSpelling 因为 `state.answered` 直接 return —— 于是回车
      //   被彻底吃掉，用户按半天没反应（需求 13 的拼写模式那一半）。
      if (state.answered) return;
      e.preventDefault();
      submitSpelling();
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
    const grade = res.correct ? (elapsed > HARD_MS_TYPING ? 'hard' : 'good') : 'wrong';

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
    if (correct) grade = elapsed > HARD_MS_CHOICE ? 'hard' : 'good';
    // ★ P6：「显示答案 / 拼写跳过」不是答错 —— 历史实现按 wrong 提交，
    //   每看一次答案 = 一次 lapse + 进错词本。改为 'skip'：后端走 cmd_skip，
    //   只推进会话、不写 study_state / review_log，两次「跳过」语义合一。
    if (reveal) grade = 'skip';

    await finishAnswer(correct, grade, elapsed);
  }

  /** 答题后的统一处理：发反馈、提交后端、自动发音、下一题。 */
  async function finishAnswer(correct, grade, elapsed) {
    const card = state.card;

    // 记进本轮历史 —— 底部的「上一个单词」与已背列表都读它。
    // 记的是**完整词条**，所以列表里能直接看释义、点详情、听发音，
    // 不需要再为每个词回查一次数据库。
    if (card && card.entry) {
      state.history.push({
        entry: card.entry,
        correct: !!correct,
        grade,
        elapsed,
        ts: Date.now(),
      });
    }
    renderLearned();

    showFeedback(correct, card, grade);

    // ---- 连对计数 + 错词攻坚 ----
    if (correct) {
      state.streak++;
      if (state.streak > state.bestStreak) state.bestStreak = state.streak;
    } else {
      state.streak = 0;
    }
    updateStreakUI();

    // skip 不是错词攻坚对象 —— 别把「你连错 N 次」的提示套在主动看答案上
    const note = grade === 'skip' ? null : grindNote(card && card.entry && card.entry.word, correct);
    if (note) {
      const box = document.getElementById('q-feedback');
      if (box) {
        box.classList.remove('hidden');
        box.insertAdjacentHTML('beforeend',
          `<div class="fb-grind ${correct ? 'ok' : 'bad'}">${note}</div>`);
      }
    }

    // 自动发音
    if (correct) {
      state.learnedCount++;
      try { speakCurrent(); } catch (e) {}
    } else {
      // 答错更要发音：这时候「听到正确读音」比什么都重要 ——
      // 老逻辑只在答对时读，答错的词用户永远听不到自己错在哪。
      // 稍等一下再读，避开答错时的提示动画。
      setTimeout(() => { try { speakCurrent(); } catch (e) {} }, 420);
    }

    /* ★ 关键顺序：**先把「可以推进了」这件事挂出去，再等后端**。
     *
     * 原来的写法是 `await API.submitAnswer(...)` 之后才 `state.advanceNow = ...`，
     * 于是整个 IPC 往返期间（本地 SQLite 也要几十到几百毫秒，冷启动更久）
     * 界面上明明已经判完分、选项也灰了，用户按回车却什么都没发生 ——
     * 这正是「背词的时候出现错误按回车不能到下一个词」的主因之一。
     *
     * 现在提交变成**在飞**（pending），回车可以在它返回之前就推进；
     * runAdvance 会 await 这个 pending 拿计数，所以「本轮完成」那条
     * 统计文案依然准确。答错那条 1.5 秒的定时器也不再是阻塞：它只是
     * 「用户没按回车时替用户按一下」。
     */
    // ★ P6：skip 走后端 cmd_skip（不计分），返回的是 SessionInfo ——
    //   映射成 AnswerResult 形状，下游收尾（计数/进度）一套代码走完。
    const pending = (grade === 'skip'
      ? API.skip(state.kind).then(si => ({
          correct_count: si.correct,
          wrong_count: si.wrong,
          total: si.total,
          requeued: false,
          skipped: true,
        }))
      : API.submitAnswer(card.entry.word, grade, elapsed, state.bookLang || null, state.kind))
      .catch((e) => {
        U().toast('保存学习记录失败：' + e.message, 'err');
        return null;
      });

    clearAdvance();
    const advance = () => {
      clearAdvance();
      return runAdvance(pending, card);
    };
    state.advanceNow = advance;
    // skip 时反馈里刚给出释义 —— 多留阅读时间（回车仍可随时推进）
    state.advanceTimer = setTimeout(advance, correct ? 900 : (grade === 'skip' ? 3000 : 1500));

    // ---- 后端返回后的收尾（计数、自动弹详情卡）----
    const res = await pending;
    if (!res) return;

    updateCounters(res.correct_count, res.wrong_count);
    if (res.total) updateProgress(res.correct_count + res.wrong_count, res.total);

    // 答错回插是本轮**真实发生**的（后端 requeue_wrong 把词排到队列后面），
    // 所以「过几题还会再考你一次」这句提示现在不是空话 —— 但只在
    // `res.requeued === true` 时才显示；没回插（比如同一词的回插预算用完了）
    // 就如实说「已记录」，不糊弄用户。
    if (!correct) {
      const tip = document.getElementById('fb-requeue');
      const plain = document.getElementById('fb-note');
      if (res.requeued) {
        tip?.classList.remove('hidden');
        plain?.classList.add('hidden');
      }
    }

    const cfg = state.config;
    const autoPopup = !cfg || !cfg.study || cfg.study.auto_popup_on_wrong !== false;
    // 已经推进到下一题了就不再补弹详情卡 —— 那会把用户已经看到的题面盖掉
    if (!correct && grade !== 'skip' && autoPopup && state.running && state.advanceNow === advance) {
      setTimeout(() => {
        if (state.advanceNow !== advance) return;   // 期间按过回车 → 别弹了
        Detail.open(res.entry || card.entry, { reason: '答错了，看一下完整词条' });
      }, 620);
    }
  }

  /** 清掉「自动进入下一题」的定时器与手动入口。 */
  function clearAdvance() {
    if (state.advanceTimer) {
      clearTimeout(state.advanceTimer);
      state.advanceTimer = null;
    }
    state.advanceNow = null;
  }

  /**
   * 进入下一题（或收尾本轮）。由定时器或用户按回车触发，谁先谁生效。
   *
   * @param {Promise|null} pendingRes 在飞的 `submitAnswer` 结果（可能还没回来）
   */
  async function runAdvance(pendingRes, card) {
    const res = pendingRes ? await pendingRes : null;
    // 用户抢在提交返回前按了回车 → 顺手把详情卡收掉，别盖着下一题
    Detail.close();

    const info = await safeSessionInfo();
    if (!info) {
      const cc = res ? res.correct_count : 0;
      const wc = res ? res.wrong_count : 0;
      const total = res ? res.total : (card ? state.history.length : 0);
      U().toast(
        `本轮完成！共 ${total} 题，答对 ${cc} 题，答错 ${wc} 题` +
        (state.bestStreak >= 3 ? ` · 最高连对 ${state.bestStreak}` : ''),
        'ok');
      if (total) updateProgress(total, total);
      return end();
    }
    updateProgress(info.index, info.total);
    await nextQuestion();
  }

  async function safeSessionInfo() {
    try {
      const m = meta();
      const card = (m.typing || m.example)
        ? (await API.buildAdvancedCard(toBackend(state.mode), state.bookLang || null, state.kind))
        : (await API.currentQuestion(state.bookLang || null, state.kind));
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

  /**
   * 读当前题的发音。
   *
   * `anchor` 是「当前语音」提示条要贴着的元素：把题面那行（音标行 / 题面行）
   * 传进去，用户点完喇叭立刻能在旁边看到这句是本地 AI 语音还是系统语音读的。
   */
  function speakCurrent() {
    const card = state.card;
    if (!card) return;
    try {
      if (window.Speak) {
        const anchor = document.getElementById('q-phonetic')
          || document.getElementById('q-prompt');
        window.Speak.speak(card.entry.word, {
          audio: card.entry.phonetic && card.entry.phonetic.audio,
          lang: card.entry.lang || 'en',
          anchor,
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

    // 反馈里同样摆出答案词 —— 那就在它旁边放一个朗读按钮。
    // 容器带 data-entry-word，委托就能取到词，不用单独绑事件。
    el.dataset.entryWord = entry.word || '';
    el.dataset.speakLang = entry.lang || 'en';
    el.dataset.speakAudio = (entry.phonetic && entry.phonetic.audio) || card.audio || '';
    // 图标一律走统一的内联 SVG（emoji 的字形宽度跟平台字体走，一排按钮会参差）
    const sayBtn = `<button class="speak-btn" title="朗读" data-speak-accent="us">${U().icon('sound')}</button>`;

    if (correct) {
      const pace = grade === 'hard' ? '答对了，但有点犹豫，已按「较难」安排下次复习。' : '答对了！';
      el.innerHTML = `<span class="fb-icon">${U().icon('check')}</span>
        <div>${pace}
        <div style="margin-top:4px;font-size:12.5px;opacity:.85">
          <b>${U().esc(entry.word)}</b>${sayBtn} —— ${U().esc(def)}
        </div></div>`;
    } else if (grade === 'skip') {
      // ★ P6：主动看答案是中性事件 —— 不套「答错」的红底与「已记录错误」，
      //   如实说明本题不计分、不影响下次复习。
      el.classList.add('skip');
      el.innerHTML = `<span class="fb-icon">${U().icon('minus')}</span>
        <div>已显示答案 · 本题不计分，也不影响下次复习安排
        <div style="margin-top:4px;font-size:12.5px;opacity:.85">
          <b>${U().esc(entry.word)}</b>${sayBtn} —— ${U().esc(def)}
        </div></div>`;
    } else {
      /* ★ 答错的提示必须**诚实**。
         原来的文案是「该词已加入强化记忆，稍后会再考你一次」，但当时的实现
         是 `s.index += 1` 无条件推进 —— 本轮根本不会再遇到它，这句是空话。
         现在后端 `requeue_wrong` 会真的把词回插到本轮队列的后面，所以这句话
         只在 `res.requeued === true` 时才显示（见 finishAnswer）。
         还没拿到后端结果之前，先如实说「已记录」。 */
      el.innerHTML = `<span class="fb-icon">${U().icon('cross')}</span>
        <div><b>${U().esc(entry.word)}</b>${sayBtn} —— ${U().esc(def)}
        <div class="fb-requeue hidden" id="fb-requeue">该词已排到本轮后面，过几题还会再考你一次；
          答对后才会按遗忘曲线排下次复习。</div>
        <div class="fb-note" id="fb-note">已记录本次错误。</div></div>`;
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

  /** 连对计数。数字在动，比任何鼓励文案都管用。 */
  function updateStreakUI() {
    const el = document.getElementById('q-streak');
    if (el) el.textContent = state.streak || 0;
    const pill = el && el.parentElement;
    // 连对 ≥3 时高亮一下：这是一个「我在状态里」的信号
    if (pill) pill.classList.toggle('hot', (state.streak || 0) >= 3);
  }

  /**
   * 错词攻坚：答错就地入队，之后**连对** 3 次才出队。
   *
   * 为什么用「连对」而不是「累计答对 3 次」：累计计数会让
   * 对、错、对、错 这种摇摆也慢慢出队，而它其实根本没记住。
   * 连对才出队，恰好等价于「连续几次都想起来了」。
   *
   * @returns {string} 给用户的提示语（空串表示不显示）
   */
  function grindNote(word, correct) {
    if (!word) return '';
    if (!correct) {
      state.grind[word] = 0;
      return `已加入攻坚：这个词要<b>连对 ${state.GRIND_TARGET} 次</b>才出队（答错会重新计数）`;
    }
    const n = state.grind[word];
    if (n === undefined) return '';          // 本来就不在攻坚队列里，不打扰
    const next = n + 1;
    if (next >= state.GRIND_TARGET) {
      delete state.grind[word];
      return `攻坚成功，${word} 已出队 —— 连对 ${next} 次`;
    }
    state.grind[word] = next;
    return `攻坚中：再连对 ${state.GRIND_TARGET - next} 次出队`;
  }

  /* ---------------- 词库下拉 ---------------- */

  async function loadBookOptions() {
    let books = [];
    try { books = await API.listWordbooks(); } catch (e) { return; }
    // 记下每本词库的语言，选词库时才能自动切换界面文案与释义语言
    state.bookLangById = {};
    state.bookNameById = {};
    books.forEach(b => {
      if (!b || !b.id) return;
      state.bookLangById[b.id] = b.lang || 'en';
      state.bookNameById[b.id] = b.name || b.id;
    });

    // 只保留叶子（可背的）词库 + 全库
    const leafs = books.filter(b => b.level >= 2 || b.id === 'root');
    const html = leafs.map(b => {
      const info = langInfo(b.lang);
      return `<option value="${U().esc(b.id)}">${U().esc(b.name)}（${b.word_count || 0} · ${U().esc(info.name)}）</option>`;
    }).join('');
    ['study-book', 'study-book2'].forEach(id => {
      const el = document.getElementById(id);
      if (!el) return;
      el.innerHTML = '<option value="">全部词库</option>' + html;
      el.value = state.bookId || '';
      el.onchange = () => setBook(el.value);
    });
    // 目标范围下拉：**包含父级词库**（「考研核心词汇」这种父级也要能选，
    // 因为目标就是「背完这本书」）。所以这里用的是完整 books，不是 leafs。
    const goalSel = document.getElementById('goal-book');
    if (goalSel) {
      goalSel.innerHTML = '<option value="">全部词库</option>' + books.map(b =>
        `<option value="${U().esc(b.id)}">${U().esc(b.name)}（${b.word_count || 0} · ${U().esc(langInfo(b.lang).name)}）</option>`
      ).join('');
      goalSel.value = state.bookId || '';
    }
    // 下拉建好后，按当前选择刷新一次界面语言
    setBook(state.bookId || '');
  }

  /* ============================================================
     学习目标（需求 15）

     「多少天背完这本书」和「每天背多少个」本质是同一件事的两种说法：
     把剩余词量摊到剩余时间上。所以前端不做任何计算 —— 全部交给后端
     `compute_study_goal` 算好（今日目标 / 已完成 / 还差 / 预计完成日 /
     复习压力 / 是否达标），前端只负责画。

     ★ 为什么坚持「前端不算」：这组数字会同时出现在三处（概览卡、
       主按钮文案、祝贺卡）。三处各算一遍，迟早出现「按钮说还差 3 个、
       点进去却已经完成」这种自相矛盾，而用户对数字的信任是一次性的。
     ============================================================ */

  /** 拉取目标视图并重绘。失败时只清空面板，不弹错（目标不是核心功能）。 */
  async function loadGoal() {
    try {
      state.goal = await API.studyGoal(state.bookId || null, state.bookLang || null);
      state.goalLoaded = true;
    } catch (e) {
      state.goal = null;
    }
    renderGoal();
    return state.goal;
  }

  /** 目标面板的全部渲染。 */
  function renderGoal() {
    const g = state.goal;
    const modeEl = document.getElementById('goal-mode');
    const inputs = document.getElementById('goal-inputs');
    const box = document.getElementById('goal-progress');
    if (!modeEl || !box) return;

    const mode = (g && g.mode) || 'off';

    // 1) 模式按钮
    modeEl.querySelectorAll('.seg-btn').forEach(b => {
      b.classList.toggle('active', b.dataset.goal === mode);
    });
    // 2) 输入项：按模式二选一显示（同时显示会让人以为两个都生效）
    if (inputs) {
      inputs.classList.toggle('hide-days', mode !== 'days');
      inputs.classList.toggle('hide-perday', mode !== 'per_day');
    }
    const daysEl = document.getElementById('goal-days');
    if (daysEl && g && g.days) daysEl.value = String(g.days);
    const perEl = document.getElementById('goal-perday');
    if (perEl && g && g.per_day) perEl.value = String(g.per_day);
    const bookEl = document.getElementById('goal-book');
    if (bookEl && g) bookEl.value = g.book_id || '';

    // 3) 「多背一点」：不设目标时没有「今天的目标」可加，按钮置灰
    const extraBtn = document.getElementById('goal-extra');
    if (extraBtn) {
      extraBtn.disabled = mode === 'off';
      extraBtn.title = mode === 'off'
        ? '先设一个学习目标，「多背一点」才有意义'
        : extraBtn.title;
    }

    // 4) 进度区
    if (!g || mode === 'off') {
      box.innerHTML = '<div class="gp-empty">还没有设定目标。'
        + '选「按天数背完」或「按每日词量」，系统会算出每天该背多少，'
        + '并同步安排之前学过的词的复习。</div>';
      return;
    }
    if (g.total_words === 0) {
      box.innerHTML = '<div class="gp-empty">这本书里还没有单词，先在「词库」页导入或下载一本。</div>';
      return;
    }

    const pct = g.total_words ? Math.min(100, Math.round((g.learned / g.total_words) * 100)) : 0;
    const donePct = g.today_target ? Math.min(100, Math.round((g.today_done / g.today_target) * 100)) : 0;
    const ok = g.today_remaining === 0;

    const facts = [
      { k: '目标范围', v: g.book_name || '全部词库' },
      { k: '今日目标', v: `${g.today_target} 词` },
      { k: '今日已完成', v: `${g.today_done} 词`, cls: ok ? 'ok' : '' },
      { k: '还差', v: g.today_remaining ? `${g.today_remaining} 词` : '已达标', cls: ok ? 'ok' : 'warn' },
      { k: '今天到期', v: `${g.due_today} 词` },
      { k: '未来 7 天复习', v: `${g.review_load_7d} 词` },
    ];
    if (g.eta_date) facts.push({ k: '预计完成', v: g.eta_date });
    if (g.days_left) facts.push({ k: '剩余天数', v: `${g.days_left} 天` });

    // 复习压力提示：多背的词全部会进入后续复习轮，这件事必须写在明处
    let note = '';
    if (g.pressure === 'high') {
      note = `<div class="gp-note">复习压力偏高：今天的目标明显超出你近期的复习能力。`
        + `多背的词会全部进入后续复习轮，明天后天一起压过来 —— 宁可匀速，也别一次冲太猛。</div>`;
    } else if (!g.on_track && g.today_remaining > 0) {
      // ★ 措辞必须与后端的口径一致：eta_date 现在按**目标速率**外推
      //   （不是按今天的瞬时节奏）。沿用「按现在的节奏」会和「30 天目标」
      //   又一次当众打架 —— 今天只背了几个词就报出「2028 年」的正是这句。
      note = `<div class="gp-note">今天还差 ${g.today_remaining} 词达标。`
        + `按当前目标的进度，预计 ${g.eta_date || '—'} 能背完这本书。</div>`;
    } else if (ok) {
      note = `<div class="gp-note calm">今天的目标已经完成 🎉 `
        + `已完成 ${g.today_done} 词，今天到期 ${g.due_today} 词也已排进轮次。</div>`;
    }

    box.innerHTML = `
      <div class="gp-row">
        <div class="gp-bar"><div class="gp-fill${g.pressure === 'high' ? ' warn' : ''}" style="width:${pct}%"></div></div>
        <span class="gp-num">全书进度 ${g.learned} / ${g.total_words}（${pct}%）</span>
      </div>
      <div class="gp-row" style="margin-top:8px">
        <div class="gp-bar"><div class="gp-fill${ok ? '' : ' warn'}" style="width:${donePct}%"></div></div>
        <span class="gp-num">今日 ${g.today_done} / ${g.today_target}</span>
      </div>
      <div class="gp-facts">
        ${facts.map(f => `<div class="gp-fact"><div class="k">${U().esc(f.k)}</div>
          <div class="v ${f.cls || ''}">${U().esc(String(f.v))}</div></div>`).join('')}
      </div>
      ${note}`;
  }

  /** 目标面板的事件绑定（模式切换 / 保存 / 多背一点 / 范围切换）。 */
  function bindGoal() {
    document.getElementById('goal-mode')?.addEventListener('click', async (e) => {
      const b = e.target.closest('.seg-btn');
      if (!b || !b.dataset.goal) return;
      try {
        // 切模式即落盘：这个面板只有三个开关，多一步「保存」纯属折磨。
        // 天数 / 词量沿用后端现值（不传就是不改），只有本次真的要用的那个
        // 值才显式传 —— 避免把另一个模式的值一并覆盖掉。
        const days = parseInt(document.getElementById('goal-days')?.value, 10);
        const per = parseInt(document.getElementById('goal-perday')?.value, 10);
        state.goal = await API.setStudyGoal({
          mode: b.dataset.goal,
          days: Number.isFinite(days) ? days : null,
          perDay: Number.isFinite(per) ? per : null,
          bookId: state.bookId || null,
        });
        state.goalLoaded = true;
        renderGoal();
        U().toast(b.dataset.goal === 'off' ? '已关闭学习目标' : '学习目标已更新', 'ok');
      } catch (err) {
        U().toast(err.message, 'err');
      }
    });

    document.getElementById('goal-save')?.addEventListener('click', async () => {
      const mode = (state.goal && state.goal.mode) || 'off';
      if (mode === 'off') {
        U().toast('先在上面选一种目标模式', 'err');
        return;
      }
      const days = parseInt(document.getElementById('goal-days')?.value, 10);
      const per = parseInt(document.getElementById('goal-perday')?.value, 10);
      if (mode === 'days' && !(days >= 1)) { U().toast('目标天数要大于 0', 'err'); return; }
      if (mode === 'per_day' && !(per >= 1)) { U().toast('每日词量要大于 0', 'err'); return; }
      try {
        state.goal = await API.setStudyGoal({
          mode,
          days: Number.isFinite(days) ? days : null,
          perDay: Number.isFinite(per) ? per : null,
          bookId: state.bookId || null,
        });
        renderGoal();
        U().toast('学习目标已保存', 'ok');
      } catch (err) {
        U().toast(err.message, 'err');
      }
    });

    document.getElementById('goal-book')?.addEventListener('change', async (e) => {
      const id = e.target.value || '';
      // 目标范围与背诵词库是同一件事的两个入口，保持一致
      setBook(id);
      await loadGoal();
    });

    document.getElementById('goal-extra')?.addEventListener('click', () => askExtra(10));

    /* ---- 祝贺卡 ---- */
    document.getElementById('goal-done-close')?.addEventListener('click', hideGoalDone);
    document.getElementById('goal-done-stop')?.addEventListener('click', () => {
      hideGoalDone();
      U().toast('今天就到这里，明天见', 'ok');
    });
    document.getElementById('goal-done-more')?.addEventListener('click', () => {
      const el = document.getElementById('goal-extra-count');
      const n = parseInt(el && el.value, 10) || 10;
      hideGoalDone();
      askExtra(n);
    });
    // 点遮罩空白处关闭
    document.getElementById('goal-done-overlay')?.addEventListener('click', (e) => {
      if (e.target.id === 'goal-done-overlay') hideGoalDone();
    });
  }

  /**
   * 「多背一点」。
   *
   * ★ 加量**必须**如实告知代价：这些词不是背完就算了，它们会全部进入
   *   后续复习轮（后端 `cmd_submit_answer` → `srs::schedule` 一视同仁地
   *   排下次复习）。后端算出的 `pressure` 就是这件事的量化结果，
   *   这里只负责把它讲成人话，不替用户做决定。
   */
  async function askExtra(n) {
    try {
      const before = state.goal;
      const g = await API.extraStudy(n, state.bookId || null, state.bookLang || null);
      state.goal = g;
      renderGoal();
      const high = (g && g.pressure) === 'high';
      const msg = `今天的目标已加到 ${g ? g.today_target : '?'} 词`
        + (high ? '。⚠ 复习压力已经偏高：加背的词会全部进入复习轮，明后天会一起压过来。' : '。');
      U().toast(msg, high ? 'err' : 'ok');
      if (!before || before.today_target === (g && g.today_target)) {
        // 后端没认这笔加量（例如目标已关闭）时给一句解释，别让按钮像是坏的
        if (!g || g.mode === 'off') U().toast('当前没有启用学习目标，加量不会生效', 'err');
      }
      // 加量之后立刻开背 —— 用户点这个按钮的意图就是「现在就多背点」
      start({ size: n });
    } catch (e) {
      U().toast(e.message, 'err');
    }
  }

  /** 今天达标了就弹一次祝贺卡（同一天、同一目标数只弹一次）。 */
  function maybeCelebrate() {
    const g = state.goal;
    if (!g || g.mode === 'off' || !g.today_target) return;
    if (g.today_remaining > 0) return;
    const stamp = `${g.eta_date || ''}|${g.today_target}|${g.today_done >= g.today_target ? 'done' : ''}`;
    if (state.goalCelebrated === stamp) return;
    state.goalCelebrated = stamp;
    showGoalDone(g);
  }

  function showGoalDone(g) {
    const ov = document.getElementById('goal-done-overlay');
    if (!ov) return;
    const stats = document.getElementById('goal-done-stats');
    if (stats) {
      stats.innerHTML = [
        { k: '今日完成', v: `${g.today_done} 词` },
        { k: '今日目标', v: `${g.today_target} 词` },
        { k: '今天到期', v: `${g.due_today} 词` },
        { k: '全书进度', v: `${g.learned} / ${g.total_words}` },
      ].map(x => `<div><div class="k">${U().esc(x.k)}</div>
        <div class="v">${U().esc(x.v)}</div></div>`).join('');
    }
    // 加量区：默认 10 个，并把「代价」直接写在这里
    const extra = document.getElementById('goal-done-extra');
    if (extra) {
      const high = g.pressure === 'high';
      extra.innerHTML = `
        <div class="gp-note${high ? '' : ' calm'}">
          ${high
            ? '当前复习压力已经偏高。再加量的话，这些词会全部进入复习轮，明天的复习量会明显变大。'
            : '多背的词会全部进入复习轮，按遗忘曲线安排后续复习 —— 这是好事，但明天的复习量会相应增加。'}
        </div>
        <div class="extra-row">
          <span class="muted">再多背</span>
          <input type="number" id="goal-extra-count" min="1" max="500" step="1" value="10" />
          <span class="muted">个词</span>
        </div>`;
    }
    const title = document.getElementById('goal-done-title');
    if (title) {
      title.textContent = g.finished ? '这本书背完了！' : '今天的目标完成啦';
    }
    const text = document.getElementById('goal-done-text');
    if (text) {
      text.textContent = g.finished
        ? `「${g.book_name || '全部词库'}」里的单词已经全部学过一轮，接下来靠复习轮把它们记牢。`
        : '今日任务已达标。要不要趁手感还在多背一点？';
    }
    ov.classList.remove('hidden');
  }

  function hideGoalDone() {
    document.getElementById('goal-done-overlay')?.classList.add('hidden');
  }

  /* ============================================================
     底部区域：上一个单词 + 已背单词折叠列表

     两个设计约束：
     1. **收起状态下也要能发音** —— 所以折叠面板的标题栏（summary）里就放
        一个 🔊，读的是最近背过的一个词，不用先展开。
     2. **所有出现单词的地方都要能发音** —— 题面、音标行、反馈、上一个词、
        列表每一行、详情卡，全都带 .speak-btn。取词一律靠事件委托
        （Speak.bindDelegate），容器上挂 data-entry-word / data-speak-word，
        渲染多少次都不会重复绑监听。
     ============================================================ */

  const $id = (id) => document.getElementById(id);

  /** 取词条第一条释义，作为列表里的一行摘要。 */
  function defLine(entry) {
    const s = (entry && entry.senses && entry.senses[0]) || null;
    if (!s) return '';
    return `${s.pos ? s.pos + ' ' : ''}${s.definition || ''}`.trim();
  }

  /**
   * 「上一个单词」。
   *
   * 注意它显示的是**上一题**：每答完一题才 push 进 history，而渲染下一题时
   * history 的最后一笔正好就是刚才那题，所以不需要额外维护「上一个」指针。
   */
  function renderPrev() {
    const box = $id('prev-word');
    if (!box) return;
    const last = state.history[state.history.length - 1];
    if (!last || !last.entry) { box.classList.add('hidden'); return; }
    box.classList.remove('hidden');

    const e = last.entry;
    const audio = (e.phonetic && e.phonetic.audio) || '';
    // 词 / 音频 / 语言挂在容器上：里面的 🔊 靠委托取词
    box.dataset.entryWord = e.word || '';
    box.dataset.speakLang = e.lang || 'en';
    box.dataset.speakAudio = audio;

    const say = $id('pw-say');
    if (say) {
      say.dataset.speakWord = e.word || '';
      say.dataset.speakLang = e.lang || 'en';
      say.dataset.speakAudio = audio;
      say.dataset.speakAccent = 'us';
    }
    const w = $id('pw-word'); if (w) w.textContent = e.word || '';
    const p = $id('pw-phon');
    if (p) p.innerHTML = U().phoneticHtml(e, { size: 'sm', speak: false });
    const d = $id('pw-def');
    if (d) d.textContent = defLine(e) || '（这个词还没有释义）';
  }

  /** 已背列表里的一行。 */
  function rowHtml(rec, idx) {
    const e = (rec && rec.entry) || {};
    const word = e.word || '';
    const audio = (e.phonetic && e.phonetic.audio) || '';
    // 跨会话的学习记录没有「这轮对错」这一笔，标记位留空
    const mark = rec.correct === true ? `<span class="lw-mark ok">${U().icon('check')}</span>`
      : rec.correct === false ? `<span class="lw-mark bad">${U().icon('cross')}</span>` : '';
    return `<div class="lw-row" data-idx="${idx}">
      <button class="speak-btn" title="朗读 ${U().esc(word)}"
        data-speak-word="${U().esc(word)}"
        data-speak-audio="${U().esc(audio)}"
        data-speak-lang="${U().esc(e.lang || 'en')}"
        data-speak-accent="us">${U().icon('sound')}</button>
      <div class="lw-main" data-act="detail" data-idx="${idx}">
        <div class="lw-word-line">
          <b>${U().esc(word)}</b>
          ${U().phoneticHtml(e, { size: 'sm', speak: false })}
          ${mark}
        </div>
        <div class="lw-def">${U().esc(defLine(e) || '（无释义，点「详情」看完整词条）')}</div>
      </div>
      <button class="ghost-btn xs" data-act="detail" data-idx="${idx}">详情</button>
    </div>`;
  }

  /**
   * 批量补齐 recs 缺失的**词库归属**（已背列表要按词库分容器，而 WordRow /
   * entry 都不带归属字段，只能开命令反查）。查过的进缓存永不再查；
   * 查询失败也写空数组 —— 否则每次渲染都会重撞同一个失败请求。
   * 返回「是否拿到了新数据」。
   */
  async function ensureBookRefs(recs) {
    const byLang = new Map();
    for (const rec of recs) {
      const e = (rec && rec.entry) || {};
      if (!e.word) continue;
      const lang = e.lang || 'en';
      const key = lang + '|' + e.word;
      if (state._bookRefs[key] !== undefined) continue;
      if (!byLang.has(lang)) byLang.set(lang, new Set());
      byLang.get(lang).add(e.word);
    }
    if (!byLang.size) return false;
    let got = false;
    for (const [lang, words] of byLang) {
      try {
        const map = await API.wordBookRefs([...words], lang);
        for (const w of words) state._bookRefs[lang + '|' + w] = (map && map[w]) || [];
        got = true;
      } catch (e) {
        for (const w of words) state._bookRefs[lang + '|' + w] = [];
      }
    }
    return got;
  }

  /**
   * 按词库把记录分组成**不同容器** —— 用户反馈「不同词库的背诵单词混到一起」。
   * 归属规则：所选词库优先 → 书名字序第一本 → 「未入词库」兜底组。
   * 一个词属多本书时只进一个组（不重复计数）；
   * 组顺序：所选库 → 其余按书名 → 未入词库永远最后。
   */
  function groupByBook(recs) {
    const pref = state.bookId || '';
    const prefName = pref ? (state.bookNameById[pref] || pref) : '';
    const groups = new Map();
    recs.forEach((rec, idx) => {
      const e = (rec && rec.entry) || {};
      const refs = state._bookRefs[(e.lang || 'en') + '|' + (e.word || '')] || [];
      let key, title, order;
      if (pref && refs.some(r => r.id === pref)) {
        key = ' pref';
        title = prefName;
        order = 0;
      } else if (refs.length) {
        key = ' b:' + refs[0].id;
        title = refs[0].name;
        order = 1;
      } else {
        key = ' none';
        title = '未入词库';
        order = 2;
      }
      if (!groups.has(key)) groups.set(key, { title, order, items: [] });
      groups.get(key).items.push({ rec, idx });
    });
    const out = [...groups.values()];
    out.sort((a, b) => (a.order - b.order) || String(a.title).localeCompare(String(b.title), 'zh'));
    return out;
  }

  /** 分组容器的 HTML：每组一个 `.lw-group`，组头 = 词库名 + 计数。
   *  data-idx 仍用扁平下标（_recs 不变），点「详情」的回查逻辑零改动。 */
  function groupsHtml(groups) {
    return groups.map(g => `
      <div class="lw-group">
        <div class="lw-group-head"><b>${U().esc(g.title)}</b><span>${g.items.length} 词</span></div>
        ${g.items.map(it => rowHtml(it.rec, it.idx)).join('')}
      </div>`).join('');
  }

  /** 渲染整个底部区域（开始一轮时调用）。 */
  function renderFoot() {
    renderPrev();
    renderLearned();
  }

  /**
   * 已背单词列表：本轮 / 最近背过 两段。
   *
   * 渲染完把当次的数组存到 state._recs —— 点击「详情」时按 data-idx 回查，
   * 比每次重新 reverse 一遍 history 更直接，也不怕两段长得一样。
   */
  function renderLearned() {
    const box = $id('learned-list');
    if (!box) return;
    // 代际守卫：归属查询是异步的，期间若又触发了新的渲染（连答两题 /
    // 快速切 tab），晚回来的重渲必须放弃，否则旧数据会盖掉新渲染。
    const gen = ++state._renderGen;

    document.querySelectorAll('#lb-seg .seg-btn').forEach(b => {
      b.classList.toggle('active', b.dataset.scope === state.learnedScope);
    });

    const title = $id('lb-title');
    const hint = $id('lb-hint');
    const spoken = $id('lb-speak-last');

    if (state.learnedScope === 'session') {
      // 最近答的排最上面：用户找的通常是「刚才那个词」
      const recs = [...state.history].reverse();
      state._recs = recs;
      if (title) title.innerHTML = `本轮已背 <b id="lb-count">${recs.length}</b> 个`;
      if (hint) hint.textContent = recs.length
        ? '点开可回看释义，每行都能单独发音'
        : '开始背诵后，这里会按顺序记下每个背过的词';

      const newest = recs[0] && recs[0].entry;
      if (spoken) {
        spoken.classList.toggle('hidden', !newest);
        if (newest) {
          spoken.dataset.speakWord = newest.word || '';
          spoken.dataset.speakLang = newest.lang || 'en';
          spoken.dataset.speakAudio = (newest.phonetic && newest.phonetic.audio) || '';
          spoken.dataset.speakAccent = 'us';
        }
      }

      // 同步首渲（缓存里有什么用什么 —— 行/发音/详情立刻可用，绝不因
      // 归属反查 IPC 空一帧），查回新归属后再原地重渲一次。
      box.innerHTML = recs.length
        ? groupsHtml(groupByBook(recs))
        : '<div class="muted lw-empty">本轮还没有背过的词</div>';
      void ensureBookRefs(recs).then((got) => {
        if (!got || gen !== state._renderGen) return;
        box.innerHTML = recs.length
          ? groupsHtml(groupByBook(recs))
          : '<div class="muted lw-empty">本轮还没有背过的词</div>';
      });
      return;
    }

    // ---- 最近背过（跨会话）----
    if (state.recentLoading) {
      box.innerHTML = '<div class="muted lw-empty">正在读取学习记录…</div>';
      return;
    }
    if (!state.recent) {
      box.innerHTML = '<div class="muted lw-empty">正在读取学习记录…</div>';
      loadRecent();
      return;
    }

    const recs = state.recent.map(r => ({ entry: r.entry || r, correct: null }));
    state._recs = recs;
    if (title) title.innerHTML = `最近背过 <b id="lb-count">${recs.length}</b> 个`;
    if (hint) hint.textContent = '来自本机学习记录，包含以前几轮背过的词';
    if (spoken) {
      const newest = recs[0] && recs[0].entry;
      spoken.classList.toggle('hidden', !newest);
      if (newest) {
        spoken.dataset.speakWord = newest.word || '';
        spoken.dataset.speakLang = newest.lang || 'en';
        spoken.dataset.speakAudio = (newest.phonetic && newest.phonetic.audio) || '';
      }
    }
    // 与本轮列表同一套分容器渲染（200 条跨词库平铺混排就是用户看到的「混」）。
    // 同步首渲 + 后台补归属重渲，理由同上。
    box.innerHTML = recs.length
      ? groupsHtml(groupByBook(recs))
      : '<div class="muted lw-empty">学习记录里还没有背过的词</div>';
    void ensureBookRefs(recs).then((got) => {
      if (!got || gen !== state._renderGen) return;
      box.innerHTML = recs.length
        ? groupsHtml(groupByBook(recs))
        : '<div class="muted lw-empty">学习记录里还没有背过的词</div>';
    });
  }

  /** 读「最近背过」（懒加载，读完缓存；点刷新会清缓存重读）。 */
  async function loadRecent() {
    if (state.recentLoading) return;
    state.recentLoading = true;
    try {
      const lang = state.bookLang || defaultLang();
      const rows = await API.reviewedWords(lang, 200, 0);
      state.recent = Array.isArray(rows) ? rows : [];
    } catch (e) {
      state.recent = [];
      U().toast('读取学习记录失败：' + e.message, 'err');
    }
    state.recentLoading = false;
    if (state.learnedScope === 'recent') renderLearned();
  }

  /** 打开某个已背单词的完整词条。 */
  function openRec(idx) {
    const rec = (state._recs || [])[parseInt(idx, 10)];
    if (!rec || !rec.entry || !window.Detail) return;
    Detail.open(rec.entry, {
      reason: rec.correct === false ? '这轮答错过，再看一遍' : '已背单词的完整词条',
    });
  }

  function onLearnedClick(e) {
    const b = e.target.closest('[data-act]');
    if (b && b.dataset.act === 'detail') openRec(b.dataset.idx);
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

      if (!state.running) return;

      // ---- 已作答：1~8 / A~H 已经没意义（选项禁用），接管的是空格与回车 ----
      if (state.answered) {
        if (e.key === ' ') {
          e.preventDefault();
          speakCurrent();   // 答错的词尤其需要再听一遍
          return;
        }
        // 回车 = 不等 0.9 秒的定时器，立刻下一题。
        // advanceNow 只在等待窗口内存在，收尾或已推进时为 null，此时回车不做事。
        if (e.key === 'Enter' && state.advanceNow) {
          e.preventDefault();
          const go = state.advanceNow;
          go();
        }
        return;
      }

      if (!state.card) return;
      if (meta().typing) return; // 拼写模式由输入框自己处理
      const key = e.key.toUpperCase();
      const idx = '12345678'.indexOf(key) >= 0
        ? parseInt(key, 10) - 1
        : 'ABCDEFGH'.indexOf(key);
      if (idx >= 0) {
        const btns = document.querySelectorAll('#q-options .opt-btn');
        if (btns[idx]) { e.preventDefault(); btns[idx].click(); return; }
      }
      // 空格在**任意**模式下都发音。
      // 原来只在「听音拼写」里生效，而那句判断又排在 `meta().typing` 的 return
      // 之后 —— 听音拼写 typing 为 true，于是它从来没被执行过（死分支）。
      // 其它模式同样需要「先听一遍再选」，所以这里统一放开。
      if (e.key === ' ') {
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

    document.getElementById('study-def-lang')?.addEventListener('change', (e) => setDefLang(e.target.value));

    // 选项个数（需求 14：不再固定 4 个）。改动即落盘，不必点保存 ——
    // 这个值只影响下一题，用户改完马上就能在下一题上验证。
    document.getElementById('opt-option-count')?.addEventListener('change', async (e) => {
      const n = Math.min(8, Math.max(2, parseInt(e.target.value, 10) || 4));
      e.target.value = String(n);
      try {
        const cfg = state.config || (window.App && window.App.config);
        if (cfg && cfg.study) {
          cfg.study.option_count = n;
          state.config = cfg;
          await API.setStudyOptions(cfg.study);
          U().toast(`每题的选项个数已设为 ${n}`, 'ok');
        }
      } catch (err) {
        U().toast(err.message, 'err');
      }
    });

    bindFoot();
    bindKeys();
    bindGoal();
    applyLangUI(defaultLang());
    loadBookOptions();
    loadGoal();
  }

  /** 绑定底部「上一个单词 / 已背单词」这一块。 */
  function bindFoot() {
    // ★ 一次事件委托覆盖整个背诵页：题面、音标行、反馈、上一个词、
    //   已背列表里所有 .speak-btn 都靠它取词。按渲染次数逐个绑监听
    //   必然会重复绑定并泄漏，这条路走不通。
    U().speakBind(document.getElementById('page-study'));

    // 「上一个单词」：点右侧那块打开完整词条
    $id('prev-open')?.addEventListener('click', () => {
      const last = state.history[state.history.length - 1];
      if (last && last.entry && window.Detail) {
        Detail.open(last.entry, { reason: '上一个单词的完整词条' });
      }
    });

    // 本轮 / 最近背过
    document.getElementById('lb-seg')?.addEventListener('click', (e) => {
      const b = e.target.closest('.seg-btn');
      if (!b || !b.dataset.scope) return;
      state.learnedScope = b.dataset.scope;
      renderLearned();
    });

    // 刷新：清掉缓存强制重读。点它同时切到「最近背过」——
    // 只有这一段是查库来的，刷新「本轮」没有意义。
    $id('lb-refresh')?.addEventListener('click', () => {
      state.recent = null;
      if (state.learnedScope !== 'recent') {
        state.learnedScope = 'recent';
        renderLearned();
      } else {
        renderLearned();
      }
    });

    document.getElementById('learned-list')?.addEventListener('click', onLearnedClick);

    // 标题栏里的 🔊：声音由委托负责放，这里只负责**别让它顺手把面板折叠掉**。
    // 委托里已经 preventDefault 过一次，但不同内核下 summary 默认行为的
    // 时机不完全一致，所以点完再校正一次开合状态，成本几乎为零。
    $id('lb-speak-last')?.addEventListener('click', () => {
      const box = $id('learned-box');
      if (!box) return;
      const wasOpen = box.hasAttribute('open');
      setTimeout(() => {
        if (wasOpen) box.setAttribute('open', '');
        else box.removeAttribute('open');
      }, 0);
    });
  }

  return {
    bind, start, end, setMode, setBook, startWithBook, loadBookOptions,
    applyLangUI, setDefLang, langInfo, modePromptLabel, modeButtonText,
    renderPrev, renderLearned, renderFoot, loadRecent, defLine,
    // 今日复习（需求 14）：独立入口 + 独立会话槽位
    startToday, beginRun,
    // 学习目标（需求 15）
    loadGoal, renderGoal, askExtra, maybeCelebrate, showGoalDone, hideGoalDone,
    // 攻坚队列与推进定时器都是**有状态**的，光看代码看不出是否自洽，
    // 必须能直接驱动才能验（冒烟测试靠这两个入口）。
    grindNote, clearAdvance, updateStreakUI,
    state,
  };
})();

window.Study = Study;
