/* ============================================================
   settings.js —— 设置面板
   记忆辅助开关（需求 3）、AI 服务（本地模型 / 在线 API）、词典源配置（需求 6）
   ============================================================ */

/* ------------------------------------------------------------
   AI 服务来源预设。

   为什么只放「OpenAI 兼容」的服务：后端 llm 客户端只会说这一套协议
   （/chat/completions + Bearer Key）。收窄到这个范围，用户点一下就能用，
   不用自己拼地址，也不用担心选了个程序不支持的。

   地址一律写到「版本号那一段」为止（`.../v1`、`.../v4`）：
   后端 endpoint() 见到版本号后缀就直接接 `/chat/completions`，
   见到别的才补 `/v1`。多写一段会拼成 `/v1/v1/...`。
   ------------------------------------------------------------ */
const AI_PRESETS = [
  {
    id: 'local-managed', name: '本机一键部署（免费离线）',
    base: '', model: '', local: true,
    hint: '用下载到本机的 Qwen 模型。不需要 API Key，不联网，数据不出机器。' +
          '先在下个面板「本地大模型一键部署」里下好模型，再点左下角状态条启动。',
  },
  {
    id: 'lm-studio', name: '本机 LM Studio（免费离线）',
    base: 'http://127.0.0.1:1234/v1', model: '', local: true,
    hint: '先在 LM Studio 里加载模型，再到 Developer 标签点 Start Server，' +
          '然后回来点「测试连接」。地址栏保持默认的 1234 端口即可。',
  },
  {
    id: 'deepseek', name: 'DeepSeek（深度求索）',
    base: 'https://api.deepseek.com/v1', model: 'deepseek-chat',
    keyUrl: 'https://platform.deepseek.com/api_keys',
    hint: '国内直连可用，中文讲解质量好、价格便宜，是最推荐的在线选择。',
  },
  {
    id: 'dashscope', name: '阿里云通义千问',
    base: 'https://dashscope.aliyuncs.com/compatible-mode/v1', model: 'qwen-plus',
    keyUrl: 'https://bailian.console.aliyun.com/',
    hint: '用「兼容模式」地址（带 compatible-mode）。国内直连可用。',
  },
  {
    id: 'moonshot', name: '月之暗面 Kimi',
    base: 'https://api.moonshot.cn/v1', model: 'moonshot-v1-8k',
    keyUrl: 'https://platform.moonshot.cn/console/api-keys',
    hint: '国内直连可用，长文本见长。',
  },
  {
    id: 'zhipu', name: '智谱 GLM',
    base: 'https://open.bigmodel.cn/api/paas/v4', model: 'glm-4-flash',
    keyUrl: 'https://bigmodel.cn/usercenter/apikeys',
    hint: 'glm-4-flash 目前免费额度较大。注意它的地址结尾是 v4 而不是 v1。',
  },
  {
    id: 'siliconflow', name: '硅基流动 SiliconFlow',
    base: 'https://api.siliconflow.cn/v1', model: 'Qwen/Qwen2.5-7B-Instruct',
    keyUrl: 'https://cloud.siliconflow.cn/account/ak',
    hint: '一家聚合服务，一个 Key 能调很多开源模型，模型名要写全（带 `组织/模型`）。',
  },
  {
    id: 'openrouter', name: 'OpenRouter（聚合，需代理）',
    base: 'https://openrouter.ai/api/v1', model: '',
    keyUrl: 'https://openrouter.ai/keys',
    hint: '聚合了各家模型，一个 Key 通用。服务器在境外，' +
          '一般要在「设置 → 网络与代理」里打开代理才连得上。',
  },
  {
    id: 'openai', name: 'OpenAI（需代理）',
    base: 'https://api.openai.com/v1', model: 'gpt-4o-mini',
    keyUrl: 'https://platform.openai.com/api-keys',
    hint: '服务器在境外，一般要在「网络 → 代理」里打开代理才连得上。',
  },
  {
    id: 'custom', name: '自定义（任意 OpenAI 兼容服务）',
    base: '', model: '',
    hint: '填其它 OpenAI 兼容服务的地址即可。地址写到版本号那段为止；' +
          '如果服务不带版本号（如自建网关），直接填到根路径也行。',
  },
];

/** 一键部署托管的端口段，必须与后端 localllm::pick_port 保持一致。 */
const MANAGED_PORT_MIN = 18080;
const MANAGED_PORT_MAX = 18180;

