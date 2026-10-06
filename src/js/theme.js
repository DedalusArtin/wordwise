/* ============================================================
   theme.js —— 主题（明暗 × 主题色）

   ★ 这个文件必须在 <head> 里、其它脚本之前同步加载。
     原因是它要在**第一次绘制之前**把 data-mode / data-accent 写到 <html> 上，
     否则深色模式启动时会先闪一帧纯白，看得人眼睛疼。

   两轴设计：
     mode   浅色 / 深色 / 跟随系统
     accent 主题色（经典蓝 / 青松 / 紫罗兰 / 琥珀 / 玫红）
   两者独立组合，共 15 种外观，但 CSS 里只需要一份 token 覆盖表 ——
   布局完全不跟着变（见 app.css 末尾的「主题层」）。

   偏好存在 localStorage，不是后端配置：
   它是纯界面偏好，换台机器没有带过去的必要；而且必须能同步读到，
   走一次 IPC 就会重新引入上面那个「闪一帧」的问题。

   主窗口与侧边栏窗口加载的是同一个 index.html，localStorage 同源，
   所以两个窗口的主题是同一份、天然同步。
   ============================================================ */

const Theme = (() => {
  const KEY = 'ww.theme';

  const MODES = [
    { id: 'light', name: '浅色', icon: 'sun' },
    { id: 'dark', name: '深色', icon: 'moon' },
    { id: 'system', name: '跟随系统', icon: 'system' },
  ];

  /** 主题色。dot 只用于设置页里的色块，真正的取值在 app.css 里。 */
  const ACCENTS = [
    { id: 'blue', name: '经典蓝', dot: '#1a6ce8' },
    { id: 'teal', name: '青松', dot: '#0d9f9a' },
    { id: 'purple', name: '紫罗兰', dot: '#7c5cff' },
    { id: 'amber', name: '琥珀', dot: '#e08a00' },
    { id: 'rose', name: '玫红', dot: '#e0457b' },
  ];

  const DEFAULTS = { mode: 'light', accent: 'blue' };

  /* ---------------- 读写偏好 ---------------- */

  function read() {
    try {
      const raw = localStorage.getItem(KEY);
      if (!raw) return { ...DEFAULTS };
      const o = JSON.parse(raw) || {};
      return {
        mode: MODES.some((m) => m.id === o.mode) ? o.mode : DEFAULTS.mode,
        accent: ACCENTS.some((a) => a.id === o.accent) ? o.accent : DEFAULTS.accent,
      };
    } catch (e) {
      return { ...DEFAULTS };
    }
  }

  function write(t) {
    try { localStorage.setItem(KEY, JSON.stringify(t)); } catch (e) { /* 隐私模式等，忽略 */ }
  }

  /** 「跟随系统」要落到一个具体的 light / dark 上。 */
  function resolvedMode(mode) {
    if (mode !== 'system') return mode === 'dark' ? 'dark' : 'light';
    try {
      return window.matchMedia && window.matchMedia('(prefers-color-scheme: dark)').matches
        ? 'dark' : 'light';
    } catch (e) {
      return 'light';
    }
  }

  /* ---------------- 应用 ---------------- */

  let cur = read();

  function apply() {
    const el = document.documentElement;
    if (!el || !el.dataset) return;
    const resolved = resolvedMode(cur.mode);
    el.dataset.mode = resolved;       // CSS 认这个
    el.dataset.modePref = cur.mode;   // 「跟随系统」时给设置页回显用
    el.dataset.accent = cur.accent;
    // 让原生控件（滚动条、下拉、日期选择器）也跟着换配色
    try { el.style.colorScheme = resolved; } catch (e) { /* 忽略 */ }
  }

  // ★ 立即应用：这一行是「不闪白」的关键
  apply();

  /* ---------------- 界面 ---------------- */

  let listeners = [];

  function emit() {
    const detail = { ...cur, resolved: resolvedMode(cur.mode) };
    listeners.forEach((fn) => { try { fn(detail); } catch (e) { /* 忽略 */ } });
    try {
      document.dispatchEvent(new CustomEvent('ww:theme', { detail }));
    } catch (e) { /* 旧环境没有 CustomEvent 也无所谓 */ }
  }

  function set(patch) {
    cur = { ...cur, ...patch };
    write(cur);
    apply();
    syncUI();
    emit();
  }

  /** 在浅色与深色之间来回切（「跟随系统」时切到当前的反面）。 */
  function toggle() {
    set({ mode: resolvedMode(cur.mode) === 'dark' ? 'light' : 'dark' });
  }

  /**
   * 取一个内联 SVG 图标。
   *
   * ★ 图标统一由 ui.js 的 `icon()` 提供，这里**不再自己写字符**：
   *   原来用的是 `\u2600`（☀）/ `\u263E`（☾）/ `\u25CE`（◎）三个 BMP 字符，
   *   宽度由平台字体决定，与导航栏那排图标不是一个视觉重量。
   *
   * 为什么带兜底：theme.js 在 <head> 里加载（要赶在首屏绘制前写 data-mode），
   * 而 ui.js 在 body 末尾。本文件只在 DOMContentLoaded 之后才渲染按钮，
   * 那时 ui.js 已经执行完 —— 但兜底仍然留着：图标缺失只该少一个图形，
   * 绝不该让主题按钮整个哑掉。
   */
  function ico(name) {
    return (window.WW && window.WW.icon) ? window.WW.icon(name) : '';
  }

  /** 把当前主题回显到设置页的按钮上。 */
  function syncUI() {
    const resolved = resolvedMode(cur.mode);
    document.querySelectorAll('#theme-modes .seg-btn').forEach((b) => {
      b.classList.toggle('active', b.dataset.mode === cur.mode);
    });
    document.querySelectorAll('#theme-accents .accent-dot').forEach((b) => {
      b.classList.toggle('active', b.dataset.accent === cur.accent);
    });
    const quick = document.getElementById('btn-theme-quick');
    if (quick) {
      quick.innerHTML = ico(resolved === 'dark' ? 'sun' : 'moon')
        + (resolved === 'dark' ? '浅色' : '深色');
      quick.title = '在浅色 / 深色之间切换（当前：' + (MODES.find((m) => m.id === cur.mode) || {}).name + '）';
    }
    // 侧边栏/标题栏上的那个小按钮也要跟着变
    const chip = document.getElementById('btn-theme-toggle');
    if (chip) {
      chip.innerHTML = ico(resolved === 'dark' ? 'sun' : 'moon');
      chip.title = resolved === 'dark' ? '切换到浅色' : '切换到深色';
    }
  }

  /** 渲染设置页里的两个选择器（只在有对应容器时才动）。 */
  function renderUI() {
    const modes = document.getElementById('theme-modes');
    if (modes && !modes.dataset.built) {
      modes.dataset.built = '1';
      modes.innerHTML = MODES.map((m) =>
        `<button class="seg-btn ico-row${m.id === cur.mode ? ' active' : ''}" data-mode="${m.id}"
           title="${m.name}">${ico(m.icon)} ${m.name}</button>`
      ).join('');
      modes.addEventListener('click', (e) => {
        const b = e.target.closest('.seg-btn');
        if (b) set({ mode: b.dataset.mode });
      });
    }

    const accents = document.getElementById('theme-accents');
    if (accents && !accents.dataset.built) {
      accents.dataset.built = '1';
      accents.innerHTML = ACCENTS.map((a) =>
        `<button class="accent-dot${a.id === cur.accent ? ' active' : ''}"
           data-accent="${a.id}" title="${a.name}"
           style="--dot:${a.dot}"><i></i><span>${a.name}</span></button>`
      ).join('');
      accents.addEventListener('click', (e) => {
        const b = e.target.closest('.accent-dot');
        if (b) set({ accent: b.dataset.accent });
      });
    }
    syncUI();
  }

  function bind() {
    renderUI();
    document.getElementById('btn-theme-quick')?.addEventListener('click', toggle);
    document.getElementById('btn-theme-toggle')?.addEventListener('click', toggle);

    // 跟随系统时，系统换配色要立刻跟上
    try {
      const mq = window.matchMedia && window.matchMedia('(prefers-color-scheme: dark)');
      if (mq && mq.addEventListener) {
        mq.addEventListener('change', () => {
          if (cur.mode === 'system') { apply(); syncUI(); emit(); }
        });
      }
    } catch (e) { /* 忽略 */ }
  }

  /** 界面还没建好时先挂到 DOMContentLoaded 上。 */
  if (document.readyState === 'loading') {
    document.addEventListener('DOMContentLoaded', bind);
  } else {
    bind();
  }

  return {
    MODES, ACCENTS,
    get mode() { return cur.mode; },
    get accent() { return cur.accent; },
    get resolved() { return resolvedMode(cur.mode); },
    set, toggle, apply, renderUI, syncUI,
    /** 订阅主题变化，返回取消函数。 */
    onChange(fn) {
      listeners.push(fn);
      return () => { listeners = listeners.filter((x) => x !== fn); };
    },
  };
})();

window.Theme = Theme;
