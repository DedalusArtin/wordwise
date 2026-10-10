/* ============================================================
   app.js —— 应用编排：页面路由、初始化、状态同步
   ============================================================ */

/* ---------------- 页面路由 ---------------- */

const Pages = (() => {
  let current = 'study';

  const loaders = {
    study: () => { refreshStudy(); window.Study.loadBookOptions(); },
    lookup: () => {},
    translate: () => window.Translate.load(),
    graph: () => window.Graph.load(),
    library: () => {
      window.Books.loadBooks();
      window.Library.loadList();
    },
    leech: () => window.Leech.load(),
    plan: () => window.Plan.load(),
    stats: () => window.Stats.load(),
    settings: () => {
      window.Settings.load();
      window.Maint.load();
      // 版本号 + 在线更新面板。版本号在 applyVersion 里统一填，
      // 打开设置页时再填一次是为了兜住「启动时那次请求失败」的情况。
      window.Update?.load?.();
    },
  };

  function go(name) {
    if (!name) return;
    current = name;
    document.querySelectorAll('.nav-item').forEach(n => {
      n.classList.toggle('active', n.dataset.page === name);
    });
    document.querySelectorAll('.page').forEach(p => {
      p.classList.toggle('active', p.id === 'page-' + name);
    });
    document.querySelector('.content')?.scrollTo({ top: 0, behavior: 'smooth' });
    try { loaders[name]?.(); } catch (e) { console.error(e); }
  }

  return { go, get current() { return current; }, refreshStudy };
})();

/* ---------------- 背诵页概览 ---------------- */

async function refreshStudy() {
  const { API } = window.WordWiseAPI;
  const U = window.WW;

  let s;
  try {
    // ★ 按所选词库取统计：四张卡与「开始今日复习（N 词）」的 N 只数该书的词，
    //   否则数字是全库的、点进去却只背所选书，两个口径当众打架。
    //   词库页（library.js）仍用无参调用取全库统计，不受影响。
    s = await API.stats(null, (window.Study && window.Study.state && window.Study.state.bookId) || null);
  } catch (e) {
    return;
  }

  const set = (id, v) => {
    const el = document.getElementById(id);
    if (el) el.textContent = v;
  };
  set('s-due', s.due_today);
  set('s-leech', s.leeches);
  set('s-mastered', s.mastered);
  set('s-streak', s.streak_days);

  // ---- 今日复习统一入口 ----
  //
  //  为什么要把「到期的词」做成主按钮上的一个数字：原来页面上只有泛泛的
  //  「开始背诵」，用户得自己看懂「待复习 / 强化记忆」两张卡、在心里加起来，
  //  才知道今天到底该背多少。
  //
  //  ★ 这里曾经把 todo 算成 `due_today + leeches`，而 leech 是 due 的**子集**，
  //    于是按钮上的数字天生偏大；点进去还会走背诵接口、被补足到「每轮题量」
  //    （12 变 20）。用户对数字的信任是一次性的，所以现在：
  //      - 数字只用 `due_today`（与复习队列**同一个 SQL 口径**，不重不漏）；
  //      - 点击走 `cmd_start_review_session`（独立槽位，绝不补足）。
  //    「常错词的攻坚」有它自己的入口（错词本页 + 勾选框），不在这里重复计入。
  const due = s.due_today || 0;

  // 目标视图：今日目标 - 今日已完成 = 还要背多少「新词 + 到期」。
  // 目标拉不到（老后端 / 数据库异常）不该拖垮整个概览，退回纯复习入口即可。
  let goal = null;
  try {
    goal = await API.studyGoal(window.Study?.state?.bookId || null,
                              window.Study?.state?.bookLang || null);
    if (window.Study) {
      window.Study.state.goal = goal;
      window.Study.state.goalLoaded = true;
      window.Study.renderGoal();
    }
  } catch (e) { /* 目标不是核心功能，失败就静默 */ }

  const goalOn = !!(goal && goal.mode && goal.mode !== 'off');
  // 今天还需要背多少（含新词）。已经到期的词优先，剩下的才是新词额度。
  const goalLeft = goalOn ? Math.max(0, goal.today_remaining || 0) : 0;
  const newLeft = Math.max(0, goalLeft - due);

  const cta = document.getElementById('btn-today');
  if (cta) {
    if (due > 0) {
      cta.textContent = `开始今日复习（${due} 词）`;
      cta.dataset.mode = 'review';
      cta.dataset.size = String(due);
    } else if (newLeft > 0) {
      cta.textContent = `背今天的新词（${newLeft} 词）`;
      cta.dataset.mode = 'study';
      cta.dataset.size = String(newLeft);
    } else {
      cta.textContent = goalOn ? '今天的目标已完成' : '开始背诵';
      cta.dataset.mode = goalOn ? 'done' : 'study';
      cta.dataset.size = '';
    }
  }
  const hint = document.getElementById('today-hint');
  if (hint) {
    if (due > 0) {
      hint.textContent = `今天有 ${due} 个词到期（含逾期的）。点进去就是这 ${due} 个，`
        + `不会掺新词、也不会被补足到别的一轮题量。`;
    } else if (newLeft > 0) {
      hint.textContent = `今天到期的词已经清完了，按目标还剩 ${newLeft} 个新词。`;
    } else if (goalOn) {
      hint.textContent = `今天的目标（${goal.today_target} 词）已经完成，`
        + `已完成 ${goal.today_done} 词。想多背可以在下面「多背一点」。`;
    } else {
      hint.textContent = '今天没有到期词，可以学点新词，或者去错词本复习常错的词。';
    }
  }

  const greet = document.getElementById('study-greeting');
  if (greet) {
    const info = window.App && window.App.info;
    const g = (info && info.greeting) || '你好';
    if (s.due_today > 0) {
      greet.textContent = `${g}，今天有 ${s.due_today} 个词等着复习`;
    } else if (s.total_words === 0) {
      greet.textContent = `${g}，先导入一些单词开始吧`;
    } else {
      greet.textContent = `${g}，今日复习已完成，可以学点新词`;
    }
  }

  U.renderBarChart(document.getElementById('study-chart'), s.history);

  // 错词角标
  const badge = document.getElementById('nav-leech-badge');
  if (badge) {
    if (s.leeches > 0) {
      badge.textContent = s.leeches > 99 ? '99+' : s.leeches;
      badge.classList.remove('hidden');
    } else {
      badge.classList.add('hidden');
    }
  }
}

