/* ============================================================
   settings.js —— 设置面板
   记忆辅助开关（需求 3）、本地模型、词典源配置（需求 6）
   ============================================================ */

const Settings = (() => {
  const { API } = window.WordWiseAPI;
  const U = () => window.WW;

  let config = null;
  let sources = [];

  const OPT_KEYS = [
    'show_phonetic', 'show_inflections', 'show_examples', 'show_related',
    'show_mnemonic', 'auto_popup_on_wrong', 'ai_explain',
  ];

  async function load() {
    try {
      config = await API.getConfig();
    } catch (e) {
      U().toast('读取配置失败：' + e.message, 'err');
      return;
    }

    // 记忆辅助开关
    for (const k of OPT_KEYS) {
      const el = document.querySelector(`[data-opt="${k}"]`);
      if (el) el.checked = config.study[k] !== false;
    }
    setVal('set-batch', config.study.batch_size);
    setVal('set-daily', config.study.daily_limit);

    // 本地模型
    setVal('set-llm-url', config.llm.base_url);
    setVal('set-llm-model', config.llm.model);
    setVal('set-llm-temp', config.llm.temperature);
    setVal('set-llm-maxtok', config.llm.max_tokens);

    // 网络与代理
    const net = config.network || {};
    setVal('set-proxy', net.proxy || '');
    setVal('set-net-timeout', net.timeout_secs || 30);
    setVal('set-lookup-timeout', net.lookup_timeout_secs || 8);
    const enableProxy = document.getElementById('set-enable-proxy');
    // 默认直连：只有明确为 true 才算启用代理
    if (enableProxy) enableProxy.checked = net.enable_proxy === true;
    const useSys = document.getElementById('set-use-sys-proxy');
    if (useSys) useSys.checked = net.use_system_proxy !== false;
    syncProxyFields();
    refreshNetStatus();

    // 语言与搜索
    // 只有「查询语言」一个概念了：源语言已改为按书写系统自动识别
    // （见 dict::detect_lang），不再需要一个永远被判定的下拉。
    setVal('set-lang', config.target_lang || 'en');
    setVal('set-search-engine', config.search_engine || 'bing');
    setVal('set-sb-width', config.sidebar_width);
    const top = document.getElementById('set-sb-top');
    if (top) top.checked = config.sidebar_always_on_top !== false;
    const wse = document.getElementById('set-websearch');
    if (wse) wse.checked = config.web_search_enabled !== false;

    // AI 讲解语言（与查词页右上角的下拉是同一份配置，两处改动互相同步）
    const exLang = document.getElementById('set-explain-lang');
    if (exLang) {
      const code = (config.explain_lang || 'zh').toLowerCase();
      exLang.innerHTML = window.WordWiseAPI.explainLangOptions(code);
      exLang.value = code;
    }
    const exAuto = document.getElementById('set-explain-auto');
    if (exAuto) exAuto.checked = config.explain_auto_translate !== false;
    setVal('set-explain-tpl', config.explain_translate_template || '');

    // 词典源
    try {
      sources = await API.getSources();
    } catch (e) {
      sources = config.dict_sources || [];
    }
    renderSources();
  }

  function setVal(id, v) {
    const el = document.getElementById(id);
    if (el) el.value = v === undefined || v === null ? '' : v;
  }
  function getVal(id) {
    const el = document.getElementById(id);
    return el ? el.value.trim() : '';
  }
  function getNum(id, fallback) {
    const n = parseFloat(getVal(id));
    return isNaN(n) ? fallback : n;
  }

  async function save() {
    if (!config) await load();

    // 记忆辅助开关
    for (const k of OPT_KEYS) {
      const el = document.querySelector(`[data-opt="${k}"]`);
      if (el) config.study[k] = el.checked;
    }
    config.study.batch_size = getNum('set-batch', 20);
    config.study.daily_limit = getNum('set-daily', 120);

    config.llm.base_url = getVal('set-llm-url') || 'http://127.0.0.1:1234/v1';
    config.llm.model = getVal('set-llm-model');
    config.llm.temperature = getNum('set-llm-temp', 0.6);
    config.llm.max_tokens = getNum('set-llm-maxtok', 1024);

    config.target_lang = getVal('set-lang') || 'en';
    // source_lang 保留在配置结构里（兼容老配置），但不再是「用户可选项」：
    // 判定词条语言由后端按书写系统完成。
    config.source_lang = '';
    config.search_engine = getVal('set-search-engine') || 'bing';
    config.sidebar_width = getNum('set-sb-width', 380);

    // 网络与代理
    config.network = config.network || {};
    config.network.proxy = getVal('set-proxy');
    config.network.timeout_secs = getNum('set-net-timeout', 30);
    config.network.lookup_timeout_secs = getNum('set-lookup-timeout', 8);
    const enableProxy = document.getElementById('set-enable-proxy');
    config.network.enable_proxy = !!(enableProxy && enableProxy.checked);
    const useSys = document.getElementById('set-use-sys-proxy');
    if (useSys) config.network.use_system_proxy = useSys.checked;

    const top = document.getElementById('set-sb-top');
    if (top) config.sidebar_always_on_top = top.checked;
    const wse = document.getElementById('set-websearch');
    if (wse) config.web_search_enabled = wse.checked;

    // AI 讲解语言：走 setExplainLang 单独落盘，保证「选了就记住」，
    // 哪怕用户最后没点保存按钮。
    const exLang = document.getElementById('set-explain-lang');
    const exCode = exLang ? exLang.value : (config.explain_lang || 'zh');
    config.explain_lang = exCode || 'zh';
    const exAuto = document.getElementById('set-explain-auto');
    if (exAuto) config.explain_auto_translate = exAuto.checked;
    config.explain_translate_template = getVal('set-explain-tpl');

    if (exCode) {
      try { await API.setExplainLang(exCode); } catch (e) { /* 保存主流程里会再写一次 */ }
    }

    try {
      await API.saveConfig(config);
      await API.saveSources(sources);
      U().toast('设置已保存', 'ok');
      // 同步背诵页的题量
      const b = document.getElementById('opt-batch');
      if (b) b.value = config.study.batch_size;
      if (window.App) window.App.refreshConfig();
      // 代理可能变了，刷新一次状态展示
      refreshNetStatus();
    } catch (e) {
      U().toast('保存失败：' + e.message, 'err');
    }
  }

  /* ---------------- 网络与代理 ---------------- */

  /** 「启用代理」关闭时把代理地址等输入项置灰，避免误以为填了就生效。 */
  function syncProxyFields() {
    const on = !!(document.getElementById('set-enable-proxy') || {}).checked;
    const fields = document.getElementById('proxy-fields');
    if (!fields) return;
    fields.classList.toggle('disabled', !on);
    fields.querySelectorAll('input').forEach(el => {
      el.disabled = !on;
    });
  }

  /** 展示当前生效的代理来源。 */
  async function refreshNetStatus() {
    const box = document.getElementById('net-status');
    if (!box) return;
    try {
      const info = await API.networkInfo();
      const using = !!(info && info.url);
      box.className = 'net-status ' + (using ? 'ok' : '');
      box.innerHTML = using
        ? `<b>当前走代理：</b>${U().esc(info.url)} <span class="muted">（来源：${U().esc(info.origin)}）</span>`
        : `<b>当前为直连</b><span class="muted">（${U().esc((info && info.origin) || '未启用代理')}）——
            内置的在线词库、释义与搜索都能直接用，无需代理。仅当检测发现某条链路不通、
            且你确认需要访问被墙资源时，才需要打开上面的「启用代理」。</span>`;
    } catch (e) {
      box.className = 'net-status warn';
      box.textContent = '读取代理状态失败：' + e.message;
    }
  }

  /** 逐站点探测，给出「哪条链路不通」的明确结论。 */
  async function detectNetwork() {
    const box = document.getElementById('net-report');
    if (!box) return;
    box.innerHTML = U().loadingHtml('正在逐个探测…');
    let r;
    try {
      r = await API.networkReport();
    } catch (e) {
      box.innerHTML = `<p class="muted">检测失败：${U().esc(e.message)}</p>`;
      return;
    }
    refreshNetStatus();

    const head = `<p class="muted">${U().esc(r.proxy)}</p>`;
    const rows = (r.items || []).map(it => `
      <div class="net-row ${it.ok ? 'ok' : 'bad'}">
        <span class="net-dot">${it.ok ? '&#10004;' : '&#10008;'}</span>
        <span class="net-name">${U().esc(it.name)}</span>
        <span class="net-detail muted">${U().esc(it.detail)}</span>
      </div>`).join('');

    // 给出可执行的结论，而不是让用户对着红叉发呆。
    // 注意：判据落在「默认直连」必须可用的那几项上。
    const items = r.items || [];
    const find = (kw) => items.find(i => i.name.includes(kw));
    const all = (kw) => items.filter(i => i.name.includes(kw));

    const mirrors = all('jsDelivr');           // 三个独立 CDN 后端
    const mirrorsOk = mirrors.filter(i => i.ok).length;
    const raw = find('GitHub 原文');
    const yd = find('有道');
    const fd = find('freedictionaryapi');
    const bing = find('必应');
    const wiki = find('维基词典');

    let advice = '';
    if (mirrors.length > 0 && mirrorsOk === 0) {
      advice += '<p class="muted">词库下载的三条镜像全部不通，这才需要处理：'
              + '请检查网络；若你所在网络必须走代理，请打开「启用代理」后点「应用并重连」。</p>';
    } else if (mirrorsOk < mirrors.length) {
      advice += `<p class="muted">词库镜像 ${mirrorsOk}/${mirrors.length} 条可用 —— `
              + '足够下载（会自动回退到可用的那条），偶发的超时重试一次即可，无需处理。</p>';
    }
    if (yd && !yd.ok) {
      advice += '<p class="muted">中文释义源（有道）不通，查词的中文结果会缺失，请检查网络。</p>';
    }
    if (fd && !fd.ok) {
      advice += '<p class="muted">英英释义源不通，会只剩中文释义，请检查网络。</p>';
    }
    if (bing && !bing.ok) {
      advice += '<p class="muted">在线搜索（必应 RSS）不通，联网搜索会没有结果。</p>';
    }
    // 只有这些「本来就需要代理」的项失败，才轮到提代理
    if (!advice && raw && !raw.ok && wiki && !wiki.ok) {
      advice += '<p class="muted">GitHub 原文与维基词典不通属正常：它们在国内需要代理，'
              + '且应用用的是 jsDelivr 镜像与其余直连源，不影响正常使用。</p>';
    }
    if (!advice && !mirrorsOk && mirrors.length === 0 && items.length === 0) {
      advice = '<p class="muted">没有探测到任何结果，请稍后重试。</p>';
    }
    if (!advice) {
      advice = '<p class="muted">所有直连链路均正常，无需代理即可使用全部常用功能。</p>';
    }
    box.innerHTML = head + rows + advice;
  }

  /** 保存当前网络配置并立即重建连接。 */
  async function applyNetwork() {
    if (!config) await load();
    config.network = config.network || {};
    config.network.proxy = getVal('set-proxy');
    config.network.timeout_secs = getNum('set-net-timeout', 30);
    config.network.lookup_timeout_secs = getNum('set-lookup-timeout', 8);
    const enableProxy = document.getElementById('set-enable-proxy');
    config.network.enable_proxy = !!(enableProxy && enableProxy.checked);
    const useSys = document.getElementById('set-use-sys-proxy');
    if (useSys) config.network.use_system_proxy = useSys.checked;

    try {
      await API.saveConfig(config);
      const info = await API.reloadNetwork();
      U().toast(info && info.url ? `已切换到代理 ${info.url}` : '已切换为直连', 'ok');
      refreshNetStatus();
    } catch (e) {
      U().toast('应用失败：' + e.message, 'err');
    }
  }

  /* ---------------- 词典源 ---------------- */

  const LANG_OPTIONS = [
    ['en', '英语'], ['ja', '日语'], ['ko', '韩语'], ['fr', '法语'],
    ['de', '德语'], ['es', '西班牙语'], ['ru', '俄语'], ['it', '意大利语'],
    ['pt', '葡萄牙语'], ['ar', '阿拉伯语'], ['hi', '印地语'],
  ];

  // 字段映射的可视化标签
  const MAP_FIELDS = [
    ['word', '词条'],
    ['phonetic_uk', '英式音标'],
    ['phonetic_us', '美式音标'],
    ['audio', '发音音频'],
    ['senses', '义项数组'],
    ['sense_pos', '义项·词性'],
    ['sense_def', '义项·释义'],
    ['sense_examples', '义项·例句'],
    ['example_text', '例句·原文'],
    ['example_translation', '例句·译文'],
    ['inflections', '变形数组'],
    ['inflection_label', '变形·类型'],
    ['inflection_form', '变形·形式'],
    ['related', '相关词'],
    ['mnemonic', '记忆法'],
  ];

  function renderSources() {
    const box = document.getElementById('source-list');
    if (!box) return;

    if (!sources.length) {
      box.innerHTML = '<div class="muted">暂无词典源，点击「恢复内置」加载默认配置。</div>';
      return;
    }

    box.innerHTML = sources.map((s, i) => `
      <div class="source-item" data-idx="${i}">
        <div class="source-head">
          <label class="switch switch-bare" title="启用 / 停用该词典源">
            <input type="checkbox" data-f="enabled" ${s.enabled ? 'checked' : ''} />
          </label>
          <input class="source-name" data-f="name" value="${U().esc(s.name)}" style="flex:1;border:1px solid transparent;background:transparent;font-weight:600;font-size:13.5px" />
          ${s.builtin ? '<span class="tag blue">内置</span>' : '<span class="tag">自定义</span>'}
          ${s.needs_proxy ? '<span class="tag orange" title="该源在国内无法直连，需要先在「网络与代理」里启用代理">需代理</span>' : ''}
          <button class="ghost-btn xs" data-act="test">测试</button>
          ${s.builtin ? '' : '<button class="ghost-btn xs" data-act="del">删除</button>'}
          <span class="source-test-result"></span>
        </div>

        <div style="margin-top:10px;display:grid;gap:8px">
          <label class="field" style="margin:0">
            <span>请求地址模板（{word}=查询词，{lang}=语言代码，{key}=API Key）</span>
            <input data-f="url_template" value="${U().esc(s.url_template)}" />
          </label>

          <div class="inline-2">
            <label class="field" style="margin:0">
              <span>请求方法</span>
              <select data-f="method">
                <option value="GET" ${s.method !== 'POST' ? 'selected' : ''}>GET</option>
                <option value="POST" ${s.method === 'POST' ? 'selected' : ''}>POST (JSON)</option>
              </select>
            </label>
            <label class="field" style="margin:0">
              <span>API Key（可选）</span>
              <input data-f="api_key" value="${U().esc(s.api_key || '')}" placeholder="留空表示无需鉴权" />
            </label>
          </div>

          <label class="field" style="margin:0">
            <span>支持的语言（勾选表示该源可用于这些语言）</span>
            <div style="display:flex;flex-wrap:wrap;gap:8px;padding:4px 0">
              ${LANG_OPTIONS.map(([code, name]) => `
                <label class="check" style="font-size:12px">
                  <input type="checkbox" data-lang="${code}" ${(s.langs || []).includes(code) ? 'checked' : ''} />
                  <span>${name}</span>
                </label>`).join('')}
            </div>
          </label>

          <details>
            <summary style="cursor:pointer;font-size:12.5px;color:#4b5568;padding:4px 0">
              字段映射（把返回的 JSON 映射到词条结构）
            </summary>
            <div class="map-grid" style="margin-top:8px">
              ${MAP_FIELDS.map(([k, label]) => `
                <label>
                  <span>${label}</span>
                  <input data-map="${k}" value="${U().esc((s.mapping || {})[k] || '')}" placeholder="如 data.entries[*].explain" />
                </label>`).join('')}
            </div>
            <p class="muted" style="margin-top:8px">
              路径语法：<code>a.b.c</code> 逐层取值 · <code>a[0].b</code> 数组下标 ·
              <code>a[*].b</code> 遍历收集 · <code>$</code> 根对象
            </p>
          </details>
        </div>
      </div>`).join('');

    // 绑定：启用开关 / 名称
    box.querySelectorAll('.source-item').forEach(item => {
      const idx = parseInt(item.dataset.idx, 10);
      const s = sources[idx];

      item.querySelector('[data-f="enabled"]')?.addEventListener('change', (e) => {
        s.enabled = e.target.checked;
      });
      item.querySelector('[data-f="name"]')?.addEventListener('input', (e) => {
        s.name = e.target.value;
      });
      item.querySelector('[data-f="url_template"]')?.addEventListener('input', (e) => {
        s.url_template = e.target.value;
      });
      item.querySelector('[data-f="method"]')?.addEventListener('change', (e) => {
        s.method = e.target.value;
      });
      item.querySelector('[data-f="api_key"]')?.addEventListener('input', (e) => {
        s.api_key = e.target.value;
      });

      item.querySelectorAll('[data-lang]').forEach(cb => {
        cb.addEventListener('change', () => {
          const langs = new Set(s.langs || []);
          if (cb.checked) langs.add(cb.dataset.lang);
          else langs.delete(cb.dataset.lang);
          s.langs = Array.from(langs);
        });
      });

      item.querySelectorAll('[data-map]').forEach(inp => {
        inp.addEventListener('input', () => {
          s.mapping = s.mapping || {};
          s.mapping[inp.dataset.map] = inp.value;
        });
      });

      item.querySelector('[data-act="test"]')?.addEventListener('click', async (e) => {
        const out = item.querySelector('.source-test-result');
        out.className = 'source-test-result';
        out.textContent = '测试中…';
        try {
          const r = await API.testSource(s);
          out.textContent = (r.ok ? '✓ ' : '✗ ') + r.message;
          out.classList.add(r.ok ? 'ok' : 'fail');
        } catch (err) {
          out.textContent = '✗ ' + err.message;
          out.classList.add('fail');
        }
      });

      item.querySelector('[data-act="del"]')?.addEventListener('click', () => {
        sources.splice(idx, 1);
        renderSources();
      });
    });
  }

  function addSource() {
    sources.push({
      id: 'custom-' + Date.now(),
      name: '自定义词典源',
      builtin: false,
      enabled: true,
      langs: ['en'],
      url_template: 'https://example.com/api/dict?word={word}',
      method: 'GET',
      headers: {},
      api_key: '',
      priority: 100,
      mapping: {
        word: 'word',
        phonetic_uk: 'phonetic',
        phonetic_us: '',
        audio: '',
        senses: 'meanings',
        sense_pos: 'partOfSpeech',
        sense_def: 'definitions[0].definition',
        sense_examples: 'definitions[*].example',
        example_text: '',
        example_translation: '',
        inflections: '',
        inflection_label: '',
        inflection_form: '',
        related: 'synonyms',
        mnemonic: '',
      },
    });
    renderSources();
    U().toast('已新增词典源，请填写地址与字段映射');
  }

  async function resetSources() {
    if (!confirm('确定恢复为内置词典源？自定义配置将被清除。')) return;
    try {
      sources = await API.resetSources();
      await load();
      U().toast('已恢复内置词典源', 'ok');
    } catch (e) {
      U().toast(e.message, 'err');
    }
  }

  async function testLlm() {
    const out = document.getElementById('llm-test-result');
    if (out) out.textContent = '连接中…';
    try {
      // 先保存当前填写的地址
      if (config) {
        config.llm.base_url = getVal('set-llm-url') || config.llm.base_url;
        config.llm.model = getVal('set-llm-model');
        await API.saveConfig(config);
      }
      const st = await API.llmStatus();
      if (out) {
        out.textContent = st.online
          ? `✓ ${st.message}${st.active_model ? '（当前：' + st.active_model + '）' : ''}`
          : '✗ ' + st.message;
        out.style.color = st.online ? '#17a673' : '#e5484d';
      }
      if (window.App) window.App.updateLlmChip(st);
    } catch (e) {
      if (out) { out.textContent = '✗ ' + e.message; out.style.color = '#e5484d'; }
    }
  }

  function bind() {
    document.getElementById('btn-save-settings')?.addEventListener('click', save);
    document.getElementById('btn-add-source')?.addEventListener('click', addSource);
    document.getElementById('btn-reset-sources')?.addEventListener('click', resetSources);
    document.getElementById('btn-llm-test')?.addEventListener('click', testLlm);
    document.getElementById('btn-net-detect')?.addEventListener('click', detectNetwork);
    document.getElementById('btn-net-apply')?.addEventListener('click', applyNetwork);
    document.getElementById('set-proxy')?.addEventListener('keydown', (e) => {
      if (e.key === 'Enter') applyNetwork();
    });
    // 打开/关闭代理总开关时，让下面的地址输入项跟着可用/置灰
    document.getElementById('set-enable-proxy')
      ?.addEventListener('change', syncProxyFields);

    document.getElementById('btn-clear-cache')?.addEventListener('click', async () => {
      try {
        await API.clearCache();
        U().toast('词典缓存已清空', 'ok');
      } catch (e) { U().toast(e.message, 'err'); }
    });

    // 开关即时生效（不必点保存）
    document.querySelectorAll('[data-opt]').forEach(el => {
      el.addEventListener('change', async () => {
        if (!config) return;
        config.study[el.dataset.opt] = el.checked;
        try { await API.setStudyOptions(config.study); } catch (e) {}
      });
    });
  }

  return { bind, load, save, get config() { return config; } };
})();

window.Settings = Settings;
