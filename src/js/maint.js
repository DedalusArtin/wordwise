/* ============================================================
   maint.js —— 设置页的运维面板

   1. **数据库**：把「数据存在哪、多大、多少行」摆到明面上，并提供
      整理 / 检查 / 备份三个动作。应用一直是 SQLite，但用户看不见它，
      出问题时只能靠猜；这一层就是把黑盒打开。

   2. **数据与模型的存放位置**：回答「我装在 D 盘，模型为什么写 C 盘」。
      装到哪数据就落哪；每个盘剩多少、各占多少、怎么搬走、怎么退回默认。

   3. **本地大模型一键部署**：没有 LM Studio 的用户也能用 AI 讲解。
      三步：装引擎 → 下模型 → 起服务。每步都有进度，不让用户对着
      一个转了几分钟的圈干等。

   这些面板共用一套「按钮点了要置灰、失败要把错误说出来」的写法，
   所以下面抽了一个 `run()` 做统一处理。
   ============================================================ */

const Maint = (() => {
  const { API } = window.WordWiseAPI;
  const U = () => window.WW;
  const $ = (id) => document.getElementById(id);

  function human(n) {
    const f = Number(n) || 0;
    if (f >= 1024 * 1024) return (f / 1024 / 1024).toFixed(1) + ' MB';
    if (f >= 1024) return (f / 1024).toFixed(0) + ' KB';
    return f + ' B';
  }

  /**
   * 统一按钮态：点了置灰 + 改文案，结束恢复。
   * 没这个的话，网络慢时用户会连点好几次，后端就会重复下载。
   */
  async function run(btn, busyText, fn) {
    if (!btn) return fn();
    const old = btn.textContent;
    btn.disabled = true;
    btn.textContent = busyText;
    try { return await fn(); }
    finally { btn.disabled = false; btn.textContent = old; }
  }

  /* ============================================================
     一、数据库
     ============================================================ */

  const Db = (() => {
    let info = null;

    function render() {
      const box = $('db-info');
      if (!box) return;
      if (!info) {
        box.innerHTML = '<span class="muted">还没读到数据库信息。</span>';
        return;
      }
      const rows = (info.tables || []).map(t => `
        <div class="db-trow">
          <span class="db-tname">${U().esc(t.label)}</span>
          <span class="db-tcount">${t.rows}</span>
        </div>`).join('');

      box.innerHTML = `
        <div class="db-head">
          <div class="db-path" title="${U().esc(info.path)}">
            <span class="db-label">文件</span>
            <code>${U().esc(info.path)}</code>
          </div>
          <div class="db-metrics">
            <span>体积 <b>${U().esc(info.size_text)}</b></span>
            <span>共 <b>${info.total_rows}</b> 行</span>
            <span>表 <b>${(info.tables || []).length}</b> 张</span>
            ${info.wal_bytes ? `<span class="muted">WAL ${U().esc(info.wal_text)}</span>` : ''}
          </div>
        </div>
        <div class="db-tables">${rows}</div>
        <div class="muted" style="margin-top:8px">
          备份与导出目录：<code>${U().esc(info.export_dir)}</code>
        </div>`;
    }

    async function load() {
      const box = $('db-info');
      if (!box) return;
      try {
        info = await API.dbInfo();
      } catch (e) {
        info = null;
        box.innerHTML = `<span class="muted">读取失败：${U().esc(e.message)}</span>`;
        return;
      }
      render();
    }

    async function act(action, btn, busyText) {
      const out = $('db-result');
      await run(btn, busyText, async () => {
        try {
          const r = await API.dbMaintain(action);
          if (out) {
            out.classList.remove('hidden');
            // 备份成功要给出路径，否则用户不知道文件去哪了
            out.innerHTML = `
              <div class="db-res ${r.ok ? 'ok' : 'bad'}">
                <b>${U().esc(r.message)}</b>
                ${action === 'vacuum' && r.ok
                  ? `<span class="muted">${U().esc(r.before_text)} → ${U().esc(r.after_text)}</span>` : ''}
                ${r.path ? `<div class="db-res-path"><code>${U().esc(r.path)}</code>
                  <button class="ghost-btn xs" id="db-res-open">打开</button></div>` : ''}
              </div>`;
            const open = $('db-res-open');
            if (open) {
              open.addEventListener('click', async () => {
                try { await API.openDir(r.path); } catch (e) { U().toast(e.message, 'err'); }
              });
            }
          }
          // 整理会改变体积，检查完也刷新一下心里有底
          await load();
        } catch (e) {
          if (out) {
            out.classList.remove('hidden');
            out.innerHTML = `<div class="db-res bad"><b>${U().esc(e.message)}</b></div>`;
          }
        }
      });
    }

    function bind() {
      $('btn-db-check')?.addEventListener('click', (e) => act('check', e.currentTarget, '检查中…'));
      $('btn-db-vacuum')?.addEventListener('click', (e) => act('vacuum', e.currentTarget, '整理中…'));
      $('btn-db-backup')?.addEventListener('click', (e) => act('backup', e.currentTarget, '备份中…'));
      $('btn-db-open')?.addEventListener('click', async (e) => {
        await run(e.currentTarget, '打开中…', async () => {
          try {
            if (!info) info = await API.dbInfo();
            await API.openDir(info.data_dir);
          } catch (err) { U().toast(err.message, 'err'); }
        });
      });

      // 清理 AI 讲解存档。
      //
      // 默认**保留已并入词库的**那些：它们对应的词条已经在 `words` 表里，
      // 删掉存档会让「讲解来自哪里」这条线索断掉。要删干净得先去词库删词条。
      $('btn-explain-clear')?.addEventListener('click', async (e) => {
        await run(e.currentTarget, '清理中…', async () => {
          const out = $('db-result');
          try {
            const n = await API.clearExplains(true);
            if (out) {
              out.classList.remove('hidden');
              out.innerHTML = `<div class="db-res ok"><b>已清理 ${n} 条未并入词库的讲解存档</b>
                <span class="muted">（已并入词库的讲解会保留）</span></div>`;
            }
            await load();
          } catch (err) {
            if (out) {
              out.classList.remove('hidden');
              out.innerHTML = `<div class="db-res bad"><b>${U().esc(err.message)}</b></div>`;
            }
          }
        });
      });
    }

    return { load, bind };
  })();

  /* ============================================================
     二、数据与模型的存放位置
     ============================================================

     这一块回答的是用户最常问、而以前**答不上来**的一个问题：
     「我明明把软件装在 D 盘，模型为什么跑到 C 盘去了？」

     所以界面不只给出路径，还要给出三件事：
       - **为什么是这个目录**（跟着软件走 / 老数据沿用 / 环境变量强制）；
       - **每个盘还剩多少**（不然用户没法判断该搬去哪）；
       - **删软件目录会连数据一起删**（「跟着软件走」的代价，必须说清楚）。
     ============================================================ */

  const Storage = (() => {
    let info = null;
    /** 当前在编辑哪个目录：`'data'` | `'models'` */
    let kind = 'data';

    function tag(text, cls) {
      return `<span class="st-tag ${cls || ''}">${U().esc(text)}</span>`;
    }

    function row(label, path, opts) {
      const o = opts || {};
      return `<div class="st-row">
        <div class="st-k">${U().esc(label)}</div>
        <div class="st-v">
          <code class="st-path" title="${U().esc(path)}">${U().esc(path)}</code>
          ${o.tags || ''}
        </div>
        <div class="st-size">${U().esc(o.size || '')}</div>
        ${o.action || '<span></span>'}
      </div>`;
    }

    function render() {
      const box = $('storage-info');
      if (!box) return;
      if (!info) {
        box.innerHTML = '<span class="muted">还没读到存放位置。</span>';
        return;
      }

      const rows = [];

      // ---- 学习数据（词库 / 进度 / 缓存 / 备份，都是它下面的东西）----
      const dataTags = [tag(info.source_label, info.travels_with_app ? 'warn' : '')];
      rows.push(row('学习数据', info.data_dir, {
        tags: dataTags.join(' '),
        size: `共 ${info.data_text}`,
        action: '<button class="ghost-btn xs" data-st-act="edit-data">更换目录</button>',
      }));

      // ---- 数据库 ----
      const wal = info.db_wal_text && info.db_wal_text !== '0 B'
        ? `（另有写入日志 ${info.db_wal_text}）` : '';
      rows.push(row('  └ 数据库', info.db_path, { size: info.db_text + wal }));

      // ---- 模型目录 ----
      const mTags = [info.models_custom ? tag('已单独指定', 'warn') : tag('在数据目录内')];
      const mCount = (info.models_installed || []).length;
      const mTotal = mCount + ((info.models_missing || []).length);
      rows.push(row('模型文件', info.models_dir, {
        tags: mTags.join(' '),
        size: `${info.models_text}（已下载 ${mCount}/${mTotal} 档）`,
        action: '<button class="ghost-btn xs" data-st-act="edit-models">更换目录</button>',
      }));

      // ---- 推理引擎 ----
      rows.push(row('推理引擎', info.engine_ready ? info.engine_dir : '未安装', {
        tags: info.engine_ready
          ? tag(info.engine_from_bundle ? '随安装包附带' : '下载到数据目录')
          : tag('还没装', 'warn'),
        size: info.engine_ready ? info.engine_text : '',
      }));

      // ---- 警告与说明 ----
      const notes = [];
      if (info.from_env) {
        notes.push(`<div class="st-note warn">
          当前数据目录由环境变量 <code>WORDWISE_DATA_DIR</code> 指定（<code>${U().esc(info.env_data_dir)}</code>），
          优先级最高，下面的按钮改了也不会生效。想用界面管理，请先去掉这个环境变量。
        </div>`);
      }
      if (info.models_env) {
        notes.push(`<div class="st-note warn">
          模型目录由环境变量 <code>WORDWISE_MODELS_DIR</code> 指定（<code>${U().esc(info.models_env)}</code>），
          优先级最高。
        </div>`);
      }
      if (info.portable) {
        notes.push(`<div class="st-note">
          检测到便携模式（软件目录里有 <code>portable.txt</code>）：
          数据全部放在程序目录内，整个文件夹拷到 U 盘就能带着走。
        </div>`);
      }
      if (info.travels_with_app) {
        notes.push(`<div class="st-note warn">
          <b>注意</b>：数据跟着软件目录走，所以<b>删掉软件目录会连词库和进度一起删掉</b>。
          要重装或卸载前，先到上面「数据库」里备份一份，或者把数据目录换到别处。
        </div>`);
      }

      // ---- 磁盘一览（选了盘才知道搬得动）----
      const drives = (info.drives || []).map((d) => `
        <div class="st-drive ${d.system ? 'sys' : ''}">
          <b>${U().esc(d.root)}</b>
          <span>${U().esc(d.kind_label)}</span>
          <span class="st-drive-free">剩余 ${U().esc(d.free_text)}</span>
          ${d.system ? tag('系统盘', 'warn') : ''}
        </div>`).join('');

      box.innerHTML = `
        <div class="st-rows">${rows.join('')}</div>
        <div class="st-total">合计占用（数据 + 模型 + 引擎）：<b>${U().esc(info.total_text)}</b></div>
        ${notes.join('')}
        ${drives ? `<div class="st-drives"><div class="st-drives-h">本机磁盘</div>${drives}</div>` : ''}`;
    }

    /* ---- 换目录表单 ---- */

    function renderDriveChips() {
      const box = $('st-edit-drives');
      if (!box) return;
      const suffix = kind === 'data' ? 'WordWiseData' : 'WordWiseModels';
      const list = (info && info.drives) || [];
      if (!list.length) {
        box.innerHTML = '';
        return;
      }
      // 系统盘排最后：用户要的就是「别放这里」
      const sorted = list.slice().sort((a, b) => (a.system ? 1 : 0) - (b.system ? 1 : 0));
      box.innerHTML = `<span class="st-chips-h">常用位置：</span>` + sorted.map((d) => `
        <button class="st-chip ${d.system ? 'sys' : ''}" data-st-drive="${U().esc(d.root)}"
                title="剩余 ${U().esc(d.free_text)}">${U().esc(d.root)} ${U().esc(suffix)}</button>`).join('');
    }

    function openEdit(which) {
      kind = which === 'models' ? 'models' : 'data';
      const form = $('storage-edit');
      if (!form) return;
      const isData = kind === 'data';
      const title = $('st-edit-title');
      if (title) title.textContent = isData ? '更换数据目录' : '更换模型目录';

      const lab = $('st-edit-migrate-label');
      if (lab) {
        lab.textContent = isData
          ? '把现有数据一起搬过去（推荐；不勾的话新目录是空的）'
          : '把已下载的模型一起搬过去（推荐；不勾的话要重新下载）';
      }
      const path = $('st-edit-path');
      if (path) {
        path.value = '';
        path.placeholder = isData ? '例如 D:\\WordWiseData' : '例如 D:\\WordWiseModels';
      }
      const mig = $('st-edit-migrate');
      if (mig) mig.checked = true;

      const note = $('st-edit-note');
      if (note) {
        note.innerHTML = isData
          ? '原目录里的文件<b>不会被删除</b>，确认新目录正常后你可以自己清理。'
          : '模型目录改完<b>立刻生效</b>（不用重启）；但如果模型正被本地服务加载着，先停一下再搬。';
      }

      renderDriveChips();
      form.classList.remove('hidden');
      const res = $('storage-result');
      if (res) res.classList.add('hidden');
      render();
    }

    function closeEdit() {
      $('storage-edit')?.classList.add('hidden');
    }

    /** 点磁盘标签 → 直接把路径填进输入框 */
    function chooseDrive(root) {
      const suffix = kind === 'data' ? 'WordWiseData' : 'WordWiseModels';
      const path = $('st-edit-path');
      if (path) path.value = `${root}${suffix}`;
      return path ? path.value : '';
    }

    async function openPath(p) {
      if (!p) return;
      try { await API.openDir(p); } catch (e) { U().toast(e.message, 'err'); }
    }

    function showResult(html, ok) {
      const box = $('storage-result');
      if (!box) return;
      box.classList.remove('hidden');
      box.innerHTML = `<div class="db-res ${ok ? 'ok' : 'bad'}">${html}</div>`;
    }

    async function save(btn) {
      const p = (($('st-edit-path') || {}).value || '').trim();
      if (!p) {
        U().toast('先填一个目录，或者点下面的磁盘标签', 'err');
        return null;
      }
      const migrate = !!($('st-edit-migrate') || {}).checked;
      return run(btn, '正在迁移…', async () => {
        try {
          const r = kind === 'data'
            ? await API.setDataDir(p, migrate)
            : await API.setModelsDir(p, migrate);
          if (!r) { await load(); return null; }
          const warn = r.restart_required
            ? `<div class="st-restart">
                 <span>改数据目录必须重启才能换库。</span>
                 <button class="primary-btn xs" id="st-btn-restart">立即重启</button>
                 <button class="ghost-btn xs" id="btn-storage-later">稍后自己重启</button>
               </div>`
            : '';
          showResult(`<b>${U().esc(r.path || '已恢复默认')}</b><div class="st-msg">${
            U().esc(r.message).replace(/\n/g, '<br>')}</div>${warn}`, true);
          closeEdit();
          await load();
          return r;
        } catch (err) {
          showResult(`<b>没能改成功</b><div class="st-msg">${
            U().esc(err.message).replace(/\n/g, '<br>')}</div>`, false);
          return null;
        }
      });
    }

    /** 恢复默认：往指针文件里写 `default`，不删任何文件 */
    async function resetDefault(btn) {
      return run(btn, '处理中…', async () => {
        try {
          const r = kind === 'data'
            ? await API.setDataDir('', true)
            : await API.setModelsDir('', true);
          showResult(`<b>已恢复默认</b><div class="st-msg">${
            U().esc((r && r.message) || '').replace(/\n/g, '<br>')}</div>`, true);
          closeEdit();
          await load();
          return r;
        } catch (err) {
          showResult(`<b>没能改成功</b><div class="st-msg">${U().esc(err.message)}</div>`, false);
          return null;
        }
      });
    }

    async function restart() {
      try {
        await API.restartApp();
      } catch (e) {
        U().toast('重启失败，请手动关掉再打开：' + e.message, 'err');
      }
    }

    async function load() {
      const box = $('storage-info');
      if (!box) return;
      try {
        info = await API.storageInfo();
      } catch (e) {
        info = null;
        box.innerHTML = `<span class="muted">读取失败：${U().esc(e.message)}</span>`;
        return;
      }
      render();
    }

    let bound = false;
    function bind() {
      if (bound) return;
      bound = true;

      // 行内按钮是 innerHTML 现生成的，只能用事件委托
      $('storage-info')?.addEventListener('click', (e) => {
        const t = e.target;
        const b = t && t.closest ? t.closest('[data-st-act]') : null;
        if (!b) return;
        const act = b.getAttribute('data-st-act');
        if (act === 'edit-data') openEdit('data');
        else if (act === 'edit-models') openEdit('models');
      });

      // 磁盘标签同理
      $('st-edit-drives')?.addEventListener('click', (e) => {
        const t = e.target;
        const b = t && t.closest ? t.closest('[data-st-drive]') : null;
        if (!b) return;
        chooseDrive(b.getAttribute('data-st-drive') || '');
      });

      // 结果区里的「立即重启」也是生成的
      $('storage-result')?.addEventListener('click', (e) => {
        const t = e.target;
        if (t && t.id === 'st-btn-restart') restart();
        else if (t && t.id === 'btn-storage-later') {
          $('storage-result')?.classList.add('hidden');
        }
      });

      $('btn-storage-refresh')?.addEventListener('click', (e) => run(e.currentTarget, '刷新中…', load));
      $('btn-storage-open-data')?.addEventListener('click', () => openPath(info && info.data_dir));
      $('btn-storage-open-models')?.addEventListener('click', () => openPath(info && info.models_dir));
      $('btn-storage-cancel')?.addEventListener('click', closeEdit);
      $('btn-storage-save')?.addEventListener('click', (e) => save(e.currentTarget));
      $('btn-storage-default')?.addEventListener('click', (e) => resetDefault(e.currentTarget));
    }

    return {
      load, bind, render,
      // 下面这些是给冒烟测试用的：沙箱里的假 DOM 解析不了 innerHTML 生成的按钮，
      // 只能直接驱动动作来验证「点下去真的调了后端」。
      openEdit, closeEdit, chooseDrive, save, resetDefault, restart,
      get info() { return info; },
      get kind() { return kind; },
    };
  })();

  /* ============================================================
     三、本地大模型一键部署
     ============================================================ */

  const Llm = (() => {
    let status = null;
    let models = [];

    function renderStatus() {
      const box = $('llm-local-status');
      if (!box) return;
      if (!status) {
        box.innerHTML = '<span class="muted">—</span>';
        return;
      }
      const e = status.engine || {};
      const s = status.server || {};
      const dot = s.running ? (s.using_ours ? 'ok' : 'warn') : 'off';
      const text = s.running
        ? (s.using_ours ? `已启动（端口 ${s.port}）` : '已启动，但 AI 当前指向的并非本服务')
        : '未启动';

      box.innerHTML = `
        <div class="llm-line">
          <span class="llm-dot ${dot}"></span>
          <b>${U().esc(text)}</b>
          ${s.running
            ? `<button class="ghost-btn xs" id="btn-llm-stop">停止</button>`
            : `<button class="primary-btn xs" id="btn-llm-start">启动</button>`}
        </div>
        <div class="llm-meta">
          <span>引擎 ${e.ready ? '已就绪' : '<b class="warn">未安装</b>'}</span>
          <span>已装模型 <b>${((status.models || {}).installed || []).length}</b> 个</span>
          <span>线程 <b>${status.threads || 0}</b> / ${status.cpu || 0} 核</span>
        </div>
        ${e.ready ? '' : `<div class="llm-hint">需要先装引擎${
          status.engine_needs_download ? '（安装包未内置，将从镜像下载，约 33MB）' : '（安装包已内置，点一下即可）'
        }。</div>`}`;

      $('btn-llm-start')?.addEventListener('click', (ev) => start(ev.currentTarget));
      $('btn-llm-stop')?.addEventListener('click', (ev) => stop(ev.currentTarget));
    }

    function renderModels() {
      const box = $('llm-model-list');
      if (!box) return;
      const installed = new Set(((status && status.models) || {}).installed || []);
      if (!models.length) {
        box.innerHTML = '<span class="muted">—</span>';
        return;
      }
      box.innerHTML = models.map(m => {
        const has = installed.has(m.id);
        return `<div class="llm-model ${has ? 'has' : ''}">
          <div class="lm-head">
            <b>${U().esc(m.name)}</b>
            ${m.recommended ? '<span class="tag blue">推荐</span>' : ''}
            <span class="lm-size">${human(m.size_bytes)}</span>
          </div>
          <div class="lm-note">${U().esc(m.note)}</div>
          <div class="lm-actions">
            ${has
              ? `<button class="ghost-btn xs" data-act="start" data-id="${U().esc(m.id)}">启动</button>
                 <button class="ghost-btn xs" data-act="del" data-id="${U().esc(m.id)}">删除</button>`
              : `<button class="primary-btn xs" data-act="dl" data-id="${U().esc(m.id)}">下载</button>`}
          </div>
        </div>`;
      }).join('');

      box.querySelectorAll('[data-act]').forEach(b => {
        b.addEventListener('click', () => {
          const id = b.dataset.id;
          if (b.dataset.act === 'dl') download(b, id);
          else if (b.dataset.act === 'start') start(b, id);
          else if (b.dataset.act === 'del') remove(b, id);
        });
      });
    }

    /* ---------------- 进度条 ---------------- */

    function showProgress(p) {
      const wrap = $('llm-progress');
      const fill = $('llm-progress-fill');
      const text = $('llm-progress-text');
      if (!wrap) return;
      wrap.classList.remove('hidden');
      if (fill) {
        // percent < 0 表示总长未知：给个来回动的「不确定」状态，
        // 别停在 0% 让人以为卡住了
        fill.style.width = p.percent >= 0 ? `${Math.min(100, p.percent).toFixed(1)}%` : '35%';
        fill.classList.toggle('unknown', p.percent < 0);
      }
      if (text) {
        const done = p.stage === 'done' || p.stage === 'failed';
        const size = p.total
          ? `${human(p.got)} / ${human(p.total)}`
          : human(p.got);
        text.innerHTML = `
          <span class="${p.stage === 'failed' ? 'warn' : ''}">${U().esc(p.message || p.stage)}</span>
          ${p.total ? `<span class="muted">${size}${p.speed ? ' · ' + U().esc(p.speed) : ''}</span>` : ''}
          ${done ? '' : `<button class="ghost-btn xs" id="btn-llm-cancel">取消</button>`}`;
        const c = $('btn-llm-cancel');
        if (c) c.addEventListener('click', async () => {
          try { await API.localLlmCancel(); } catch (e) {}
        });
      }
    }

    function hideProgress() {
      $('llm-progress')?.classList.add('hidden');
    }

    let unlisten = null;
    async function bindProgress() {
      if (unlisten) return;
      try {
        unlisten = await API.onLocalLlmProgress((p) => {
          if (!p) return;
          showProgress(p);
          if (p.stage === 'done' || p.stage === 'failed') {
            setTimeout(() => { hideProgress(); refresh(); }, 1600);
          }
        });
      } catch (e) { /* 事件订阅失败不影响手动刷新 */ }
    }

    /* ---------------- 动作 ---------------- */

    async function refresh() {
      try { status = await API.localLlmStatus(); } catch (e) { status = null; }
      try { models = await API.localLlmModels(); } catch (e) { models = []; }
      renderStatus();
      renderModels();
    }

    async function installEngine(btn) {
      await run(btn, '安装中…', async () => {
        try {
          const r = await API.localLlmInstallEngine();
          U().toast(r && r.message ? r.message : '引擎已就绪', 'ok');
        } catch (e) { U().toast(e.message, 'err'); }
        await refresh();
      });
    }

    async function download(btn, id) {
      await run(btn, '下载中…', async () => {
        showProgress({ kind: 'model', stage: 'download', got: 0, total: 0, percent: -1, speed: '', message: '准备下载…' });
        try {
          await API.localLlmInstallModel(id);
        } catch (e) { U().toast(e.message, 'err'); }
        await refresh();
      });
    }

    async function start(btn, id) {
      await run(btn, '启动中…', async () => {
        try {
          // 引擎没装就先装，别让用户自己猜为什么起不来
          if (status && status.engine && !status.engine.ready) {
            await API.localLlmInstallEngine();
          }
          const r = await API.localLlmStart(id || null);
          U().toast((r && r.message) || '已启动本地服务', 'ok');
          // 起完后顺手把 AI 指到本服务，省得用户再去填地址
          try { await API.localLlmProbe(); } catch (e) { /* 探测失败不阻断 */ }
        } catch (e) { U().toast(e.message, 'err'); }
        await refresh();
      });
    }

    async function stop(btn) {
      await run(btn, '停止中…', async () => {
        try {
          const r = await API.localLlmStop();
          U().toast((r && r.message) || '已停止本地服务', 'ok');
        }
        catch (e) { U().toast(e.message, 'err'); }
        await refresh();
      });
    }

    async function remove(btn, id) {
      if (!window.confirm('删除这个已下载的模型文件？下次用需要重新下载。')) return;
      await run(btn, '删除中…', async () => {
        try { await API.localLlmRemoveModel(id); U().toast('已删除', 'ok'); }
        catch (e) { U().toast(e.message, 'err'); }
        await refresh();
      });
    }

    async function probe(btn) {
      await run(btn, '检测中…', async () => {
        try {
          const r = await API.localLlmProbe();
          U().toast((r && r.message) || JSON.stringify(r), 'ok');
        } catch (e) { U().toast(e.message, 'err'); }
        await refresh();
      });
    }

    async function openModels(btn) {
      await run(btn, '打开中…', async () => {
        try {
          if (!status) status = await API.localLlmStatus();
          await API.openDir((status.models || {}).dir || (status.data_dir || ''));
        } catch (e) { U().toast(e.message, 'err'); }
      });
    }

    function bind() {
      $('btn-llm-probe')?.addEventListener('click', (e) => probe(e.currentTarget));
      $('btn-llm-open-models')?.addEventListener('click', (e) => openModels(e.currentTarget));
      $('btn-llm-install-engine')?.addEventListener('click', (e) => installEngine(e.currentTarget));
      // 设置页里的开关与左下角弹层里的开关是同一份配置，改哪边都要同步，
      // 否则两处显示不一致，用户会以为没保存上
      $('set-llm-auto-start')?.addEventListener('change', async (e) => {
        const on = e.target.checked;
        try {
          const r = await API.setLocalLlmAuto(on);
          U().toast((r && r.message) || '已更新', 'ok');
        } catch (err) { U().toast(err.message, 'err'); }
        const pop = $('llm-pop-auto');
        if (pop) pop.checked = on;
      });
      bindProgress();
    }

    return { load: refresh, bind };
  })();

  /* ============================================================
     四、左下角状态条弹层
     ============================================================

     启动/停止原本只藏在设置页里，用户想用 AI 得先翻两屏。这里把同一组
     动作搬到常驻的状态条上：点一下就有「启动 / 停止」。

     状态条本身只显示一句话，弹层才负责动作 —— 两者读的是同一份 status，
     不会出现「状态条说已就绪、弹层说没启动」这种自相矛盾。
     ============================================================ */

  const Chip = (() => {
    let open = false;
    /** 最近一次读到的状态。启动时要据此决定「要不要先装引擎」。 */
    let last = null;

    function setOpen(v) {
      open = !!v;
      $('llm-pop')?.classList.toggle('hidden', !open);
      if (open) refresh();
    }

    /** 引擎/模型缺什么就说什么，别让用户对着一个灰按钮猜。 */
    function render(s) {
      const box = $('llm-pop-status');
      const acts = $('llm-pop-actions');
      const models = $('llm-pop-models');
      const note = $('llm-pop-note');
      const auto = $('llm-pop-auto');
      if (!box || !acts) return;

      if (!s) {
        box.innerHTML = '<span class="muted">读取状态失败</span>';
        acts.innerHTML = '';
        if (models) models.innerHTML = '';
        return;
      }

      const e = s.engine || {};
      const sv = s.server || {};
      const ai = s.ai || {};
      const installed = ((s.models || {}).installed) || [];
      const spec = (s.model_list || []).filter(m => installed.includes(m.id));
      // 后端记录的是「去掉扩展名的文件名」（llama.cpp 的模型名习惯），
      // 而清单里给的是完整文件名。这里做一次归一化，否则「运行中」标记永远不亮。
      //
      // 注意：不能写成 replace(/\.[^.]+$/, '') —— 模型名里本来就带小数点
      // （Qwen3-1.7B-Q4_K_M），那样会把版本号当成扩展名砍掉，
      // 得到 'Qwen3-1'，两边永远对不上。只剥真正认识的权重扩展名。
      const stem = (f) => String(f || '').replace(/\.(gguf|ggml|bin|safetensors)$/i, '');
      const runningModel = sv.running ? stem(sv.model) : null;
      // 归一化后对不上就直接比原值，兼容后端将来换命名习惯
      const sameModel = (a, b) => !!a && (stem(a) === stem(b) || a === b);

      // AI 实际在用哪一类服务。读数拿不到就退回「托管服务在不在跑」，
      // 至少不会把一个正在跑的本机服务说成在线 API。
      const kind = ai.endpoint_kind || (sv.running ? 'managed' : 'local');
      const isCloud = kind === 'cloud';
      const host = String(ai.base_url || '').split('://').pop().split('/')[0] || '';
      const kindLabel = isCloud
        ? (host || '在线 API')
        : (kind === 'managed' ? '本机一键部署' : 'LM Studio / 本机服务');
      // 有在线 API 配置时，唯一的真相是「AI 能不能连上」；
      // 用本机服务时才看进程状态（模型加载中也会先算「已连接」）。
      const live = sv.running || !!ai.online;

      const dot = live ? 'ok' : 'off';
      box.innerHTML = `
        <div class="llm-line">
          <span class="llm-dot ${dot}"></span>
          <b>${live ? '已连接' : '未连接'}</b>
          <span class="llm-kind">${U().esc(kindLabel)}</span>
        </div>
        <div class="llm-meta">
          ${ai.active_model ? `<span>模型 <b>${U().esc(ai.active_model)}</b></span>` : ''}
          ${sv.running ? `<span>端口 <b>${sv.port}</b></span>` : ''}
          <span>引擎 ${e.ready ? '就绪' : '<b class="warn">未安装</b>'}</span>
          <span>本地模型 <b>${installed.length}</b> 个</span>
        </div>`;

      // ---- 主按钮 ----
      //
      // 用在线 API 的人，多半根本不想要本机服务。这时把「启动」摆成主按钮
      // 是误导：一点下去 base_url 就被改成本机地址，AI 从云端切到本地小模型，
      // 用户只会觉得「点了反而变笨了」。所以在线时主按钮给「打开 AI 设置」，
      // 本机启停降级成次要动作。
      const canManageLocal = e.ready && installed.length > 0;
      if (isCloud) {
        acts.innerHTML =
          `<button class="primary-btn sm wide" data-act="settings">打开 AI 设置</button>` +
          (sv.running
            ? `<button class="ghost-btn sm wide danger" data-act="stop">停止本机服务（释放内存）</button>`
            : (canManageLocal
                ? `<button class="ghost-btn sm wide" data-act="start">改用本机模型</button>`
                : ''));
      } else if (!e.ready) {
        acts.innerHTML = `<button class="primary-btn sm wide" data-act="setup">去安装引擎</button>`;
      } else if (!installed.length) {
        acts.innerHTML = `<button class="primary-btn sm wide" data-act="setup">去下载模型</button>`;
      } else if (sv.running) {
        acts.innerHTML = `<button class="ghost-btn sm wide danger" data-act="stop">停止（释放内存）</button>`;
      } else {
        acts.innerHTML = `<button class="primary-btn sm wide" data-act="start">启动</button>`;
      }

      if (models) {
        models.innerHTML = spec.length
          ? spec.map(m => `
            <div class="lp-model">
              <span class="lp-mname">${U().esc(m.name)}</span>
              ${sameModel(runningModel, m.file)
                ? '<span class="tag green">运行中</span>'
                : `<button class="ghost-btn xs" data-act="start-one" data-id="${U().esc(m.id)}">启动</button>`}
            </div>`).join('')
          : '';
      }

      if (note) {
        note.textContent = isCloud
          ? `AI 现在走在线 API（${kindLabel}）。改成用本机模型会切过去，停止后又自动还回来。`
          : (e.ready && installed.length
              ? '服务只在本机 127.0.0.1 上监听，退出 WordWise 会自动停止，不会留在后台。'
              : '引擎随安装包附带；模型需要下载一次（0.6B 约 400MB，1.7B 约 1.1GB）。');
      }

      if (auto) auto.checked = !!s.auto_start;

      acts.querySelectorAll('[data-act]').forEach(b => {
        b.addEventListener('click', () => {
          const act = b.dataset.act;
          if (act === 'settings') { setOpen(false); toSettings(); return; }
          if (act === 'setup') { setOpen(false); toSettings(); return; }
          if (act === 'stop') return doStop(b);
          if (act === 'start') return doStart(b, null);
          if (act === 'start-one') return doStart(b, b.dataset.id);
        });
      });
      models?.querySelectorAll('[data-act]').forEach(b => {
        b.addEventListener('click', () => doStart(b, b.dataset.id));
      });
    }

    async function refresh() {
      let s = null;
      try { s = await API.localLlmStatus(); } catch (e) { /* render(null) 会走失败分支 */ }
      // 顺便把模型清单并进来，省一次往返
      if (s) {
        try { s.model_list = await API.localLlmModels(); } catch (e) { s.model_list = []; }
      }
      // 「本机服务在跑」不等于「AI 正在用它」：用户可能正连着 LM Studio 或在线 API。
      // 两者是不同的判断，弹层必须都拿到，否则文案会自相矛盾。
      if (!s) s = {};
      try { s.ai = await API.llmStatus(); } catch (e) { s.ai = null; }
      last = s;
      render(s);
    }

    function toSettings() {
      window.Pages?.go('settings');
      // 设置页里那一大块才是下载/删除模型的地方，滚过去
      setTimeout(() => {
        ($('llm-model-list') || $('set-llm-preset'))?.scrollIntoView({ behavior: 'smooth', block: 'center' });
      }, 120);
    }

    async function doStart(btn, id) {
      await run(btn, '启动中…', async () => {
        try {
          // 引擎不在就先装。随包分发时这一步只是把 vendor/llama 认下来，
          // 是瞬时的；真的缺文件才会走镜像下载。
          if (!(last && last.engine && last.engine.ready)) {
            await API.localLlmInstallEngine();
          }
          const r = await API.localLlmStart(id || null);
          U().toast((r && r.message) || '本地模型服务已启动', 'ok');
        } catch (e) {
          U().toast(e.message, 'err');
        }
        await refresh();
        window.App?.checkLlm?.();
      });
    }

    async function doStop(btn) {
      await run(btn, '停止中…', async () => {
        try {
          // 用后端返回的文案：服务停掉时如果还把 AI 地址还原了原来的
          // LM Studio，得让用户知道，否则他以为只是「停了个服务」。
          const r = await API.localLlmStop();
          U().toast((r && r.message) || '本地模型服务已停止，内存已释放', 'ok');
        } catch (e) { U().toast(e.message, 'err'); }
        await refresh();
        window.App?.checkLlm?.();
      });
    }

    async function toggleAuto(on) {
      try {
        const r = await API.setLocalLlmAuto(on);
        U().toast((r && r.message) || '已更新', 'ok');
      } catch (e) {
        U().toast(e.message, 'err');
      }
      // 设置页那份开关也要跟着变，否则两个地方显示不一致
      const box = $('set-llm-auto-start');
      if (box) box.checked = on;
    }

    let bound = false;
    function bind() {
      if (bound) return;
      bound = true;
      $('llm-chip')?.addEventListener('click', (e) => {
        // 不 stopPropagation 的话会冒到 document 上，被「点别处就关」那条
        // 监听器立刻关掉 —— 表现是「点了没反应」。
        e.stopPropagation();
        setOpen(!open);
        // 顺手重测一次，状态条上的文字要跟弹层同步
        window.App?.checkLlm?.();
      });
      $('llm-pop-close')?.addEventListener('click', () => setOpen(false));
      $('llm-pop-settings')?.addEventListener('click', () => { setOpen(false); toSettings(); });
      $('llm-pop-auto')?.addEventListener('change', (e) => toggleAuto(e.target.checked));
      $('llm-pop')?.addEventListener('click', (e) => e.stopPropagation());

      // 点别处 / Esc 关掉
      document.addEventListener('click', () => { if (open) setOpen(false); });
      document.addEventListener('keydown', (e) => {
        if (e.key === 'Escape' && open) setOpen(false);
      });
    }

    // start/stop/toggleAuto 也导出：冒烟测试里假 DOM 解析不了 innerHTML 生成的按钮，
    // 只能直接驱动动作来验证「点下去真的调了后端」。
    return {
      bind, refresh, setOpen, start: doStart, stop: doStop, toggleAuto,
      get isOpen() { return open; },
    };
  })();

  let bound = false;
  function bind() {
    if (bound) return;
    bound = true;
    Db.bind();
    Storage.bind();
    Llm.bind();
    Chip.bind();
  }

  /** 进设置页时才拉数据，别在启动时无谓地读盘。 */
  async function load() {
    bind();
    await Db.load();
    await Storage.load();
    await Llm.load();
    const box = $('set-llm-auto-start');
    if (box) {
      try {
        const cfg = await API.getConfig();
        box.checked = !!cfg.auto_start_local_llm;
      } catch (e) { /* 读不到就保持现状 */ }
    }
  }

  return { load, bind, chip: Chip, storage: Storage };
})();

window.Maint = Maint;