/* ---------------- 窗口控制 ---------------- */

/**
 * 把自绘标题栏的「最大化」图标同步成当前状态：
 * 已最大化显示「还原」图标（❐），否则显示「最大化」图标（☐）。
 *
 * 注意这里是 IIFE 外的顶层函数，拿不到 App 里解构出来的 WIN / U，
 * 只能从 window 上取。
 */
async function syncMaxIcon() {
  const btn = document.getElementById('btn-maximize');
  if (!btn) return;
  const W = window.WordWiseAPI && window.WordWiseAPI.WIN;
  if (!W) return;
  let maxed = false;
  try { maxed = await W.isMaximized(); } catch (e) { return; }
  btn.innerHTML = maxed ? '&#10098;' : '&#9633;';
  btn.title = maxed ? '还原' : '最大化';
}

/**
 * 包一层：窗口操作失败必须说出来。
 *
 * 之前这四个动作直接把异常丢在 async 回调里，一旦底层不可用（比如
 * window.__TAURI__ 不存在）就彻底静默 —— 用户看到的是「按钮点了没反应」，
 * 我们这边一点线索都没有。宁可弹个 toast 难看，也不要这种哑失败。
 */
async function winAction(label, fn) {
  if (!(window.WordWiseAPI && window.WordWiseAPI.HAS_TAURI)) return;
  try {
    await fn();
  } catch (e) {
    const msg = (e && e.message) ? e.message : String(e);
    if (window.WW && window.WW.toast) window.WW.toast(`${label}失败：${msg}`, 'err');
  }
}

/* ---------------- 应用主控 ---------------- */

