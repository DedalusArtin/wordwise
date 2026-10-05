/* ============================================================
   translate.js —— 独立「翻译」栏目（需求 5）

   对照有道翻译的交互：左右分栏、语言互换、实时翻译、复制、朗读、
   收藏、历史记录，支持句子与段落，并叠加 AI 增强能力。

   几个刻意的设计：
   - **实时翻译带防抖 + 请求序号**。在线翻译接口限频很严（见
     src-tauri/src/translate/mod.rs 的说明），边打字边发请求会立刻撞
     411，所以要等用户停手 600ms 再发；序号则保证「旧请求的返回」
     不会覆盖「新请求的结果」。
   - **读音只作附属标注，单独一行**（需求 2）。选日语时主体必须是
     「こんにちは」，罗马音/假名注音放在下面一行小字里。
   - **语言严格跟随方向选择器**（需求 3）：结果区右上角会写明本次
     用的是哪种语言，译文文字只可能来自那个语言。
   ============================================================ */

const Translate = (() => {
  const { API } = window.WordWiseAPI;
  const U = () => window.WW;

  const MAX_LEN = 5000;
  const DEBOUNCE = 600;

  let timer = null;
  let seq = 0;             // 请求序号：只有最后一次请求能写界面
  let current = null;      // 最近一次翻译结果
  let histFilter = 'all';
  let aiBusy = false;

  const $ = (id) => document.getElementById(id);
  const dir = () => window.DirPicker;

  /* ---------------- 面板与标签同步 ---------------- */

  /** 方向变了：更新两侧语言标签与占位提示，并重译当前内容。 */
  function syncDir(info) {
    const D = dir();
    const from = info ? info.from : D.from;
    const to = info ? info.to : D.to;

    const srcLabel = $('tr-src-label');
    const dstLabel = $('tr-dst-label');
    if (srcLabel) {
      srcLabel.textContent = from === D.AUTO
        ? '原文 · 自动检测'
        : `原文 · ${D.langName(from)}`;
    }
    if (dstLabel) dstLabel.textContent = `译文 · ${D.langName(to)}`;

    const ta = $('tr-src');
    if (ta) {
      ta.placeholder = from === D.AUTO
        ? `输入要翻译的内容，自动识别语种 → ${D.langName(to)}`
        : `输入${D.langName(from)}，翻译成${D.langName(to)}…`;
    }

    // 方向一变，上一次的译文就不再对应当前语言了，必须重译
    const text = ta ? ta.value.trim() : '';
    if (text) run(text, true);
    else clearResult();
  }

  /* ---------------- 翻译 ---------------- */

  function schedule() {
    clearTimeout(timer);
    const text = $('tr-src').value.trim();
    updateCount();
    if (!text) { clearResult(); return; }
    timer = setTimeout(() => run(text, false), DEBOUNCE);
  }

  function updateCount() {
    const el = $('tr-count');
    if (!el) return;
    const n = $('tr-src').value.length;
    el.textContent = `${n} / ${MAX_LEN}`;
    el.classList.toggle('over', n > MAX_LEN);
  }

  async function run(text, force) {
    const D = dir();
    const mySeq = ++seq;

    const box = $('tr-dst');
    if (!box) return;
    if (!current) box.innerHTML = U().loadingHtml('正在翻译…');

    try {
      const res = await API.translate(text, D.from, D.to, !!force);
      if (mySeq !== seq) return;         // 已有更新的请求，丢弃本次结果
      current = res;
      renderResult(res);
      loadHistory();                     // 历史列表同步刷新
    } catch (e) {
      if (mySeq !== seq) return;
      current = null;
      box.innerHTML = `<div class="tr-error">翻译失败：${U().esc(e.message)}</div>`;
      setEngine('失败');
      toggleNote('');
    }
  }

  function setEngine(label, cls) {
    const el = $('tr-engine');
    if (el) {
      el.textContent = label;
      el.classList.toggle('warn', cls === 'warn');
    }
  }

  function toggleNote(msg) {
    const el = $('tr-note');
    if (!el) return;
    if (msg) { el.textContent = msg; el.classList.remove('hidden'); }
    else { el.textContent = ''; el.classList.add('hidden'); }
  }

  /** 渲染一次翻译结果。 */
  function renderResult(res) {
    const D = dir();
    const box = $('tr-dst');
    if (!box) return;

    if (!res || !res.text) {
      box.innerHTML = '<p class="muted">没有拿到译文。</p>';
      setEngine('无结果');
      return;
    }

    // 正文：目标语言的**实际文字**。保留换行，段落翻译能对上原文结构。
    box.innerHTML = U().renderPlainText
      ? U().renderPlainText(res.text)
      : U().esc(res.text).replace(/\n/g, '<br>');

    // 读音：单独一行，附属标注
    const ph = $('tr-phonetic');
    if (ph) {
      if (res.phonetic) {
        ph.textContent = res.phonetic;
        ph.classList.remove('hidden');
      } else {
        ph.textContent = '';
        ph.classList.add('hidden');
      }
    }

    // 其他候选译文
    const alt = $('tr-alt');
    if (alt) {
      const list = (res.alternatives || []).filter(Boolean);
      if (list.length) {
        alt.innerHTML = '<span class="tr-alt-title">其他译法</span>' +
          list.map(t => `<span class="tr-alt-item">${U().esc(t)}</span>`).join('');
        alt.classList.remove('hidden');
      } else {
        alt.innerHTML = '';
        alt.classList.add('hidden');
      }
    }

    // 降级/失败提示（例如「触发频率限制，已改用本地模型翻译」）
    toggleNote(res.note || '');

    // 引擎与语言标注：让「结果语言是否跟着目标语言走」一眼可查
    const engineText = {
      youdao: '在线翻译',
      llm: '本地模型',
      cache: '缓存',
      none: '未翻译',
    }[res.engine] || res.engine;
    const langTag = res.from === D.AUTO ? '' : `${D.langName(res.from)} → `;
    setEngine(`${engineText} · ${langTag}${D.langName(res.to)}`, res.engine === 'llm' ? 'warn' : '');

    // 收藏按钮状态
    refreshFavBtn();
  }

  function clearResult() {
    const box = $('tr-dst');
    if (box) box.innerHTML = '<p class="muted">译文会显示在这里。</p>';
    ['tr-phonetic', 'tr-alt', 'tr-note'].forEach(id => $(id)?.classList.add('hidden'));
    setEngine('待翻译');
    current = null;
    refreshFavBtn();
  }

  function refreshFavBtn() {
    const btn = $('tr-fav');
    if (!btn) return;
    const has = !!(current && current.record_id);
    btn.disabled = !has;
    btn.classList.toggle('on', has && !!current.favorite);
    btn.textContent = has && current.favorite ? '已收藏' : '收藏';
  }

  /* ---------------- 朗读 / 复制 / 收藏 ---------------- */

  function speak() {
    if (!current || !current.text) { U().toast('还没有译文', 'err'); return; }
    // 有道会给一个目标语言的 TTS 地址，音色最贴切；没有就退回浏览器语音
    if (current.tts_url && window.Speak && window.Speak.playUrl) {
      try { window.Speak.playUrl(current.tts_url, 'trans-dst'); return; } catch (e) {}
    }
    if (window.Speak && window.Speak.playTts) {
      window.Speak.playTts(current.text, current.to, 0, 'trans-dst');
    } else {
      U().toast('当前环境不支持朗读', 'err');
    }
  }

  async function copy() {
    if (!current || !current.text) { U().toast('还没有译文', 'err'); return; }
    const text = current.text;
    try {
      if (navigator.clipboard && navigator.clipboard.writeText) {
        await navigator.clipboard.writeText(text);
      } else {
        const ta = document.createElement('textarea');
        ta.value = text;
        document.body.appendChild(ta);
        ta.select();
        document.execCommand('copy');
        ta.remove();
      }
      U().toast('译文已复制', 'ok');
    } catch (e) {
      U().toast('复制失败：' + e.message, 'err');
    }
  }

  async function toggleFavorite() {
    if (!current || !current.record_id) { U().toast('这条翻译还不能收藏', 'err'); return; }
    const next = !current.favorite;
    try {
      await API.translateFavorite(current.record_id, next);
      current.favorite = next;
      refreshFavBtn();
      U().toast(next ? '已加入收藏' : '已取消收藏', 'ok');
      loadHistory();
    } catch (e) {
      U().toast(e.message, 'err');
    }
  }

  /* ---------------- AI 增强 ---------------- */

  async function runAi(action, btn) {
    if (aiBusy) return;
    const text = $('tr-src').value.trim();
    if (!text) { U().toast('请先输入要翻译的内容', 'err'); return; }
    if (!current || !current.text) {
      U().toast('请等翻译完成后再使用 AI 增强', 'err');
      return;
    }

    const D = dir();
    const body = $('tr-ai-body');
    aiBusy = true;
    if (btn) btn.classList.add('busy');
    if (body) body.innerHTML = U().loadingHtml('AI 正在分析…');

    try {
      const md = await API.translateAi(
        action, text, current.text,
        current.from || D.from, current.to || D.to
      );
      if (body) body.innerHTML = U().renderMarkdown(md || '（没有返回内容）');
    } catch (e) {
      if (body) {
        body.innerHTML = `<div class="muted" style="color:#e5484d">AI 增强失败：${U().esc(e.message)}</div>`;
      }
    } finally {
      aiBusy = false;
      if (btn) btn.classList.remove('busy');
    }
  }

  /* ---------------- 历史记录 ---------------- */

  async function loadHistory() {
    const list = $('tr-hist-list');
    if (!list) return;
    try {
      const items = await API.translateHistory(200, histFilter === 'fav');
      const count = $('tr-hist-count');
      if (count) count.textContent = items.length ? `共 ${items.length} 条` : '';

      if (!items.length) {
        list.innerHTML = `<p class="muted">${
          histFilter === 'fav' ? '还没有收藏的翻译。' : '还没有翻译记录。'
        }</p>`;
        return;
      }

      list.innerHTML = items.map(it => `
        <div class="tr-hist-item" data-id="${it.id}">
          <div class="tr-hist-main">
            <div class="tr-hist-src">${U().esc(clip(it.src_text))}</div>
            <div class="tr-hist-dst">${U().esc(clip(it.dst_text))}</div>
          </div>
          <div class="tr-hist-side">
            <span class="tr-hist-dir">${U().esc(dirLabel(it))}</span>
            <button class="icon-btn tr-hist-fav${it.favorite ? ' on' : ''}"
                    data-fav="${it.id}" title="${it.favorite ? '取消收藏' : '收藏'}">&#9733;</button>
            <button class="icon-btn tr-hist-del" data-del="${it.id}" title="删除这条">&#10005;</button>
          </div>
        </div>`).join('');
    } catch (e) {
      list.innerHTML = `<p class="muted">读取历史失败：${U().esc(e.message)}</p>`;
    }
  }

  function dirLabel(it) {
    const D = dir();
    return `${D.langName(it.source_lang)} → ${D.langName(it.target_lang)}`;
  }

  function clip(s) {
    const t = String(s || '').replace(/\s+/g, ' ').trim();
    return t.length > 80 ? t.slice(0, 80) + '…' : t;
  }

  function useHistoryItem(it) {
    const ta = $('tr-src');
    if (ta) ta.value = it.src_text;
    // 方向不同就先切方向，再显示它自己的译文
    const D = dir();
    if (it.source_lang && it.target_lang &&
        (D.from !== it.source_lang || D.to !== it.target_lang)) {
      // 历史里的源语言可能是检测结果，而选择器里没有「检测结果」这一项，
      // 所以只在两个语言都合法时才回填，否则保持当前方向并重新翻译。
      D.set(it.source_lang, it.target_lang);
    } else {
      current = {
        source: it.src_text, text: it.dst_text, alternatives: [],
        from: it.source_lang, to: it.target_lang, phonetic: '',
        tts_url: '', engine: it.engine, favorite: it.favorite,
        record_id: it.id,
      };
      renderResult(current);
    }
    updateCount();
  }

  /* ---------------- 绑定 ---------------- */

  function bind() {
    const ta = $('tr-src');
    ta?.addEventListener('input', schedule);
    ta?.addEventListener('keydown', (e) => {
      // Ctrl/Cmd + Enter 立即翻译，不等防抖
      if ((e.ctrlKey || e.metaKey) && e.key === 'Enter') {
        e.preventDefault();
        const v = ta.value.trim();
        if (v) run(v, false);
      }
    });

    $('tr-clear')?.addEventListener('click', () => {
      if (ta) ta.value = '';
      updateCount();
      clearResult();
      const ai = $('tr-ai-body');
      if (ai) ai.innerHTML = '<p class="muted">先翻译一段内容，再用上面的按钮让 AI 进一步讲解、润色或举例。</p>';
    });
    $('tr-retry')?.addEventListener('click', () => {
      const v = ta?.value.trim();
      if (!v) { U().toast('请先输入内容', 'err'); return; }
      run(v, true);
    });
    $('tr-copy')?.addEventListener('click', copy);
    $('tr-speak')?.addEventListener('click', speak);
    $('tr-fav')?.addEventListener('click', toggleFavorite);

    // AI 增强按钮
    document.querySelectorAll('[data-tr-ai]').forEach(btn => {
      btn.addEventListener('click', () => runAi(btn.dataset.trAi, btn));
    });
    $('tr-ai-clear')?.addEventListener('click', () => {
      const ai = $('tr-ai-body');
      if (ai) ai.innerHTML = '<p class="muted">先翻译一段内容，再用上面的按钮让 AI 进一步讲解、润色或举例。</p>';
    });

    // 上区 tab
    document.querySelectorAll('.tr-tab').forEach(t => {
      t.addEventListener('click', () => {
        const name = t.dataset.trtab;
        document.querySelectorAll('.tr-tab').forEach(x => x.classList.toggle('active', x === t));
        document.querySelectorAll('.tr-tabpane').forEach(p => {
          p.classList.toggle('active', p.id === 'tr-pane-' + name);
        });
        if (name === 'history') loadHistory();
      });
    });

    // 历史筛选
    document.querySelectorAll('[data-histfilter]').forEach(b => {
      b.addEventListener('click', () => {
        histFilter = b.dataset.histfilter;
        document.querySelectorAll('[data-histfilter]').forEach(x => x.classList.toggle('active', x === b));
        loadHistory();
      });
    });

    $('tr-hist-clear')?.addEventListener('click', async () => {
      try {
        const n = await API.translateClear(true);
        U().toast(n ? `已清空 ${n} 条历史（收藏已保留）` : '没有可清空的历史', 'ok');
        loadHistory();
      } catch (e) {
        U().toast(e.message, 'err');
      }
    });

    // 历史列表事件委托：点击回填 / 收藏 / 删除
    $('tr-hist-list')?.addEventListener('click', async (e) => {
      const favId = e.target.closest('[data-fav]');
      if (favId) {
        e.stopPropagation();
        const id = Number(favId.dataset.fav);
        const on = !favId.classList.contains('on');
        try {
          await API.translateFavorite(id, on);
          loadHistory();
        } catch (err) { U().toast(err.message, 'err'); }
        return;
      }
      const del = e.target.closest('[data-del]');
      if (del) {
        e.stopPropagation();
        try {
          await API.translateDelete(Number(del.dataset.del));
          loadHistory();
        } catch (err) { U().toast(err.message, 'err'); }
        return;
      }
      const item = e.target.closest('.tr-hist-item');
      if (item) {
        try {
          const all = await API.translateHistory(200, false);
          const hit = all.find(x => String(x.id) === item.dataset.id);
          if (hit) useHistoryItem(hit);
        } catch (err) { /* 忽略 */ }
      }
    });

    // 方向变化 → 同步标签与占位提示
    dir()?.onChange(syncDir);
    syncDir();
    updateCount();
  }

  /** 页面每次进入时刷新（历史可能被别处改动）。 */
  async function load() {
    await loadHistory();   // 必须 await：否则调用方拿到的还是旧列表
    syncDir();
  }

  // `run` / `renderResult` 对外暴露是为了冒烟测试能直接断言「选日语必须得到
  // こんにちは」这类核心行为，不必靠模拟输入事件绕一圈。
  return { bind, load, run, renderResult, syncDir, get current() { return current; } };
})();

window.Translate = Translate;
