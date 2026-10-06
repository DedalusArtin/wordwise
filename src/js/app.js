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
    s = await API.stats();
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

  function isSidebarView() {
    return new URLSearchParams(location.search).get('view') === 'sidebar';
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

    if (isSidebarView()) {
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

  /* ---------- 主窗口 ---------- */

  async function setupMainView() {
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

    // 关闭 = 隐藏到托盘（不是退出程序）。退出请用托盘菜单。
    document.getElementById('btn-close-win')?.addEventListener('click', () =>
      winAction('隐藏窗口', () => WIN.hide()));

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

    // 界面语言：配置里的 `ui_lang` 优先，读不到就退回本地缓存 / 简体中文。
    // 必须放在拿到配置**之后**做一次全量刷新，把 index.html 里那批静态文案
    // 一起过字典；放早了就会变成「进应用还是中文、改一次设置才变英文」。
    try {
      window.I18n?.init(config && config.ui_lang);
    } catch (e) { /* 字典出错不该拖垮启动，界面退回中文即可 */ }

    // 检测本地模型
    checkLlm();

    // 静默检查更新：后台跑，不阻塞首屏（失败只写进设置页，不弹窗）
    window.Update?.checkOnStart?.();

    // 首屏
    Pages.go('study');

    // 监听托盘「开始复习」
    try {
      await window.WordWiseAPI.listen('app://start-review', () => {
        Pages.go('study');
        Study.start();
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
    });

    setInterval(() => {
      if (Pages.current === 'study' && !Study.state.running) refreshStudy();
    }, 60000);
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
    init, checkLlm, updateLlmChip, refreshConfig,
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