const App = (() => {
  const { API, HAS_TAURI, WIN } = window.WordWiseAPI;
  const U = () => window.WW;

  let info = null;
  let config = null;
  let llmStatus = null;

  /**
   * 当前窗口是哪种视图：`main` / `sidebar` / `mini`。
   *
   * 三个窗口加载的是**同一个 index.html**，靠 `?view=` 区分 —— 所以这个
   * 判断必须只有一个实现。以前 `app.js` 和 `api.js` 各写了一份判 sidebar 的
   * 逻辑，加第三个窗口时必然只改一处、漏一处。
   */
  function viewKind() {
    let v = '';
    try {
      v = new URLSearchParams(location.search).get('view') || '';
    } catch (e) { /* 解析不了就当主窗口 */ }
    if (v === 'sidebar') return 'sidebar';
    if (v === 'mini') return 'mini';
    return 'main';
  }

  function isSidebarView() {
    return viewKind() === 'sidebar';
  }

  async function init() {
    try {
      info = await API.appInfo();
    } catch (e) {
      console.warn('获取应用信息失败', e);
    }
    // ★ 版本号只有这一个来源（后端 CARGO_PKG_VERSION）。页面上所有
    //   [data-app-ver] 节点都在这里被一次性填好，前端不留版本字面量。
    //   侧边栏窗口走的是同一个 index.html，所以也会执行到这一行。
    window.Update?.applyVersion?.(info && info.version);

    // 迷你悬浮窗：同样要先 init i18n —— 否则主窗口切成英文后，
    // 小窗还是一整屏中文（与侧边栏此前那个「语言乱飘」是同一个病根）。
    if (viewKind() === 'mini') {
      try { window.I18n?.init(); } catch (e) { /* 字典坏了别挡小窗 */ }
      setupMiniView();
      return;
    }

    if (isSidebarView()) {
      /* ★ R3 修复：侧栏窗口此前在这里提前 return，i18n 从没初始化过 ——
         主窗切成英文后侧栏还是一整屏中文（「语言乱飘」的来源之一）。
         侧栏读不到 config（setupMainView 才拉配置），先用 localStorage
         上色；主窗改语言会写 LS，storage 事件（i18n.js init 挂的）随后
         还会把变化实时推过来。 */
      try { window.I18n?.init(); } catch (e) { /* 字典坏了别挡侧栏 */ }
      setupSidebarView();
      return;
    }

    setupMainView();
  }

  /* ---------- 侧边栏窗口 ---------- */

  function setupSidebarView() {
    document.body.dataset.view = 'sidebar';
    document.getElementById('main-shell').classList.add('hidden');
    document.getElementById('sidebar-shell').classList.remove('hidden');
    document.getElementById('titlebar').classList.add('hidden');

    window.Sidebar.bind();

    // 恢复上次查询
    const input = document.getElementById('sb-input');
    if (input) setTimeout(() => input.focus(), 120);

    // 有词库内容时自动出一道题，让侧边栏一打开就有用
    API.wordCount().then(n => {
      if (n > 0) Sidebar.quickQuiz();
    }).catch(() => {});
  }

  /* ---------- 迷你悬浮窗 ---------- */

  function setupMiniView() {
    document.body.dataset.view = 'mini';
    document.getElementById('main-shell')?.classList.add('hidden');
    document.getElementById('sidebar-shell')?.classList.add('hidden');
    document.getElementById('mini-shell')?.classList.remove('hidden');
    document.getElementById('titlebar')?.classList.add('hidden');

    window.Mini?.bind();
    // 一打开就出一个词：悬浮窗的价值就是「抬手就能背一个」
    window.Mini?.next();
    window.Mini?.refreshProgress();
  }

  /* ---------- 主窗口 ---------- */

  async function setupMainView() {
    /* ★ R4：先用 localStorage 立即上色（不等 IPC —— getConfig 冷启动要几十毫秒，
       期间首屏闪一下中文又变英文很难受）。语言下拉现在 LS 与后端 config
       双写同步（cmd_set_ui_lang），正常情况两次结果一致，下面那次「校准」
       是空操作；只有 config 被外部改动/损坏时才真正生效。 */
    try { window.I18n?.init(); } catch (e) { /* 同上 */ }

    document.querySelectorAll('.nav-item').forEach(n => {
      n.addEventListener('click', () => Pages.go(n.dataset.page));
    });

    // 窗口控制
    //
    // ★ 一律走 WIN.*（内部是 invoke），不要用 window.__TAURI__.window ——
    //   本项目没开 withGlobalTauri，那个全局对象根本不存在，写成它会抛
    //   TypeError 并被 async 回调吞掉，按钮表现为「点了没反应」。
    document.getElementById('btn-minimize')?.addEventListener('click', () =>
      winAction('最小化', () => WIN.minimize()));

    // 主窗口已改为无边框（自绘标题栏），系统标题栏的「最大化/还原」没了，
    // 这里补一个自绘按钮，并把图标在 ☐ / ❐ 之间切换。
    document.getElementById('btn-maximize')?.addEventListener('click', () =>
      winAction('最大化', async () => {
        await WIN.toggleMaximize();
        await syncMaxIcon();
      }));

    // 双击标题栏最大化/还原，符合 Windows 操作习惯
    document.getElementById('titlebar')?.addEventListener('dblclick', (e) => {
      if (e.target.closest('button') || e.target.closest('select')) return;
      winAction('最大化', async () => {
        await WIN.toggleMaximize();
        await syncMaxIcon();
      });
    });

    // 关闭行为可选（设置 → 记忆辅助内容）：默认缩小到托盘（后台词库增强、
    // 复习提醒继续跑）；关掉开关则真正退出（设置 → 关于与更新也可退出）。
    document.getElementById('btn-close-win')?.addEventListener('click', async () => {
      // ★ 现读现判：设置页改完开关立刻生效，不等重启（此前用的是启动时的
      //   缓存，「改了开关也白改」的另一半根因）。读失败用缓存兜底。
      try { config = await API.getConfig(); } catch (e) { /* 用缓存 */ }
      const toTray = !(config && config.study && config.study.close_to_tray === false);
      if (toTray) winAction('隐藏窗口', () => WIN.hide());
      else winAction('退出', () => API.appExit());
    });

    document.getElementById('btn-open-sidebar')?.addEventListener('click', async () => {
      try {
        await API.sidebarShow();
        U().toast('侧边栏已显示，可随时查词', 'ok');
      } catch (e) { U().toast(e.message, 'err'); }
    });

    // 载入示例词库
    document.getElementById('btn-seed')?.addEventListener('click', async () => {
      try {
        const n = await API.seedDemo();
        U().toast(`已载入 ${n} 个示例单词`, 'ok');
        refreshStudy();
      } catch (e) { U().toast(e.message, 'err'); }
    });

    // 今日复习统一入口：进站即背，省掉「先读懂两张卡再决定背多少」这一步。
    //
    // ★ 两种去向互不混用（需求 14）：
    //   data-mode="review" → 走 `cmd_start_review_session`（只出今天到期的词，
    //     独立会话槽位，绝不补足到 batch_size）
    //   data-mode="study"  → 按目标剩余量开一轮新词（背诵槽位）
    //   已经在背的时候不重开一轮 —— 重开会丢弃当前进度，这时只把页面切过去。
    document.getElementById('btn-today')?.addEventListener('click', () => {
      Pages.go('study');
      const running = window.Study && window.Study.state && window.Study.state.running;
      if (running) {
        document.getElementById('quiz-card')
          ?.scrollIntoView({ behavior: 'smooth', block: 'center' });
        return;
      }
      const btn = document.getElementById('btn-today');
      const mode = (btn && btn.dataset.mode) || 'study';
      if (mode === 'done') {
        U().toast('今天的目标已经完成，想多背就在下面点「多背一点」', 'ok');
        return;
      }
      if (mode === 'review') {
        Study.startToday();
        return;
      }
      const size = parseInt((btn && btn.dataset.size) || '', 10);
      Study.start(Number.isFinite(size) && size > 0 ? { size } : {});
    });

    // 互译方向选择器要先于各页面绑定：查词页与翻译页都靠它驱动
    await window.DirPicker.loadLangs();
    window.DirPicker.bind();

    // 模块初始化
    Study.bind();
    Detail.bind();
    Lookup.bind();
    Library.bind();
    Books.bind();
    Settings.bind();
    window.Translate.bind();
    window.Leech.bind();
    window.Plan.bind();
    window.Graph.bind();
    window.Demo.bind();
    window.Maint.bind();

    // 初始化配置
    try {
      config = await API.getConfig();
      const batch = document.getElementById('opt-batch');
      if (batch && config.study) batch.value = config.study.batch_size;
      const seg = document.querySelector('#mode-seg .seg-btn');
      if (seg) Study.setMode('en_to_zh');
    } catch (e) { /* 用默认 */ }

    // 界面语言的**校准**（R4 下半）：setupMainView 开头已经用 localStorage
    // 上过一次色，这里拿到真实配置后再校准一次 —— 两源双写同步后正常是
    // 空操作；只在 config 被外部改动/本地缓存损坏时才真正生效。
    try {
      window.I18n?.init(config && config.ui_lang);
    } catch (e) { /* 字典出错不该拖垮启动，界面退回中文即可 */ }

    // 全局错误上报到日志：排障诊断的数据源之一（节流 1 条/秒防刷屏）
    let lastErrLog = 0;
    window.addEventListener('error', (e) => {
      const now = Date.now();
      if (now - lastErrLog < 1000) return;
      lastErrLog = now;
      API.logWrite?.('error', '[js] ' + ((e && e.message) || '未知错误'));
    });
    window.addEventListener('unhandledrejection', (e) => {
      const now = Date.now();
      if (now - lastErrLog < 1000) return;
      lastErrLog = now;
      API.logWrite?.('error', '[js promise] ' + String((e && e.reason && (e.reason.message || e.reason)) || '未处理的 Promise 拒绝').slice(0, 300));
    });

    // 检测本地模型
    checkLlm();
    // 语音载入情况（侧边栏常驻，与模型状态并排）
    checkTts();

    // 静默检查更新：后台跑，不阻塞首屏（失败只写进设置页，不弹窗）
    window.Update?.checkOnStart?.();

    // 首屏
    Pages.go('study');

    // 监听托盘「开始复习」
    // 与页面上那个主按钮走**完全同一条**路径（直接触发它的点击）：
    // 托盘菜单和界面按钮对「今天该背什么」的判断必须一致，
    // 否则从托盘进来背的是新词、从界面进来背的是复习词。
    try {
      await window.WordWiseAPI.listen('app://start-review', () => {
        Pages.go('study');
        document.getElementById('btn-today')?.click();
      });
    } catch (e) { /* 忽略 */ }

    // 键盘快捷键：Ctrl+1..7 切换页面
    document.addEventListener('keydown', (e) => {
      // 在输入框里按 Ctrl+数字 不该跳页
      if (window.WW && window.WW.isTypingTarget && window.WW.isTypingTarget(e.target)) return;
      // 1..9 对应导航栏顺序；新增栏目时记得同步这里，否则会按不到
      if ((e.ctrlKey || e.metaKey) && e.key >= '1' && e.key <= '9') {
        const pages = ['study', 'lookup', 'translate', 'graph', 'library',
                       'leech', 'plan', 'stats', 'settings'];
        const p = pages[parseInt(e.key, 10) - 1];
        if (p) { e.preventDefault(); Pages.go(p); }
      }
      // Ctrl+M 呼出/收起迷你背词窗：主界面之外最顺手的一条路
      // （另两条是托盘菜单，以及小窗自己的关闭按钮）。
      if ((e.ctrlKey || e.metaKey) && (e.key === 'm' || e.key === 'M')) {
        e.preventDefault();
        API.miniToggle().catch(() => {});
      }
    });

    setInterval(() => {
      if (Pages.current === 'study' && !Study.state.running) refreshStudy();
    }, 60000);
  }

  /* ---------- 语音（TTS）载入情况 ----------

     用户原话：「明明已经载入成功但是没有读……在边上显示语音载入情况」。
     之前「语音包装没装好」只有进设置页才知道 —— 而朗读发生在查词页、
     背诵页的每一次点击上。这里与「本地模型」状态并排，常驻侧边栏。 */
  async function checkTts() {
    const dot = document.getElementById('tts-dot');
    const text = document.getElementById('tts-text');
    const chip = document.getElementById('tts-chip');
    if (!dot || !text) return;
    if (!API.ttsStatus) return;
    let st = null;
    try { st = await API.ttsStatus(); } catch (e) { /* 后端没就绪就先不显示 */ }
    if (!st) return;
    dot.className = 'dot';
    const engineOk = !!st.engine_ready;
    const n = (st.installed || []).length;
    if (!engineOk) {
      dot.classList.add('offline');
      text.textContent = '语音引擎未安装';
    } else if (n > 0) {
      dot.classList.add('online');
      const cur = (st.config && st.config.voice_local) || '';
      const label = cur
        ? ((st.voices || []).find(v => v.id === cur) || { label: cur }).label
        : '自动';
      text.textContent = `语音已就绪 · ${label}`;
      chip && (chip.title = `已装 ${n} 个语音包，点击进设置管理`);
    } else {
      dot.classList.add('offline');
      text.textContent = '语音包未安装';
    }
    if (chip && !engineOk) chip.title = '到「设置 → 朗读」先下载语音引擎';

    // 朗读链路失败 → 状态条立即转红并写明原因（覆盖「已就绪」——
    // 「文件都在却发不出声」正是用户撞上的盲区，状态必须跟着失败走）。
    // 成功朗读后 Speak 自己会清回正常态（healthOk）。
    if (!checkTts._healthBound && window.Speak && window.Speak.onHealth) {
      checkTts._healthBound = true;
      window.Speak.onHealth((h) => {
        const d = document.getElementById('tts-dot');
        const t = document.getElementById('tts-text');
        const c = document.getElementById('tts-chip');
        if (!d || !t) return;
        if (h && h.state === 'error') {
          d.className = 'dot offline';
          t.textContent = '语音出错：' + (h.message || '').slice(0, 26);
          if (c) c.title = h.message || '';
        } else {
          checkTts();   // 恢复正常态 → 重新按后端真实状态渲染
        }
      });
    }
  }

  /* ---------- 本地模型状态 ---------- */

  async function checkLlm() {
    updateLlmChip({ online: false, message: '检测中…', checking: true });
    try {
      const st = await API.llmStatus();
      llmStatus = st;
      updateLlmChip(st);
      const modelEl = document.getElementById('ai-model');
      if (modelEl) {
        modelEl.textContent = st.active_model
          ? truncateModel(st.active_model)
          : (st.online ? '未加载模型' : '离线');
      }
    } catch (e) {
      updateLlmChip({ online: false, message: '检测失败' });
    }
  }

  function truncateModel(m) {
    const s = String(m);
    return s.length > 24 ? s.slice(0, 22) + '…' : s;
  }

  function updateLlmChip(st) {
    const dot = document.getElementById('llm-dot');
    const text = document.getElementById('llm-text');
    if (!dot || !text) return;

    dot.className = 'dot';
    if (st.checking) {
      dot.classList.add('checking');
      text.textContent = '检测中…';
      return;
    }
    // 说「本地模型未连接」而用户配的是在线 API，会让人去 LM Studio 里白找一圈。
    // 按后端给的 endpoint_kind 分开措辞。
    const cloud = st.endpoint_kind === 'cloud';
    const label = cloud ? '在线 API' : '本地模型';
    if (st.online) {
      dot.classList.add('online');
      text.textContent = st.active_model ? `${label}已就绪` : '已连接（无模型）';
    } else {
      dot.classList.add('offline');
      text.textContent = `${label}未连接`;
    }
    const chip = document.getElementById('llm-chip');
    // ★ 别用 `chip.title = st.message` 直接覆盖：那会把 HTML 里那句
    //   「点击查看 AI 服务」抹掉，用户就再也看不出这块是可以点的。
    if (chip) {
      chip.title = (st.message ? st.message + '　·　' : '') + '点击查看 AI 服务';
    }
  }

  function refreshConfig() {
    API.getConfig().then(c => { config = c; }).catch(() => {});
  }

  return {
    init, checkLlm, checkTts, updateLlmChip, refreshConfig,
    get info() { return info; },
    get config() { return config; },
    get llmStatus() { return llmStatus; },
  };
})();

/* ---------------- 启动 ---------------- */

document.addEventListener('DOMContentLoaded', () => {
  App.init();
  // 左下角状态条的点击行为（展开本地模型弹层）由 Maint.chip 负责，
  // 这里只做绑定；两者都挂在同一个元素上会互相打架，所以别再单独加监听。
  window.Maint?.bind?.();
});

window.Pages = Pages;
window.App = App;
