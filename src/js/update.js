/* ============================================================
   update.js —— 「关于与更新」面板 + 版本号填充

   与 maint.js 的分工：maint 管数据库、存放位置、本地模型部署；
   这里只管两件事 ——「我现在是什么版本」和「有没有新版、怎么装上去」。

   ★ 版本号只有一个来源：后端 `cmd_app_info` 的 `version`（= Cargo.toml 的
     `CARGO_PKG_VERSION`）。页面上任何地方都不许写版本字面量，全部节点带
     `data-app-ver` 属性，由 `applyVersion()` 统一填。
     侧边栏窗口与主窗口加载的是同一个 index.html，所以这一段在两个窗口里
     都会执行，不需要各自处理。

   ★ 启动时的静默检查刻意「不弹错误」：更新检查失败对用户毫无价值，
     弹个红框只会让人以为软件坏了。失败原因留在面板里，想查的人点进来就能看到。
   ============================================================ */

const Update = (() => {
  const { API } = window.WordWiseAPI;
  const U = () => window.WW;
  const $ = (id) => document.getElementById(id);

  /** 兜底链接：拿不到 Release 页面地址时用它。 */
  const RELEASES_PAGE = 'https://github.com/DedalusArtin/wordwise/releases';

  let info = null;        // cmd_app_info 的结果（含 version）
  let last = null;        // 最近一次检查结果；出错时是 { error, silent }
  let downloaded = null;  // 已下载好的安装包
  let busy = false;
  let unlisten = null;

  function human(n) {
    const f = Number(n) || 0;
    if (f >= 1024 * 1024) return (f / 1024 / 1024).toFixed(1) + ' MB';
    if (f >= 1024) return (f / 1024).toFixed(0) + ' KB';
    return f + ' B';
  }

  /** 统一显示格式：`v0.41.0`。后端给的是不带 v 的 `0.41.0`。 */
  function fmtVersion(v) {
    const s = String(v == null ? '' : v).trim();
    if (!s) return 'v—';
    return (s[0] === 'v' || s[0] === 'V') ? s : 'v' + s;
  }

  /** 把版本号写进页面上所有 `[data-app-ver]` 节点。 */
  function applyVersion(v) {
    const t = fmtVersion(v);
    document.querySelectorAll('[data-app-ver]').forEach((el) => { el.textContent = t; });
  }

  /** 「有新版本」的两个视觉信号：设置导航项上的小红点 + 版本胶囊变蓝。 */
  function markNav(has) {
    $('nav-update-dot')?.classList.toggle('hidden', !has);
    document.querySelectorAll('.ver-chip[data-app-ver]').forEach((el) => {
      el.classList.toggle('blue', !!has);
    });
  }

  /* ---------------- 检查 ---------------- */

  /**
   * 联网核对版本。
   *
   * @param {boolean} silent 静默模式：不改按钮文案、失败不弹 toast。
   */
  async function check(silent) {
    const btn = $('btn-update-check');
    if (btn && !silent) {
      btn.disabled = true;
      btn.textContent = '检查中…';
    }
    try {
      last = await API.checkUpdate();
      // 版本变了之前的安装包就不作数了，避免装了旧包
      if (downloaded && last && downloaded.version !== last.latest) downloaded = null;
      render();
      if (!silent) {
        U().toast(
          last && last.has_update
            ? `发现新版本 ${fmtVersion(last.latest)}`
            : `已是最新版本 ${fmtVersion((info && info.version) || (last && last.current))}`,
          'ok',
        );
      }
      return last;
    } catch (e) {
      // silent 标记要留在对象里：渲染时静默失败只写一行灰字，不吓人
      last = { error: e.message, silent: !!silent };
      render();
      if (!silent) U().toast('检查更新失败：' + e.message, 'err');
      return null;
    } finally {
      if (btn && !silent) {
        btn.disabled = false;
        btn.textContent = '检查更新';
      }
    }
  }

  /* ---------------- 渲染 ---------------- */

  function render() {
    const box = $('update-status');
    const res = $('update-result');
    if (!box) return;
    if (res) res.innerHTML = '';

    const cur = fmtVersion((info && info.version) || (last && last.current));

    // 还没检查过
    if (!last) {
      box.innerHTML = `<span class="muted">当前版本 <b>${U().esc(cur)}</b>。
        点右上角「检查更新」即可联网核对（也可打开上面的开关，让它在每次启动时静默检查）。</span>`;
      return;
    }

    // 检查失败
    if (last.error) {
      box.innerHTML = `<div class="up-block ${last.silent ? '' : 'err'}">
        <b>${last.silent ? '这次没能连上更新服务器' : '检查更新失败'}</b>
        <p>${U().esc(last.error)}</p>
        <p class="muted">WordWise 的更新包放在 GitHub。如果这台机器访问 GitHub 需要走代理，
          请到上面「网络与代理」里打开代理，再回来点一次「检查更新」。</p>
      </div>`;
      return;
    }

    const meta = `<span class="muted">${U().esc(last.source || '')} · ${
      U().timeAgo ? U().timeAgo(last.checked_at) : ''
    }</span>`;

    // 没有新版本
    if (!last.has_update) {
      const skipped = last.skipped
        ? `<p class="muted">${U().esc(last.note || '')}</p>
           <div class="btn-row"><button class="ghost-btn xs" data-act="unskip"
             data-ver="${U().esc(last.latest)}">取消跳过</button></div>`
        : '';
      box.innerHTML = `<div class="up-block ok">
        <b>已是最新版本</b>
        <p class="muted">当前 ${U().esc(cur)}${last.latest ? ' · 最新 ' + U().esc(fmtVersion(last.latest)) : ''}</p>
        ${skipped}
      </div>
      <div class="up-meta">${meta}</div>`;
      return;
    }

    // 有新版本
    const a = last.asset;
    const assetLine = a
      ? `<div class="up-asset">
           <span class="tag ${a.installable ? 'blue' : 'orange'}">${a.installable ? '安装包' : '便携包'}</span>
           <span class="ua-name">${U().esc(a.name)}</span>
           <span class="muted">${U().esc(a.size_text)}</span>
         </div>`
      : `<div class="up-asset"><span class="muted">这个版本没有找到可自动下载的安装包</span></div>`;

    const canAuto = !!(a && a.installable);
    box.innerHTML = `<div class="up-block new">
      <div class="up-head">
        <b>发现新版本 ${U().esc(fmtVersion(last.latest))}</b>
        ${last.prerelease ? '<span class="tag orange">预发布</span>' : '<span class="tag green">正式版</span>'}
        <span class="muted">${last.published_at ? '发布于 ' + U().esc(last.published_at) : ''}</span>
      </div>
      <p class="muted">当前 ${U().esc(cur)} → 最新 ${U().esc(fmtVersion(last.latest))}${
        last.name ? ' · ' + U().esc(last.name) : ''
      }</p>
      ${assetLine}
      ${last.note ? `<p class="muted">${U().esc(last.note)}</p>` : ''}
      <div class="btn-row">
        ${canAuto
          ? '<button class="primary-btn sm" data-act="dl">下载更新</button>'
          : '<button class="ghost-btn sm" data-act="release">到发布页手动下载</button>'}
        <button class="ghost-btn sm" data-act="skip" data-ver="${U().esc(last.latest)}">跳过此版本</button>
      </div>
      ${last.notes ? `<details class="up-notes"><summary>更新说明</summary>
        <div class="up-md">${U().renderMarkdown(last.notes)}</div></details>` : ''}
    </div>
    <div class="up-meta">${meta}</div>`;

    // 已经下好：把「立即安装」摆到最显眼的位置
    if (res && downloaded) {
      res.innerHTML = `<div class="up-block ok">
        <b>安装包已就绪</b>
        <p>${U().esc((downloaded.path || '').split(/[\\/]/).pop() || '')} · ${U().esc(downloaded.size_text || '')}
          ${downloaded.sha256 ? `· SHA256 ${U().esc(String(downloaded.sha256).slice(0, 8))}…` : ''}</p>
        <div class="btn-row">
          <button class="primary-btn sm" data-act="install">立即安装并退出</button>
          <button class="ghost-btn sm" data-act="open">打开所在文件夹</button>
        </div>
        <p class="muted">点「立即安装」会启动安装程序，WordWise 随后自动退出 —— 先退出是必须的，
          否则安装程序无法覆盖还在运行中的主程序。</p>
      </div>`;
    }
  }

  /* ---------------- 进度条 ---------------- */

  function showProgress(p) {
    const wrap = $('update-progress');
    const fill = $('update-progress-fill');
    const text = $('update-progress-text');
    if (!wrap) return;
    wrap.classList.remove('hidden');
    if (fill) {
      // percent < 0 = 总长未知，给个来回动的状态，别停在 0% 让人以为卡死
      fill.style.width = p.percent >= 0 ? `${Math.min(100, p.percent).toFixed(1)}%` : '35%';
      fill.classList.toggle('unknown', p.percent < 0);
    }
    if (text) {
      const done = p.stage === 'done' || p.stage === 'failed';
      const size = p.total ? `${human(p.got)} / ${human(p.total)}` : human(p.got);
      text.innerHTML = `<span class="${p.stage === 'failed' ? 'warn' : ''}">${U().esc(p.message || p.stage)}</span>
        ${p.total ? `<span class="muted">${size}${p.speed ? ' · ' + U().esc(p.speed) : ''}</span>` : ''}
        ${done ? '' : '<button class="ghost-btn xs" data-act="cancel">取消</button>'}`;
    }
  }

  function hideProgress() {
    $('update-progress')?.classList.add('hidden');
  }

  async function bindProgress() {
    if (unlisten) return;
    try {
      unlisten = await API.onUpdateProgress((p) => {
        if (!p) return;
        showProgress(p);
        if (p.stage === 'done' || p.stage === 'failed') setTimeout(hideProgress, 1600);
      });
    } catch (e) { /* 订阅失败不影响手动检查 */ }
  }

  /* ---------------- 动作 ---------------- */

  async function download(btn) {
    if (busy) return;
    busy = true;
    const old = btn.textContent;
    btn.disabled = true;
    btn.textContent = '下载中…';
    showProgress({ stage: 'download', got: 0, total: 0, percent: -1, speed: '', message: '准备下载…' });
    try {
      const r = await API.downloadUpdate();
      downloaded = r;
      U().toast((r && r.message) || '安装包已下载完成', 'ok');
      render();
    } catch (e) {
      U().toast(e.message, 'err');
      render();
    } finally {
      busy = false;
      btn.disabled = false;
      btn.textContent = old;
      setTimeout(hideProgress, 1200);
    }
  }

  async function install(btn) {
    if (!downloaded) return;
    const ok = window.confirm
      ? window.confirm('将启动安装程序，WordWise 会随后自动退出。\n\n安装完成后请手动重新打开 WordWise。继续吗？')
      : true;
    if (!ok) return;
    btn.disabled = true;
    try {
      const r = await API.runUpdate(downloaded.path);
      U().toast((r && r.message) || '安装程序已启动', 'ok');
    } catch (e) {
      btn.disabled = false;
      U().toast(e.message, 'err');
    }
  }

  /** 加入 / 取消「跳过此版本」。 */
  async function skip(version) {
    try {
      await API.setUpdatePrefs({ skipVersion: version || '' });
      U().toast(version ? `已跳过 ${fmtVersion(version)}` : '已恢复对新版本的提示', 'ok');
      // 重新核对一次，让面板与小红点立刻跟上
      await check(true);
      markNav(!!(last && last.has_update));
    } catch (e) {
      U().toast(e.message, 'err');
    }
  }

  async function openRelease() {
    const url = (last && last.page_url) || RELEASES_PAGE;
    // 更新下载必须走系统浏览器：应用内 WebView 接不住安装包下载，
    // 也拿不到「下载完成后的文件」，所以这里无视「软件内打开」开关
    try { await API.openExternal(url); } catch (e) { U().toast(e.message, 'err'); }
  }

  async function onPanelClick(e) {
    const btn = e.target.closest('[data-act]');
    if (!btn) return;
    const act = btn.dataset.act;
    if (act === 'cancel') {
      try { await API.updateCancel(); } catch (err) { /* 取消失败不影响下载继续 */ }
      return;
    }
    if (act === 'dl') return download(btn);
    if (act === 'skip') return skip(btn.dataset.ver);
    if (act === 'unskip') return skip('');
    if (act === 'install') return install(btn);
    if (act === 'release') return openRelease();
    if (act === 'open') {
      try { await API.openUpdateDir(); } catch (err) { U().toast(err.message, 'err'); }
    }
  }

  /* ---------------- 生命周期 ---------------- */

  /** 进设置页时调用（与 Maint.load() 并列）。 */
  async function load() {
    try { info = await API.appInfo(); } catch (e) { /* 拿不到就沿用上次的 */ }
    applyVersion(info && info.version);

    try {
      const prefs = await API.updatePrefs();
      const chk = $('set-check-update');
      // 默认开：只有后端明确说 false 才关
      if (chk) chk.checked = !prefs || prefs.check_on_start !== false;
    } catch (e) { /* 读不到偏好就用默认显示 */ }

    render();
  }

  /**
   * 启动时的静默检查。
   *
   * 只在主窗口跑：侧边栏窗口是常驻的窄条，让它也发一次请求毫无意义。
   */
  async function checkOnStart() {
    let prefs = null;
    try { prefs = await API.updatePrefs(); } catch (e) { return; }
    if (prefs && prefs.check_on_start === false) {
      markNav(false);
      return;
    }
    const r = await check(true);
    markNav(!!(r && r.has_update));
    if (r && r.has_update) {
      U().toast(`发现新版本 ${fmtVersion(r.latest)}，可在「设置 → 关于与更新」里查看`, 'ok');
    }
  }

  function bind() {
    $('btn-update-check')?.addEventListener('click', () => check(false));
    $('btn-update-release')?.addEventListener('click', openRelease);

    // 开关即时生效：不走「保存设置」，点一下就落盘
    $('set-check-update')?.addEventListener('change', async (e) => {
      try {
        await API.setUpdatePrefs({ checkOnStart: e.target.checked });
        U().toast(e.target.checked ? '下次启动会静默检查更新' : '已关闭启动时检查更新', 'ok');
      } catch (err) {
        e.target.checked = !e.target.checked;
        U().toast(err.message, 'err');
      }
    });

    // 面板里所有按钮（含动态注入的）统一在这里分发
    $('panel-update')?.addEventListener('click', onPanelClick);
    bindProgress();
  }

  return {
    bind, load, check, checkOnStart,
    applyVersion, render, openRelease,
    get info() { return info; },
    get last() { return last; },
    get downloaded() { return downloaded; },
  };
})();

window.Update = Update;
