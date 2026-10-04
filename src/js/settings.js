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

    // 语言与侧边栏
    setVal('set-lang', config.target_lang);
    setVal('set-sb-width', config.sidebar_width);
    const top = document.getElementById('set-sb-top');
    if (top) top.checked = config.sidebar_always_on_top !== false;

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
    config.sidebar_width = getNum('set-sb-width', 380);
    const top = document.getElementById('set-sb-top');
    if (top) config.sidebar_always_on_top = top.checked;

    try {
      await API.saveConfig(config);
      await API.saveSources(sources);
      U().toast('设置已保存', 'ok');
      // 同步背诵页的题量
      const b = document.getElementById('opt-batch');
      if (b) b.value = config.study.batch_size;
      if (window.App) window.App.refreshConfig();
    } catch (e) {
      U().toast('保存失败：' + e.message, 'err');
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
          <label class="switch" style="padding:0">
            <input type="checkbox" data-f="enabled" ${s.enabled ? 'checked' : ''} />
          </label>
          <input class="source-name" data-f="name" value="${U().esc(s.name)}" style="flex:1;border:1px solid transparent;background:transparent;font-weight:600;font-size:13.5px" />
          ${s.builtin ? '<span class="tag blue">内置</span>' : '<span class="tag">自定义</span>'}
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