/** 从地址反推属于哪个预设，让下拉框跟着手填的地址走。 */
function detectPreset(base) {
  const b = String(base || '').trim().replace(/\/+$/, '');
  if (!b) return 'custom';
  const lower = b.toLowerCase();
  // 托管服务端口不固定（18080 起找一个空的），只能按段认
  const m = lower.match(/(?:127\.0\.0\.1|localhost|\[::1\]):(\d+)/);
  if (m) {
    const p = parseInt(m[1], 10);
    if (p >= MANAGED_PORT_MIN && p < MANAGED_PORT_MAX) return 'local-managed';
  }
  const hit = AI_PRESETS.find((p) => p.base && p.base.replace(/\/+$/, '') === b);
  return hit ? hit.id : 'custom';
}

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

    // AI 服务：地址 / 模型 / Key
    setVal('set-llm-url', config.llm.base_url);
    setVal('set-llm-model', config.llm.model);
    setVal('set-llm-key', config.llm.api_key || '');
    setVal('set-llm-temp', config.llm.temperature);
    setVal('set-llm-maxtok', config.llm.max_tokens);
    syncPresetUI();

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

    // 界面语言 + AI 讲解语言：两件不同的事，但放在同一处管理。
    //   界面语言 = 软件自己说什么语言（ui_lang）
    //   讲解语言 = AI 给我讲的时候说什么语言（explain_lang）
    // 界面语言立刻切走 UI（不等保存），讲解语言仍走「保存」落盘。
    const uiLang = document.getElementById('set-ui-lang');
    if (uiLang) {
      const code = (config.ui_lang || window.I18n?.current?.() || 'zh-CN');
      uiLang.innerHTML = (window.I18n?.LANGS || [{ id: 'zh-CN', name: '简体中文' }])
        .map(l => `<option value="${l.id}">${l.name}</option>`).join('');
      uiLang.value = code;
    }

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

    // 朗读音色/语速（存在 localStorage，不属于后端配置）
    loadSpeak();
    // 本地语音包与引擎状态（要读后端，异步渲染）
    loadTts();
    // 日志诊断自动跑一次（打开设置页即见结果，不用手点）
    if (API.logAnalysis) renderLogAnalysis();
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
    // Key 留空就写空串（请求时不带 Authorization 头）。不能「留空则保持原值」：
    // 用户清掉 Key 就是想停用它，偷偷留着旧的反而会拿废弃 Key 去请求，
    // 收到 401 还找不到原因。
    config.llm.api_key = getVal('set-llm-key').trim();
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

    // 界面语言：不等「保存」按钮，选了立刻换。
    // 这里的取舍是故意的——语言不像「每批多少题」那样需要反悔，
    // 让用户拿不准时能立刻看到效果，比多点一次保存更重要。
    const uiLang = document.getElementById('set-ui-lang');
    if (uiLang) {
      config.ui_lang = uiLang.value || 'zh-CN';
      try { window.I18n?.setLang(config.ui_lang); } catch (e) { /* 字典坏了也别挡住保存 */ }
    }

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
        // 测连接必须带上当前填的 Key，否则在线 API 必然测出 401，
        // 用户会以为「地址填对了却连不上」
        config.llm.api_key = getVal('set-llm-key').trim();
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

  /* ---------------- AI 服务来源 ---------------- */

  /** 按当前地址刷新「服务来源」下拉与提示文案。 */
  function syncPresetUI() {
    const sel = document.getElementById('set-llm-preset');
    if (!sel) return;
    // 只在第一次建 option：每次同步都重造会闪，还会把用户当前的选择冲掉
    if (!sel.options || !sel.options.length) {
      sel.innerHTML = AI_PRESETS
        .map((p) => `<option value="${U().esc(p.id)}">${U().esc(p.name)}</option>`)
        .join('');
    }
    const base = getVal('set-llm-url') || (config && config.llm.base_url) || '';
    const id = detectPreset(base);
    sel.value = id;
    renderPresetHint(id);
  }

  function renderPresetHint(id) {
    const p = AI_PRESETS.find((x) => x.id === id) || AI_PRESETS[AI_PRESETS.length - 1];
    const hint = document.getElementById('llm-preset-hint');
    if (hint) hint.textContent = p.hint || '';
    // 「申请 API Key」只在需要 Key 的来源下露面，本机模型点了也没意义
    const btn = document.getElementById('btn-llm-getkey');
    if (btn) {
      btn.classList.toggle('hidden', !p.keyUrl);
      if (p.keyUrl) btn.dataset.url = p.keyUrl;
    }
  }

  /**
   * 选一个来源：带出地址和推荐模型。
   *
   * 刻意**不动 API Key**：很多人会在几家中来回试，一把一清会逼着反复粘贴。
   */
  async function applyPreset(id) {
    const p = AI_PRESETS.find((x) => x.id === id);
    if (!p) return;
    renderPresetHint(id);

    // 自定义：地址和模型都留给用户自己填
    if (id === 'custom') return;

    if (id === 'local-managed') {
      // 托管服务的端口是运行时挑的（18080 起找一个空闲的），没有静态地址可填，
      // 只能问后端实际监听在哪。没启动就什么都别改 —— 填个死端口更误导。
      try {
        const s = await API.localLlmStatus();
        const sv = (s && s.server) || {};
        if (sv.running && sv.port) {
          setVal('set-llm-url', `http://127.0.0.1:${sv.port}/v1`);
        } else {
          U().toast('本机服务还没启动：点左下角状态条启动后，地址会自动填上', 'err');
        }
      } catch (e) { /* 读不到就保持原样 */ }
      setVal('set-llm-model', '');
      return;
    }

    if (p.base) setVal('set-llm-url', p.base);
    // 本机来源把模型清空，让后端走「自动选第一个已加载模型」
    setVal('set-llm-model', p.model || '');
  }

  function toggleKeyVisible() {
    const inp = document.getElementById('set-llm-key');
    const btn = document.getElementById('btn-llm-key-eye');
    if (!inp) return;
    const show = inp.type === 'password';
    inp.type = show ? 'text' : 'password';
    if (btn) btn.textContent = show ? '隐藏' : '显示';
  }

  /* ---- 朗读与发音（离线系统语音） ---- */

  /**
   * 填充音色下拉。
   *
   * 音色列表来自 WebView 的 `speechSynthesis`，**首次调用常常是空的**
   * （引擎异步加载），所以这里在 `voiceschanged` 时再刷一次；一直为空就
   * 如实告诉用户「系统里没装语音包」，而不是给一个永远空的下拉。
   */
  /** 上次渲染的音色清单指纹；用来判断「列表真的变了没」。 */
  let speakVoicesSig = null;
  /** 上次渲染用的那个 `<select>` 元素。 */
  let speakVoicesFor = null;
  /** 音色迟迟读不到时的重试定时器。 */
  let speakVoicesTimer = null;

  /** 音色清单的指纹（语言 + 名字）。 */
  function voiceSignature(list) {
    return list.map(v => `${v.lang}|${v.name}`).join('\n');
  }

  /**
   * 让下拉的选中项跟用户偏好对齐。
   *
   * 单独抽出来是因为两个时机都要跑：重建之后、以及「列表没变、只同步不重建」。
   * `sel.value` 指向已不存在的 option 时浏览器会把它变成空串 —— 这种情况下
   * 顺手把过期的偏好清掉，免得每次进设置页都白找一遍。
   */
  function syncVoiceSelection() {
    const sel = document.getElementById('set-speak-voice');
    if (!sel) return;
    const S = window.Speak;
    const pref = (S && S.voicePref) ? S.voicePref() : '';
    const want = pref.includes('|') ? pref : '';
    if (sel.value !== want) sel.value = want;
    if (want && sel.value !== want && S && S.setVoicePref) S.setVoicePref('', '');
  }

  /**
   * 音色列表为空时的有限重试。
   *
   * WebView2 首次 `getVoices()` 常返回空数组（语音引擎在后台加载），而且
   * **不保证**之后一定会触发 voiceschanged。没有这个重试，下拉就永久停在
   * 「（系统未提供可用语音）」—— 用户看到的就是「系统音色根本没法选」。
   */
  function retrySpeakVoices(times) {
    clearTimeout(speakVoicesTimer);
    const left = typeof times === 'number' ? times : 10;
    if (left <= 0) return;
    speakVoicesTimer = setTimeout(() => {
      const S = window.Speak;
      const list = (S && S.voices) ? S.voices() : [];
      if (list.length) { renderSpeakVoices(true); return; }
      retrySpeakVoices(left - 1);
    }, 400);
  }

  /**
   * 填充音色下拉。
   *
   * @param {boolean} [force] 清单没变时也强制重建（列表从空变为有值时用）
   */
  function renderSpeakVoices(force) {
    const sel = document.getElementById('set-speak-voice');
    if (!sel) return;
    const S = window.Speak;
    const list = (S && S.voices) ? S.voices() : [];

    if (!list.length) {
      // 只在「上一次不是空」时写一次；反复重建会打断用户正在操作的下拉
      if (speakVoicesSig !== '') {
        speakVoicesSig = '';
        sel.innerHTML = '<option value="">（系统未提供可用语音）</option>';
      }
      const hint = document.getElementById('set-speak-voices-hint');
      if (hint) {
        hint.textContent = '这台机器上暂时读不到系统语音。可在「Windows 设置 → 时间和语言 → 语音」'
          + '安装语音包后重开本程序；朗读按钮届时会自动可用。';
      }
      retrySpeakVoices();
      return;
    }

    const sig = voiceSignature(list);
    // ★ 清单没变、且下拉还是**同一个元素**时，只同步选中项，绝不重建
    //   innerHTML。重建会把用户正在展开 / 正在选择的下拉瞬间打断 ——
    //   表现就是「点了没反应」「选完又弹回去」。voiceschanged 在部分
    //   WebView 里触发得相当频繁，每次重建都等于把用户刚做的选择抹掉一次。
    //
    //   还要比元素引用：只比签名的话，DOM 若被整体换过（如切换界面语言
    //   重建页面），`sel` 是个全新的空下拉，而签名还停在旧值上 ——
    //   跳过重建就会留下一个永远空着的下拉。
    if (!force && sig === speakVoicesSig && speakVoicesFor === sel) {
      syncVoiceSelection();
      return;
    }
    speakVoicesSig = sig;
    speakVoicesFor = sel;

    // 按语言分组，方便在几十个音色里找
    const byLang = {};
    list.forEach(v => { (byLang[v.lang] = byLang[v.lang] || []).push(v); });
    const langs = Object.keys(byLang).sort();
    sel.innerHTML = '<option value="">自动（按语言挑最好的音色）</option>'
      + langs.map(l => `<optgroup label="${U().esc(l)}">`
        + byLang[l].map(v => `<option value="${U().esc(v.lang + '|' + v.name)}">`
          + `${U().esc(v.name)}${v.local ? '' : '（在线）'}</option>`).join('')
        + '</optgroup>').join('');
    // 选中态统一由 syncVoiceSelection 落，避免两处各写一份判断
    syncVoiceSelection();

    // ---- 提示语：不说「检测到 N 个」这种没有信息量的废话 ----
    //
    // 现场：用户机器上一共 7 个音色，日语 4 个 + 中文 3 个，**一个英语的都没有**，
    // 而他在学英语。旧文案「检测到 7 个系统音色」让人以为一切正常，于是
    // 「点了朗读没声音 / 音色里选不到英语」就成了悬案 —— 界面既没说缺什么，
    // 也没说去哪儿装。这里把两件事补齐：先说英语到底有没有，再给 Windows
    // 的具体路径。
    const hint = document.getElementById('set-speak-voices-hint');
    if (hint) {
      const base = (s) => String(s || '').toLowerCase().replace('_', '-').split('-')[0];
      const hasBase = new Set(list.map(v => base(v.lang)));
      const study = currentStudyLang();
      const absent = LANG_OPTIONS.filter(([code]) => !hasBase.has(base(code)));

      const parts = [];
      if (!hasBase.has(base(study))) {
        const nm = U().langLabel(study);
        parts.push(`本机没有${nm}的系统语音，朗读${nm}单词时会没声音或发音不准。`);
      }
      if (absent.length) {
        parts.push(`未检测到：${absent.map(([, n]) => n).join('、')}。`);
      }
      parts.push('到「Windows 设置 → 时间和语言 → 语音 → 添加语音」安装对应语音包，装完重开本程序即可在下拉里选择；'
        + '也可以直接装上面的离线语音包（Piper），不依赖系统语音。');
      hint.textContent = parts.join(' ');
    }
  }
  function loadSpeak() {
    const S = window.Speak;
    if (!S) return;
    const rate = S.ratePref ? S.ratePref() : 0.95;
    const slider = document.getElementById('set-speak-rate');
    if (slider) slider.value = String(rate);
    const label = document.getElementById('set-speak-rate-val');
    if (label) label.textContent = `${Number(rate).toFixed(2)}×`;
    renderSpeakVoices();
  }

  function bindSpeak() {
    const S = () => window.Speak;

    document.getElementById('set-speak-voice')?.addEventListener('change', (e) => {
      const v = e.target.value || '';
      const i = v.indexOf('|');
      if (i > 0) S().setVoicePref(v.slice(0, i), v.slice(i + 1));
      else S().setVoicePref('', '');
      // 底部「当前语音模型」跟着一起变，用户不用离开这一屏就能确认选上了
      renderTtsFooter();
      U().toast('音色已保存，点「试听」确认', 'ok');
    });

    document.getElementById('set-speak-rate')?.addEventListener('input', (e) => {
      const r = parseFloat(e.target.value) || 0.95;
      const label = document.getElementById('set-speak-rate-val');
      if (label) label.textContent = `${r.toFixed(2)}×`;
      S().setRatePref(r);   // 拖动即生效，不用再点保存
      // 本地引擎的语速在后端配置里，拖动时防抖写一次（每动一下就 IPC 太浪费）
      clearTimeout(ttsRateTimer);
      ttsRateTimer = setTimeout(() => {
        API.setTtsPrefs({ rate: r }).catch(() => { /* 落盘失败不影响本次会话 */ });
      }, 400);
    });

    document.getElementById('btn-speak-test')?.addEventListener('click', () => {
      S().preview('Hello, this is how I read. 你好，这是朗读效果。', 'en');
    });

    document.getElementById('btn-speak-reset')?.addEventListener('click', () => {
      S().resetPrefs();
      loadSpeak();
      U().toast('朗读设置已恢复默认', 'ok');
    });

    // 引擎异步加载完音色后会触发（首次进设置页常常还没就绪）。
    //
    // ★ 用 addEventListener 而不是给 `onvoiceschanged` 赋值：那是**单槽位**
    //   属性，speak.js 也在用同一招做预加载，两边互相覆盖 —— 谁后绑定谁生效，
    //   另一个就永远收不到通知。用事件监听才能两边都收到。
    if (window.speechSynthesis) {
      const onVoices = () => renderSpeakVoices();
      try {
        window.speechSynthesis.addEventListener('voiceschanged', onVoices);
      } catch (e) {
        try { window.speechSynthesis.onvoiceschanged = onVoices; } catch (e2) { /* 忽略 */ }
      }
    }
  }

  /* ---- 本地神经语音（Piper）：引擎与语音包 ---- */

  /*
    两条硬约束决定了这里的交互：
      1. 语音包不小（20~120 MB），所以下载全程必须有进度，不能只有一个转圈；
      2. 引擎没装时**必须禁用**语音包的下载按钮 —— 否则用户下完 60 MB 语音包
         仍然发不出声，还会以为功能坏了。
  */
  let ttsProgressUnlisten = null;
  let ttsRateTimer = null;
  /** 「当前语音」订阅的退订函数（只订一次，面板反复进出不叠加）。 */
  let ttsVoiceUnsub = null;
  /** 语音健康订阅的退订函数（同上）。 */
  let ttsHealthUnsub = null;

  /*
    语音包按语言分组后，组一多整页就长得没法扫 —— 而且用户真正关心的
    通常只有「我**正在学**的那门语言」，其余语言是干扰（需求：加折叠）。

    折叠状态按语言代码记在模块级 Map 里：
      · 未记录 → 走默认（只展开「正在学的语言」那一组）；
      · 用户手动点过 → 永远以用户最后一次点击为准。
    因为下载/删除语音包都会整块重渲染，若状态只存 DOM，用户刚展开的组
    会在下载完成后被合回去，看起来像界面抽风。
  */
  const ttsLangOpen = new Map();

  /**
   * 当前生效的**本地语音包 id**（后端 `cfg.tts.voice_local`）。
   *
   * 空串 = 后端按词条语言自动挑。这个值同时决定两件事：
   *   · 语音包列表里哪一条显示「使用中」；
   *   · 下载完成后要不要自动接管（见 bindTts 里下载分支）。
   * 所以它在每次 loadTts 后都要与后端对齐，不能只信本地。
   */
  let ttsCurrentVoice = '';

  /** id → 显示名。底部「当前语音模型」要显示人话而不是 `en_US-amy-medium`。 */
  let ttsVoiceLabels = {};

  /** 当前正在学的语言：优先取 App 已加载的配置，其次看设置里的下拉。 */
  function currentStudyLang() {
    try {
      if (window.App && window.App.config && window.App.config.target_lang) {
        return String(window.App.config.target_lang);
      }
    } catch (e) { /* 忽略 */ }
    const sel = document.getElementById('set-lang');
    return (sel && sel.value) || 'en';
  }

  /** 两个语言码是否指代同一「主语言」（en / en-US / en_US 视为同一门）。 */
  function sameBase(a, b) {
    const base = (s) => String(s || '').toLowerCase().replace('_', '-').split('-')[0];
    return base(a) === base(b);
  }

  /**
   * 本次渲染该默认展开哪一组。
   *
   * 规则：优先「正在学的语言」那组；**若它压根没有语音包**（e.g. 在学韩语而
   * 清单里只有英/中），那就展开第一组 —— 否则整页只剩一排折叠标题，用户会
   * 以为列表是空的 / 功能坏了。一次只展开一组，剩下的靠标题上的「已装/总数」
   * 给线索。
   */
  function ttsDefaultOpenLang(groups) {
    const study = currentStudyLang();
    const hit = groups.find(g => sameBase(g.lang, study));
    return hit ? hit.lang : (groups[0] ? groups[0].lang : '');
  }

  function ttsEl(id) { return document.getElementById(id); }

  /**
   * 把某一组的折叠状态**同时**写进 Map 与 DOM。
   *
   * 状态只有 `ttsLangOpen` 这一个来源，DOM 只是它的投影 —— 这样「点击就地开合」
   * 与「整块重渲染」不会各持一份互相打架。
   *
   * 之前是交互只改 DOM、渲染只读 Map，两边一旦不同步就会出现用户报的那几个
   * 现象：点了像没反应（改的是根本不生效的 `hidden` 属性）、刚展开又被合回去
   * （重渲染按旧 Map 画）、多个分组跟着一起变（DOM 与 Map 各说各话）。
   *
   * @param {Element} group `.tts-lang-group` 元素
   * @param {string}  lang  语言代码（Map 的键）
   * @param {boolean} open  目标状态
   */
  function applyLangOpen(group, lang, open) {
    if (!group) return;
    ttsLangOpen.set(lang, open);
    group.classList.toggle('open', open);
    const head = group.querySelector('.tts-lang-head');
    if (head) head.setAttribute('aria-expanded', open ? 'true' : 'false');
    const caret = group.querySelector('.tts-caret');
    if (caret) caret.textContent = open ? '▾' : '▸';
    // ★ 必须用 class：`.tts-lang-body { display: flex }` 会盖掉浏览器对
    //   `[hidden]` 的默认 `display:none`，属性写法等于没写。
    const body = group.querySelector('.tts-lang-body');
    if (body) body.classList.toggle('hidden', !open);
  }

  function humanSize(n) {
    const f = Number(n) || 0;
    if (f >= 1048576) return `${(f / 1048576).toFixed(1)} MB`;
    return `${Math.round(f / 1024)} KB`;
  }

  function ttsShowProgress(p) {
    const wrap = ttsEl('tts-progress');
    const fill = ttsEl('tts-progress-fill');
    const text = ttsEl('tts-progress-text');
    if (!wrap) return;
    wrap.classList.remove('hidden');
    if (fill) {
      // percent < 0 = 总长未知，给个来回动的状态，别停在 0% 让人以为卡死
      fill.style.width = p.percent >= 0 ? `${Math.min(100, p.percent).toFixed(1)}%` : '35%';
      fill.classList.toggle('unknown', p.percent < 0);
    }
    if (text) {
      const size = p.total ? `${humanSize(p.got)} / ${humanSize(p.total)}` : humanSize(p.got);
      text.textContent = [p.message, p.percent >= 0 ? size : '', p.speed].filter(Boolean).join(' · ');
    }
  }

  function ttsHideProgress() {
    ttsEl('tts-progress')?.classList.add('hidden');
  }

  /** 拉后端状态并渲染语音包清单。 */
  async function loadTts() {
    const box = ttsEl('tts-voice-list');
    if (!box || !API.ttsStatus) return;
    let st;
    try {
      st = await API.ttsStatus();
    } catch (e) {
      box.innerHTML = `<p class="muted">读取语音包状态失败：${U().esc(String((e && e.message) || e))}</p>`;
      return;
    }
    if (!st) return;

    const installed = st.installed || [];
    const engineOk = !!st.engine_ready;

    // 与后端对齐「当前生效的本地语音」。必须在下面渲染之前取 ——
    // voiceRow 要靠它决定哪一行显示「使用中」。
    ttsCurrentVoice = (st.config && st.config.voice_local) || '';
    if (window.Speak && window.Speak.setLocalVoiceId) {
      window.Speak.setLocalVoiceId(ttsCurrentVoice);
    }
    ttsVoiceLabels = {};
    (st.voices || []).forEach(v => { ttsVoiceLabels[v.id] = v.label; });

    // 把「本地能不能用」同步给发音模块：不可用时 speak() 直接走系统语音，
    // 不必每次点喇叭都去后端撞一次墙再回退。
    if (window.Speak && window.Speak.setLocalReady) {
      window.Speak.setLocalReady(engineOk && installed.length > 0);
    }

    const stateEl = ttsEl('tts-state');
    if (stateEl) {
      stateEl.textContent = !engineOk
        ? '未安装语音引擎'
        : (installed.length ? `已安装 ${installed.length} 个语音包` : '引擎就绪，尚未安装语音包');
    }

    const engRow = ttsEl('tts-engine-row');
    if (engRow) engRow.classList.toggle('hidden', engineOk);
    const engHint = ttsEl('tts-engine-hint');
    if (engHint) {
      engHint.textContent = st.engine_bundled
        ? '语音引擎已随程序预置，无需下载'
        : `引擎约 ${st.engine_size_text || ''}，只需下载一次，所有语音包共用`;
    }

    // ---- 语音包清单：按语言分组（需求 9） ----
    //
    // 平铺一列在语音包变多之后就没法用了：用户真正要回答的问题是
    // 「我想学的这门语言，语音包装了吗 / 有没有得装」——
    // 分组 + 每组的「已装/总数」一眼就能回答，不用逐行读语言标签。
    const voices = st.voices || [];
    const byLang = new Map();
    voices.forEach(v => {
      const k = (v.lang || '').trim() || 'other';
      if (!byLang.has(k)) byLang.set(k, []);
      byLang.get(k).push(v);
    });

    const groups = [...byLang.entries()].map(([lang, list]) => {
      // 已装的排前面：用户多半是来管理自己正在学的那门语言
      const sorted = list.slice().sort((a, b) => (b.installed ? 1 : 0) - (a.installed ? 1 : 0));
      return { lang, list: sorted, inst: list.filter(v => v.installed).length, total: list.length };
    }).sort((a, b) => (b.inst - a.inst) || a.lang.localeCompare(b.lang));

    const voiceRow = (v) => {
      // 随包预置的那份在安装目录里，删不掉（下次覆盖安装还会回来），
      // 所以不给删除按钮，只标出来源。
      const bundled = v.source === 'bundled';
      // 「使用中」= 后端正在拿它合成。下载了不等于在用 —— 这两件事必须
      // 分开显示，否则用户会以为「下完了怎么还是系统音色」。
      const inUse = v.installed && !!ttsCurrentVoice && v.id === ttsCurrentVoice;
      let act;
      if (v.installed) {
        act = (inUse
          ? '<span class="tag inuse">使用中</span>'
          // 没被指定的已装语音给一个切换入口 —— 否则下载完就没法「选中生效」，
          // 用户只能靠后端按语言自动挑，装了多条时无从选择。
          : `<button class="ghost-btn xs" data-tts-use="${U().esc(v.id)}">使用</button>`)
          + `<span class="tag ok">${bundled ? '随包预置' : '已下载'}</span>`
          + (bundled ? '' : `<button class="ghost-btn xs" data-tts-remove="${U().esc(v.id)}">删除</button>`);
      } else {
        act = `<button class="ghost-btn xs" data-tts-install="${U().esc(v.id)}"${engineOk ? '' : ' disabled'}>`
          + `下载 ${U().esc(v.size_text || '')}</button>`;
      }
      const meta = [
        v.accent ? String(v.accent).toUpperCase() : '',
        v.size_text || '',
        v.preset ? '预置' : '',
      ].filter(Boolean).join(' · ');
      // 小体积模型有已知短板（音素表不全），如实写在名字下面，别让人以为捡了便宜
      const note = v.note
        ? `<span class="tts-note">${U().esc(v.note)}</span>`
        : '';
      return `<div class="tts-voice-row${inUse ? ' in-use' : ''}">
        <div class="tts-voice-main"><b>${U().esc(v.label)}</b><span class="muted">${U().esc(meta)}</span>${note}</div>
        <div class="tts-voice-act">${act}</div>
      </div>`;
    };

    // 有清单里没覆盖到的**学习语言**，必须如实列出来 ——
    // 用户找不到日语包时的第一反应是「软件漏了」，而不是「上游没有」。
    const covered = new Set(groups.map(g => g.lang));
    const missing = LANG_OPTIONS
      .filter(([code]) => !covered.has(code))
      .map(([, label]) => label);
    const missingHtml = missing.length
      ? '<div class="tts-missing"><b>暂不支持本地语音包：' + U().esc(missing.join('、')) + '</b>' +
        '<span>这几门语言在离线引擎（Piper）的官方仓库里没有可用音色 —— 仅有的日/韩语音需要新版引擎，' +
        '装上去也读不出声，所以宁可不出。朗读时会自动改用<b>在线语音</b>或<b>系统语音</b>，' +
        '在设置里把「朗读引擎」设为「自动」即可。</span></div>'
      : '';

    // 折叠交互：整块 `<button class="tts-lang-head">` 都能点，命中面积比
    // 只点一个小箭头大得多 —— 这正是「看到铁就知道这门语言有没有可装的包」。
    const defaultOpen = ttsDefaultOpenLang(groups);
    box.innerHTML = (groups.map(g => {
      const open = ttsLangOpen.has(g.lang) ? ttsLangOpen.get(g.lang) : g.lang === defaultOpen;
      const label = U().langLabel(g.lang);
      return '<div class="tts-lang-group' + (open ? ' open' : '') + '">' +
        '<button type="button" class="tts-lang-head" data-tts-fold="' + U().esc(g.lang) + '"'
          + ' aria-expanded="' + (open ? 'true' : 'false') + '">' +
          '<span class="tts-caret" aria-hidden="true">' + (open ? '▾' : '▸') + '</span>' +
          `<span class="tts-lang-name">${U().esc(label)}</span>` +
          // 折叠时这行「已装/总数」就是唯一线索，必须留着
          `<span class="tts-lang-count">${g.inst}/${g.total}</span>` +
        '</button>' +
        // ★ 折叠态用 `.hidden` **class**，不是 `hidden` **属性**。
        //   `.tts-lang-body { display: flex }` 是作者样式，会盖掉浏览器给
        //   `[hidden]` 的默认 `display: none`（作者样式优先于 UA 样式），
        //   于是「藏起来」这个动作根本不生效 —— 箭头变、内容还在，
        //   这就是「折叠点了没反应 / 状态错乱」的根因。
        //   本项目本来就约定用 `.hidden`（带 !important，见 app.css 顶部）。
        '<div class="tts-lang-body' + (open ? '' : ' hidden') + '">'
          + g.list.map(voiceRow).join('') + '</div>' +
      '</div>';
    }).join('') + missingHtml)
      || '<p class="muted">没有可用的语音清单。</p>';

    const hint = ttsEl('tts-hint');
    if (hint) {
      hint.textContent = engineOk
        ? '语音包按需下载，不随安装包分发；换语言学习时再装对应那条即可。'
        : '请先下载语音引擎，之后才能安装具体语种的语音包。';
    }

    const sel = ttsEl('set-tts-engine');
    if (sel && st.config) {
      const e = st.config.engine || 'auto';
      sel.value = e;
      if (window.Speak && window.Speak.setEnginePref) window.Speak.setEnginePref(e);
    }

    // 朗读输出设备：枚举本机所有输出设备（每台机器不同；有的机器默认设备
    // 失效会让 <audio> 全部播不出，指到别的设备即可绕开）
    const outSel = ttsEl('tts-output');
    if (outSel && navigator.mediaDevices && navigator.mediaDevices.enumerateDevices) {
      navigator.mediaDevices.enumerateDevices().then((devs) => {
        const outs = devs.filter((d) => d.kind === 'audiooutput');
        const cur = (window.Speak && window.Speak.outputPref) ? window.Speak.outputPref() : '';
        while (outSel.options.length > 1) outSel.remove(1);
        outs.forEach((d, i) => {
          const o = document.createElement('option');
          o.value = d.deviceId;
          // 设备名要媒体权限才拿得到；拿不到就用序号，至少能选
          o.textContent = d.label || ('输出设备 ' + (i + 1));
          outSel.appendChild(o);
        });
        outSel.value = cur;
        if (outSel.value !== cur) outSel.value = '';   // 记住的设备已不在了 → 回默认
      }).catch(() => { /* 枚举失败就只留系统默认 */ });
    }

    // 底部「当前语音模型」跟着一起刷新（它是 sticky 的，内容变了也要改）
    renderTtsFooter();
    // 侧边栏的「语音载入情况」同步刷新 —— 下载/删除语音包后不用等下次启动
    if (window.App && window.App.checkTts) window.App.checkTts();
  }

  /**
   * 刷新面板右下角那行「当前语音模型」。
   *
   * 优先级：用户指定的本地语音包 > 最近一次真正发声用的音色 > 自动。
   * 第三个分支是必要的 —— 一条语音包都没装、也还没听过任何声音时，
   * 显示「—」会让人以为功能坏了，如实说「自动（按语言挑选）」更准确。
   */
  function renderTtsFooter() {
    const el = ttsEl('tts-current-model');
    if (!el) return;
    if (ttsCurrentVoice) {
      el.textContent = ttsVoiceLabels[ttsCurrentVoice] || ttsCurrentVoice;
      el.title = ttsCurrentVoice;   // 截断时鼠标悬停能看到完整 id
      return;
    }
    const S = window.Speak;
    const cur = (S && S.currentVoice) ? S.currentVoice() : null;
    if (cur && cur.voice) {
      el.textContent = S.voiceLabel(cur.engine, cur.voice);
      el.title = el.textContent;
      return;
    }
    el.textContent = '自动（按语言挑选）';
    el.title = '';
  }

  function bindTts() {
    const sel = ttsEl('set-tts-engine');
    sel?.addEventListener('change', async () => {
      const e = sel.value || 'auto';
      // 先落内存偏好：发音是高频操作，不能等 IPC 回来才生效
      if (window.Speak && window.Speak.setEnginePref) window.Speak.setEnginePref(e);
      try {
        await API.setTtsPrefs({ engine: e });
      } catch (err) { /* 落盘失败也不影响本次会话 */ }
      U().toast('朗读引擎已切换', 'ok');
    });

    // 朗读输出设备：写进 Speak 偏好，元素与 WebAudio 两条通道都跟着走
    const outSel = ttsEl('tts-output');
    outSel?.addEventListener('change', () => {
      if (window.Speak && window.Speak.setOutputPref) window.Speak.setOutputPref(outSel.value || '');
      U().toast(outSel.value ? '朗读输出设备已切换' : '朗读输出设备已恢复系统默认', 'ok');
    });

    // 语音包列表是重渲染的，所以用事件委托而不是逐个绑定
    ttsEl('tts-voice-list')?.addEventListener('click', async (ev) => {
      // 分组标题：就地开合，**不重渲染** —— 重渲染会把焦点弄丢，连按几次
      // 展开就没反应了。状态由 applyLangOpen 统一落到 Map + DOM 两处。
      const head = ev.target.closest?.('.tts-lang-head[data-tts-fold]');
      if (head) {
        const lang = head.getAttribute('data-tts-fold');
        // 用 class 判断当前状态，与 applyLangOpen 的写入口径一致
        const group = head.closest('.tts-lang-group');
        const next = !(group && group.classList.contains('open'));
        applyLangOpen(group, lang, next);
        return;
      }

      // 「使用」某条已安装的语音 → 立即生效并记住。
      // ★ 只准走 Speak.selectVoice 这个全局门面：IPC 参数名是 camelCase
      //   （voiceLocal），手拼极易错成 snake_case —— 错了后端收到 null、
      //   配置根本没写，界面上却毫无异常，正是「点了使用却没切换」的根因。
      const use = ev.target.closest?.('[data-tts-use]');
      if (use) {
        const id = use.getAttribute('data-tts-use');
        use.disabled = true;
        try {
          await window.Speak.selectVoice(id);
          ttsCurrentVoice = id;
          U().toast('已切换本地语音，点「试听」确认', 'ok');
        } catch (e) {
          U().toast('切换失败：' + String((e && e.message) || e), 'err');
        }
        loadTts();
        return;
      }

      const ins = ev.target.closest?.('[data-tts-install]');
      const del = ev.target.closest?.('[data-tts-remove]');
      if (ins) {
        const id = ins.getAttribute('data-tts-install');
        ins.disabled = true;
        try {
          const r = await API.ttsInstallVoice(id);
          // 下载完**立即应用**：用户花几十 MB 流量下的东西，就是为了让它发声。
          // 只在「当前还没指定语音」时才自动接管 —— 他已经明确选过另一条的话，
          // 抢过来会把用户的设置改掉。
          if (!ttsCurrentVoice) {
            await window.Speak.selectVoice(id);
            ttsCurrentVoice = id;
          }
          U().toast((r && r.message) || '语音包已安装', 'ok');
        } catch (e) {
          U().toast('语音包下载失败：' + String((e && e.message) || e), 'err');
        } finally {
          loadTts();
        }
      } else if (del) {
        const id = del.getAttribute('data-tts-remove');
        try {
          await API.ttsRemoveVoice(id);
          // 删掉的正好是「当前使用」的那条 → 一并清掉指定，免得残留一个
          // 指向不存在文件的 voice_local（它会让每次朗读都先失败再回退）
          if (ttsCurrentVoice === id) {
            await window.Speak.selectVoice('');
            ttsCurrentVoice = '';
          }
          U().toast('已删除语音包', 'ok');
        } catch (e) {
          U().toast('删除失败：' + String((e && e.message) || e), 'err');
        }
        loadTts();
      }
    });

    ttsEl('btn-tts-install-engine')?.addEventListener('click', async () => {
      const btn = ttsEl('btn-tts-install-engine');
      if (btn) btn.disabled = true;
      try {
        const r = await API.ttsInstallEngine();
        U().toast((r && r.message) || '语音引擎已就绪', 'ok');
      } catch (e) {
        U().toast('引擎安装失败：' + String((e && e.message) || e), 'err');
      } finally {
        if (btn) btn.disabled = false;
        loadTts();
      }
    });

    ttsEl('btn-tts-clear-cache')?.addEventListener('click', async () => {
      try {
        const r = await API.ttsClearCache();
        U().toast(`已清理 ${(r && r.removed) || 0} 个语音缓存（${(r && r.freed) || '0 KB'}）`, 'ok');
      } catch (e) {
        U().toast('清理失败', 'err');
      }
    });

    // 进度订阅只订一次（面板每次进入都会重新 bind）
    if (!ttsProgressUnlisten && API.onTtsProgress) {
      Promise.resolve(API.onTtsProgress((p) => {
        if (!p) return;
        ttsShowProgress(p);
        if (p.stage === 'done' || p.stage === 'failed') setTimeout(ttsHideProgress, 1600);
      })).then((un) => { ttsProgressUnlisten = un; }).catch(() => { /* 订阅失败不影响手动下载 */ });
    }

    // 「当前语音模型」跟着实际发声实时更新 —— 用户点一次朗读就能看到
    // 这次到底是谁念的（本地 Piper / 系统语音 / 词典录音），
    // 不用去猜「我装的语音包生效了没」。同样只订一次。
    if (!ttsVoiceUnsub && window.Speak && window.Speak.onVoice) {
      ttsVoiceUnsub = window.Speak.onVoice(() => renderTtsFooter());
    }

    // 语音健康：链路任何失败的最终原因常驻在这里（toast 太快看不清）。
    // 一切正常 → 显示中性小字；出错 → 红字 + 可行动的办法，不再自动消失。
    if (!ttsHealthUnsub && window.Speak && window.Speak.onHealth) {
      ttsHealthUnsub = window.Speak.onHealth((h) => {
        const el = ttsEl('tts-health');
        if (!el) return;
        if (h && h.state === 'error') {
          el.textContent = h.message;
          el.classList.add('error');
        } else {
          el.classList.remove('error');
          el.textContent = '朗读链路正常';
        }
      });
    }
  }

  /* ---- 设置页分组手风琴（重构） ----
     面板太多太杂（原话「设置内容过多且杂乱」）。按功能归成六个组，
     每组一个手风琴：组头点击收起/展开，组内面板弱化成带分隔线的列表；
     「数据与网络」这类高级组默认收起，常用组全部展开。分组靠 DOM 重组
     实现（HTML 不动，守卫与既有 id 查找不受影响），手动选择记
     localStorage 跨启动保留。 */
  const LS_GROUP_FOLD = 'ww.settings.groupFold';

  const SETTINGS_GROUPS = [
    { title: '通用', open: true, panels: ['外观与主题', '语言'] },
    { title: '学习与朗读', open: true, panels: ['记忆辅助内容', '朗读与发音'] },
    { title: 'AI 与模型', open: true, panels: ['AI 服务（本地模型 / 在线 API）', '本地大模型一键部署', '演示模式'] },
    { title: '搜索与词典', open: true, panels: ['语言与搜索', '词典与翻译源（可扩展小语种）'] },
    { title: '数据与网络', open: false, panels: ['网络与代理', '数据库', '数据与模型的存放位置'] },
    { title: '关于', open: true, panels: ['日志诊断', '关于与更新'] },
  ];

  /* ---- 日志诊断：自动分析 + 清晰呈现 ----
     后端读环形缓冲（最近 2000 条），按规则表归类错误/警告并给建议；
     同一问题刷屏会合并成一条带计数。这里只负责把结果画清楚。 */
  let logSeq = 0;

  function esc50(v, n) {
    return U().esc(String(v || '').slice(0, n || 120));
  }

  async function renderLogAnalysis() {
    const sum = ttsEl('log-summary');
    const box = ttsEl('log-items');
    if (!sum || !box) return;
    const my = ++logSeq;
    sum.innerHTML = U().loadingHtml('正在分析运行日志…');
    box.innerHTML = '';
    let r = null;
    try { r = await API.logAnalysis(); } catch (e) { /* 后端不可用就不展示 */ }
    if (my !== logSeq || !r) return;

    if (!r.findings || !r.findings.length) {
      sum.innerHTML = '<span class="tag green">日志健康</span>' +
        `<span class="muted" style="margin-left:8px">最近 ${r.total_lines || 0} 条日志中没有错误或警告。</span>`;
      return;
    }
    sum.innerHTML = `<span class="tag ${r.errors ? 'orange' : 'green'}">${r.errors} 个错误</span>` +
      ` <span class="tag ${r.warns ? 'orange' : ''}">${r.warns} 个警告</span>` +
      `<span class="muted" style="margin-left:8px">来自最近 ${r.total_lines || 0} 条日志，同类问题已合并。</span>`;

    box.innerHTML = r.findings.map(f => `
      <div class="log-finding">
        <div class="lf-head">
          <span class="dot ${f.level === 'error' ? 'offline' : 'checking'}"></span>
          <b class="lf-cat">${esc50(f.category, 20)}</b>
          ${f.count > 1 ? `<span class="tag">${f.count} 次</span>` : ''}
          <span class="lf-time muted">${esc50(f.last_seen, 19)}</span>
        </div>
        <div class="lf-msg">${esc50(f.message, 200)}</div>
        <div class="lf-sug"><i>建议</i>${esc50(f.suggestion, 200)}</div>
      </div>`).join('');
  }

  function setupPanelFolds() {
    const page = document.getElementById('page-settings');
    if (!page || page.__groupsBound) return;
    // 面板不是 page 的直接子元素，而是包在 .two-col.settings-cols 网格里。
    // （2026-10-07 事故：当初从 page 直接找 .panel，一个都匹配不到，随后
    //  把 page 的 innerHTML 整个清空——「折叠了之后什么都没了」。）
    // 现在只替换 cols 这个容器；找不到就原样退出，宁可保持旧版式也不能清页。
    const cols = page.querySelector('.settings-cols');
    if (!cols) return;
    page.__groupsBound = true;
    let folded = {};
    try { folded = JSON.parse(localStorage.getItem(LS_GROUP_FOLD) || '{}'); } catch (e) {}

    // 收集面板（cols 的直接子面板；朗读面板的标题在 .tts-topbar 里）
    const byTitle = new Map();
    cols.querySelectorAll(':scope > .panel').forEach((p) => {
      const t = p.querySelector('.panel-title');
      if (t) byTitle.set((t.textContent || '').trim(), p);
    });

    const makeGroup = (title, open) => {
      const group = document.createElement('div');
      group.className = 'settings-group' + (open ? '' : ' collapsed');
      const head = document.createElement('div');
      head.className = 'settings-group-head';
      head.setAttribute('role', 'button');
      head.setAttribute('tabindex', '0');
      head.setAttribute('aria-expanded', open ? 'true' : 'false');
      const tt = document.createElement('span');
      tt.className = 'sg-title';
      tt.textContent = title;
      const caret = document.createElement('span');
      caret.className = 'sg-caret';
      caret.setAttribute('aria-hidden', 'true');
      caret.textContent = open ? '▾' : '▸';
      head.appendChild(tt);
      head.appendChild(caret);
      const body = document.createElement('div');
      body.className = 'settings-group-body';
      group.appendChild(head);
      group.appendChild(body);
      const toggle = () => {
        const willFold = !group.classList.contains('collapsed');
        group.classList.toggle('collapsed', willFold);
        caret.textContent = willFold ? '▸' : '▾';
        head.setAttribute('aria-expanded', willFold ? 'false' : 'true');
        folded[title] = willFold;
        try { localStorage.setItem(LS_GROUP_FOLD, JSON.stringify(folded)); } catch (e) { /* 忽略 */ }
      };
      head.addEventListener('click', toggle);
      head.addEventListener('keydown', (e) => {
        if (e.key === 'Enter' || e.key === ' ') { e.preventDefault(); toggle(); }
      });
      const initFold = Object.prototype.hasOwnProperty.call(folded, title)
        ? !!folded[title]
        : !open;
      if (initFold) {
        group.classList.add('collapsed');
        caret.textContent = '▸';
        head.setAttribute('aria-expanded', 'false');
      }
      return { group, head, caret, body };
    };

    // 逐组建手风琴并搬入面板（appendChild 自带「从原位搬走」语义）
    const wrap = document.createElement('div');
    wrap.className = 'settings-groups';
    const claimed = new Set();
    SETTINGS_GROUPS.forEach((g) => {
      const parts = makeGroup(g.title, g.open);
      g.panels.forEach((name) => {
        const p = byTitle.get(name);
        if (p && !claimed.has(p)) { parts.body.appendChild(p); claimed.add(p); }
      });
      wrap.appendChild(parts.group);
    });

    // 没进任何分组的面板收进「其他」——宁可多一组，也绝不能把设置面板弄丢
    const rest = [];
    cols.querySelectorAll(':scope > .panel').forEach((p) => {
      if (!claimed.has(p)) rest.push(p);
    });
    if (rest.length) {
      const parts = makeGroup('其他', true);
      rest.forEach((p) => parts.body.appendChild(p));
      wrap.appendChild(parts.group);
    }

    // 只替换面板容器；page-head（标题与保存按钮）原样保留
    cols.parentNode.replaceChild(wrap, cols);
  }

  function bind() {
    setupPanelFolds();
    ttsEl('btn-log-analyze')?.addEventListener('click', renderLogAnalysis);
    document.getElementById('btn-save-settings')?.addEventListener('click', save);
    document.getElementById('btn-add-source')?.addEventListener('click', addSource);
    document.getElementById('btn-reset-sources')?.addEventListener('click', resetSources);
    document.getElementById('btn-llm-test')?.addEventListener('click', testLlm);
    document.getElementById('set-llm-preset')?.addEventListener('change', (e) => applyPreset(e.target.value));
    // 手填地址时让下拉跟着走，免得「下拉显示 DeepSeek、地址却是别家」
    document.getElementById('set-llm-url')?.addEventListener('input', syncPresetUI);
    document.getElementById('btn-llm-key-eye')?.addEventListener('click', toggleKeyVisible);
    document.getElementById('btn-llm-getkey')?.addEventListener('click', async (e) => {
      const url = e.currentTarget.dataset.url;
      if (!url) return;
      try { await API.openUrl(url); } catch (err) { U().toast(err.message, 'err'); }
    });
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

    // 界面语言：改动立刻换 UI，不等「保存」。
    // 故意不落 save —— 用户想看的是「切过去长什么样」，让他多点一次保存
    // 才能看到效果，在这个选项上没有意义。真正写盘由 save() 负责。
    document.getElementById('set-ui-lang')?.addEventListener('change', (e) => {
      const code = e.target.value || 'zh-CN';
      if (config) config.ui_lang = code;
      window.I18n?.setLang(code);
    });

    bindSpeak();
    bindTts();
  }

  return {
    bind, load, save,
    renderSpeakVoices, loadSpeak, loadTts, renderTtsFooter,
    // 折叠状态写入函数也导出：冒烟测试要能直接断言「开合后 body 用的是
    // `.hidden` class 而不是 `hidden` 属性」，而不是去猜 DOM 长什么样
    applyLangOpen,
    setupPanelFolds,
    // 预设表与识别函数也导出：冒烟测试要能直接断言「选某家带出什么地址」，
    // 而不是靠解析 DOM 里的 option 文本去猜
    presets: AI_PRESETS,
    detectPreset,
    applyPreset,
    syncPresetUI,
    toggleKeyVisible,
    get config() { return config; },
  };
})();

window.Settings = Settings;
