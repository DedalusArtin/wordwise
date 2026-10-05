/* ============================================================
   dir.js —— 互译方向选择器（需求 1）

   顶部语言控件从「单个语言下拉」升级为明确的
   「源语言 ⇄ 目标语言」方向选择器：

       [自动检测 ▼]  ⇄  [日语 ▼]

   - 源语言可以是「自动检测」，目标语言只能是具体语言；
   - 中间 ⇄ 一键互换（由后端 cmd_swap_direction 落盘）；
   - 查词页与翻译页的**所有**选择器共享同一份配置，
     任意一处改动会同步到其他所有选择器，并广播给订阅方
     （查词页三个面板据此刷新占位提示与请求语言）；
   - 源语言与目标语言撞车时自动纠正目标语言 ——
     否则后端会直接返回「源语言与目标语言相同，没有可翻译的内容」。

   标记约定（HTML 侧只要写这些 data 属性即可，不必写 id）：
       <div data-dir-picker>
         <select data-dir-from></select>
         <button data-dir-swap></button>
         <select data-dir-to></select>
         <span data-dir-label></span>   <!-- 可选：显示「中文 → 日语」 -->
       </div>
   ============================================================ */

const DirPicker = (() => {
  const { API } = window.WordWiseAPI;

  const AUTO = 'auto';

  /** 语言清单：[{code,name,self_name}]，来自后端，保证前后端同一份 */
  let langs = [
    { code: AUTO, name: '自动检测', self_name: '自动检测' },
    { code: 'zh', name: '中文', self_name: '简体中文' },
    { code: 'en', name: '英语', self_name: 'English' },
    { code: 'ja', name: '日语', self_name: '日本語' },
  ];

  let from = AUTO;
  let to = 'en';
  let ready = false;

  /** 订阅者：方向变化时被调用（参数是 {from, to}） */
  const listeners = [];

  function langName(code) {
    if (code === AUTO) return '自动检测';
    const l = langs.find(x => x.code === code);
    return l ? l.name : code;
  }

  /** 生成 <option> 列表。`allowAuto` 只有源语言才给。 */
  function optionsHtml(current, allowAuto) {
    return langs
      .filter(l => allowAuto || l.code !== AUTO)
      .map(l => `<option value="${l.code}"${l.code === current ? ' selected' : ''}>${l.name}</option>`)
      .join('');
  }

  /** 把所有选择器画成当前方向。 */
  function render() {
    document.querySelectorAll('[data-dir-picker]').forEach(box => {
      const f = box.querySelector('[data-dir-from]');
      const t = box.querySelector('[data-dir-to]');
      if (f) { f.innerHTML = optionsHtml(from, true); f.value = from; }
      if (t) { t.innerHTML = optionsHtml(to, false); t.value = to; }
      box.querySelectorAll('[data-dir-label]').forEach(el => {
        el.textContent = `${langName(from)} → ${langName(to)}`;
      });
      // 互换按钮在源语言为「自动检测」时仍可用，但要说明会发生什么
      const sw = box.querySelector('[data-dir-swap]');
      if (sw) {
        sw.title = from === AUTO
          ? `互换方向：把「${langName(to)}」当成源语言`
          : `互换：${langName(from)} → ${langName(to)} 变成 ${langName(to)} → ${langName(from)}`;
      }
    });
  }

  /** 广播给订阅方。 */
  function emit() {
    const info = { from, to };
    listeners.forEach(fn => { try { fn(info); } catch (e) { console.error(e); } });
  }

  /** 把方向写进配置（后端即时落盘），随后广播。 */
  async function persist() {
    try {
      await API.setDirection(from === AUTO ? AUTO : from, to, null);
    } catch (e) {
      // 落盘失败不阻断界面：本次会话仍按新方向工作
      console.warn('保存互译方向失败', e);
    }
  }

  /**
   * 设置方向。
   *
   * `fixTarget` 为真时，若目标语言与源语言相同（且源语言不是自动检测），
   * 自动把目标语言挪开 —— 否则查询/翻译必然无效。
   */
  function set(nextFrom, nextTo, opts = {}) {
    const oldFrom = from, oldTo = to;
    from = nextFrom || AUTO;
    to = nextTo || to || 'en';

    if (opts.fixTarget !== false && from !== AUTO && from === to) {
      to = from === 'zh' ? 'en' : 'zh';
      if (to === from) to = 'en';
    }
    // 目标语言不能是「自动检测」
    if (to === AUTO) to = 'zh';

    render();
    if (from !== oldFrom || to !== oldTo || opts.force) {
      if (opts.persist !== false) persist();
      emit();
    }
  }

  /** 重新拉一遍配置（设置页保存后调用）。 */
  async function refresh() {
    let cfg = (window.App && window.App.config) || null;
    if (!cfg) { try { cfg = await API.getConfig(); } catch (e) { return; } }
    set(cfg.source_lang || AUTO, cfg.target_lang || 'en', { persist: false });
  }

  /** 绑定所有选择器 + 互换按钮。 */
  function bind() {
    document.querySelectorAll('[data-dir-picker]').forEach(box => {
      const f = box.querySelector('[data-dir-from]');
      const t = box.querySelector('[data-dir-to]');
      const sw = box.querySelector('[data-dir-swap]');

      // 用 _bound 打标避免重复绑定（页面可能被多次 bind）
      if (f && !f._dirBound) {
        f._dirBound = true;
        f.addEventListener('change', () => set(f.value, to));
      }
      if (t && !t._dirBound) {
        t._dirBound = true;
        t.addEventListener('change', () => set(from, t.value));
      }
      if (sw && !sw._dirBound) {
        sw._dirBound = true;
        sw.addEventListener('click', swap);
      }
    });
    if (!ready) { ready = true; refresh(); }
    else { render(); }
  }

  /** 一键互换方向。 */
  async function swap() {
    // 优先走后端，保证配置与界面同时更新；失败再本地兜底
    try {
      const r = await API.swapDirection();
      if (Array.isArray(r) && r.length === 2) {
        set(r[0], r[1], { persist: false });   // 后端已经落盘了
        if (window.WW && window.WW.toast) {
          window.WW.toast(`方向已互换：${langName(r[0])} → ${langName(r[1])}`, 'ok');
        }
        return;
      }
    } catch (e) { /* 落到本地兜底 */ }

    if (from === AUTO) {
      const nf = to;
      set(nf, nf === 'zh' ? 'en' : 'zh', { fixTarget: false });
    } else {
      set(to, from, { fixTarget: false });
    }
  }

  /** 订阅方向变化。返回取消订阅函数。 */
  function onChange(fn) {
    listeners.push(fn);
    return () => {
      const i = listeners.indexOf(fn);
      if (i >= 0) listeners.splice(i, 1);
    };
  }

  /** 用后端语言清单覆盖内置兜底清单（保证与后端一致）。 */
  async function loadLangs() {
    try {
      const list = await API.translateLangs();
      if (Array.isArray(list) && list.length) {
        langs = list;
        render();
      }
    } catch (e) { /* 用兜底清单 */ }
  }

  return {
    bind, refresh, set, swap, render, onChange, loadLangs,
    langName,
    get from() { return from; },
    get to() { return to; },
    get current() { return { from, to }; },
    get AUTO() { return AUTO; },
  };
})();

window.DirPicker = DirPicker;
