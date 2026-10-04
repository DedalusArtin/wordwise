/* ============================================================
   app.js —— 应用编排：页面路由、初始化、状态同步
   ============================================================ */

/* ---------------- 页面路由 ---------------- */

const Pages = (() => {
  let current = 'study';

  const loaders = {
    study: () => refreshStudy(),
    lookup: () => {},
    library: () => window.Library.loadList(),
    leech: () => window.Leech.load(),
    plan: () => window.Plan.load(),
    stats: () => window.Stats.load(),
    settings: () => window.Settings.load(),
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

/* ---------------- 应用主控 ---------------- */

const App = (() => {
  const { API, HAS_TAURI } = window.WordWiseAPI;
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
    document.getElementById('btn-minimize')?.addEventListener('click', async () => {
      if (HAS_TAURI) {
        const w = window.__TAURI__.window?.getCurrentWindow?.();
        if (w) await w.minimize();
      }
    });
    document.getElementById('btn-close-win')?.addEventListener('click', async () => {
      if (HAS_TAURI) {
        const w = window.__TAURI__.window?.getCurrentWindow?.();
        if (w) await w.hide();
      }
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

    // 模块初始化
    Study.bind();
    Detail.bind();
    Lookup.bind();
    Library.bind();
    Settings.bind();

    // 初始化配置
    try {
      config = await API.getConfig();
      const batch = document.getElementById('opt-batch');
      if (batch && config.study) batch.value = config.study.batch_size;
      const seg = document.querySelector('#mode-seg .seg-btn');
      if (seg) Study.setMode('en_to_zh');
    } catch (e) { /* 用默认 */ }

    // 检测本地模型
    checkLlm();

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
      if ((e.ctrlKey || e.metaKey) && e.key >= '1' && e.key <= '7') {
        const pages = ['study', 'lookup', 'library', 'leech', 'plan', 'stats', 'settings'];
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
    if (st.online) {
      dot.classList.add('online');
      text.textContent = st.active_model
        ? '本地模型已就绪'
        : '已连接（无模型）';
    } else {
      dot.classList.add('offline');
      text.textContent = '本地模型未连接';
    }
    const chip = document.getElementById('llm-chip');
    if (chip) chip.title = st.message || '';
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

  // 点击 LLM 状态条重新检测
  document.getElementById('llm-chip')?.addEventListener('click', () => App.checkLlm());
});

window.Pages = Pages;
window.App = App;
