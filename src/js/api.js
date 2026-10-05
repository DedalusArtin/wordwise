/* ============================================================
   api.js —— 前后端桥接层
   统一封装 Tauri 命令调用、事件订阅与错误提示。
   浏览器直接打开（无 Tauri）时进入 Mock 模式，方便单独调试界面。
   ============================================================ */

const HAS_TAURI = typeof window.__TAURI__ !== 'undefined' || typeof window.__TAURI_INTERNALS__ !== 'undefined';

/** 调用后端命令。自动把后端的中文错误抛出为 Error。 */
async function invoke(cmd, args = {}) {
  if (!HAS_TAURI) {
    return Mock.call(cmd, args);
  }
  // Tauri v2: withGlobalTauri 下为 __TAURI__.core.invoke
  const t = window.__TAURI__;
  const fn = (t && t.core && t.core.invoke)
    || (t && t.invoke)
    || (window.__TAURI_INTERNALS__ && window.__TAURI_INTERNALS__.invoke);
  if (!fn) throw new Error('Tauri API 不可用，请通过桌面程序启动');
  try {
    return await fn(cmd, args);
  } catch (e) {
    // 后端返回的字符串错误
    const msg = typeof e === 'string' ? e : (e && e.message) || String(e);
    throw new Error(msg);
  }
}

/** 监听后端事件（AI 流式讲解等）。返回取消函数。 */
async function listen(event, handler) {
  if (!HAS_TAURI) return () => {};
  const t = window.__TAURI__;
  const fn = (t && t.event && t.event.listen)
    || (window.__TAURI_INTERNALS__ && window.__TAURI_INTERNALS__.listen);
  if (!fn) return () => {};
  const un = await fn(event, (e) => handler(e.payload));
  return typeof un === 'function' ? un : () => {};
}

/* ---------------- 业务 API ---------------- */

const API = {
  // 系统
  appInfo: () => invoke('cmd_app_info'),
  getConfig: () => invoke('cmd_get_config'),
  saveConfig: (config) => invoke('cmd_save_config', { config }),
  setStudyOptions: (options) => invoke('cmd_set_study_options', { options }),
  setLlmConfig: (llm) => invoke('cmd_set_llm_config', { llm }),

  // 本地模型
  llmStatus: () => invoke('cmd_llm_status'),
  llmAutoconnect: () => invoke('cmd_llm_autoconnect'),
  // ★ 返回 ExplainResult { word, text, original, lang, translated, note }
  //   而不是字符串：text 是最终展示文本，original 是模型原文（供「查看原文」对照）。
  aiExplain: (word, lang, question) =>
    invoke('cmd_ai_explain', { word, lang: lang || null, question: question || null }),
  aiExplainSync: (word, lang) => invoke('cmd_ai_explain_sync', { word, lang: lang || null }),
  aiGenerateEntry: (word, lang) =>
    invoke('cmd_ai_generate_entry', { word, lang: lang || null }),
  // AI 讲解语言（切换即时生效并落盘 / 对已有原文重译）
  setExplainLang: (lang) => invoke('cmd_set_explain_lang', { lang }),
  translateText: (text, lang) => invoke('cmd_translate_text', { text, lang: lang || null }),

  // 查词搜索
  lookup: (word, lang, forceRefresh) =>
    invoke('cmd_lookup', { word, lang: lang || null, forceRefresh: !!forceRefresh }),
  suggest: (query, lang) => invoke('cmd_suggest', { query, lang: lang || null }),
  search: (query, lang, withWiki) =>
    invoke('cmd_search', { query, lang: lang || null, withWiki: withWiki !== false }),
  wiki: (word, lang) => invoke('cmd_wiki', { word, lang: lang || null }),
  dictLinks: (word, lang) => invoke('cmd_dict_links', { word, lang: lang || null }),
  openUrl: (url) => invoke('cmd_open_url', { url }),
  getWord: (word, lang) => invoke('cmd_get_word', { word, lang: lang || null }),
  searchWords: (query, lang, limit) =>
    invoke('cmd_search_words', { query, lang: lang || null, limit: limit || 50 }),
  recentSearches: () => invoke('cmd_recent_searches'),

  // 词库
  listWords: (lang, limit, offset) =>
    invoke('cmd_list_words', { lang: lang || null, limit: limit || 100, offset: offset || 0 }),
  addWord: (entry) => invoke('cmd_add_word', { entry }),
  importWords: (words, lang, prefetch) =>
    invoke('cmd_import_words', { words, lang: lang || null, prefetch: prefetch !== false }),
  deleteWord: (word, lang) => invoke('cmd_delete_word', { word, lang: lang || null }),
  wordCount: (lang) => invoke('cmd_word_count', { lang: lang || null }),
  seedDemo: (lang) => invoke('cmd_seed_demo', { lang: lang || null }),

  // 多级词库管理（需求 3 / 5）
  listWordbooks: (lang) => invoke('cmd_list_wordbooks', { lang: lang || null }),
  getWordbook: (id) => invoke('cmd_get_wordbook', { id }),
  createWordbook: (name, category, parentId, description, sourceUrl, license, lang) =>
    invoke('cmd_create_wordbook', {
      name,
      category: category || null,
      parentId: parentId || null,
      description: description || null,
      sourceUrl: sourceUrl || null,
      license: license || null,
      lang: lang || null,
    }),
  deleteWordbook: (id) => invoke('cmd_delete_wordbook', { id }),
  importWordsToBook: (content, bookId, bookName, formatHint, source, lang) =>
    invoke('cmd_import_words_to_book', {
      content,
      bookId: bookId || null,
      bookName: bookName || null,
      formatHint: formatHint || null,
      source: source || null,
      lang: lang || null,
    }),
  remoteCatalog: (category) => invoke('cmd_remote_catalog', { category: category || null }),
  downloadBook: (id, lang) => invoke('cmd_download_book', { id, lang: lang || null }),
  reviewedWords: (lang, limit, offset) =>
    invoke('cmd_reviewed_words', { lang: lang || null, limit: limit || 200, offset: offset || 0 }),
  wordsInBook: (bookId, lang, limit, offset) =>
    invoke('cmd_words_in_book', {
      bookId,
      lang: lang || null,
      limit: limit || 200,
      offset: offset || 0,
    }),

  // 在线搜索 / 语言方向 / 进阶练习（需求 4 / 6 / 7）
  searchEngines: () => invoke('cmd_search_engines'),
  webSearch: (query, engine, limit, both) =>
    invoke('cmd_web_search', {
      query,
      engine: engine || null,
      limit: limit || 8,
      both: !!both,
    }),
  lookupLinks: (word, lang) => invoke('cmd_lookup_links', { word, lang: lang || null }),

  // 网络与代理诊断
  networkInfo: () => invoke('cmd_network_info'),
  networkReport: () => invoke('cmd_network_report'),
  reloadNetwork: () => invoke('cmd_reload_network'),

  setDirection: (sourceLang, targetLang, searchEngine) =>
    invoke('cmd_set_direction', {
      sourceLang: sourceLang === undefined ? null : sourceLang,
      targetLang: targetLang === undefined ? null : targetLang,
      searchEngine: searchEngine === undefined ? null : searchEngine,
    }),
  getDirection: () => invoke('cmd_get_direction'),
  exampleCoverage: (lang) => invoke('cmd_example_coverage', { lang: lang || null }),
  quizModes: () => invoke('cmd_quiz_modes'),
  startBookSession: (bookId, mode, size, lang) =>
    invoke('cmd_start_book_session', {
      bookId: bookId || null,
      mode: mode || null,
      size: size || null,
      lang: lang || null,
    }),
  checkSpelling: (input, answer, strict) =>
    invoke('cmd_check_spelling', { input, answer, strict: !!strict }),
  spellHint: (word, reveal) => invoke('cmd_spell_hint', { word, reveal: reveal || 0 }),
  maskExample: (sentence, word) => invoke('cmd_mask_example', { sentence, word }),
  buildAdvancedCard: (mode, lang) =>
    invoke('cmd_build_advanced_card', { mode, lang: lang || null }),

  // 背诵
  startSession: (mode, size, leechOnly, lang) =>
    invoke('cmd_start_session', {
      mode: mode || null,
      size: size || null,
      leechOnly: !!leechOnly,
      lang: lang || null,
    }),
  currentQuestion: (lang) => invoke('cmd_current_question', { lang: lang || null }),
  submitAnswer: (word, grade, elapsedMs, lang) =>
    invoke('cmd_submit_answer', {
      word: word || null,
      grade,
      elapsedMs: elapsedMs || 0,
      lang: lang || null,
    }),
  skip: () => invoke('cmd_skip'),
  endSession: () => invoke('cmd_end_session'),
  wordState: (word, lang) => invoke('cmd_word_state', { word, lang: lang || null }),

  // 统计
  stats: (lang) => invoke('cmd_stats', { lang: lang || null }),
  reviewPlan: (days, lang) => invoke('cmd_review_plan', { days: days || 14, lang: lang || null }),
  leechList: (limit, lang) => invoke('cmd_leech_list', { limit: limit || 100, lang: lang || null }),
  clearLeech: (word, lang) => invoke('cmd_clear_leech', { word, lang: lang || null }),

  // 词典源
  getSources: () => invoke('cmd_get_sources'),
  saveSources: (sources) => invoke('cmd_save_sources', { sources }),
  testSource: (source, word) => invoke('cmd_test_source', { source, word: word || null }),
  resetSources: () => invoke('cmd_reset_sources'),
  clearCache: () => invoke('cmd_clear_cache'),

  // 数据
  exportData: (path) => invoke('cmd_export', { path: path || null }),
  importData: (path) => invoke('cmd_import', { path }),

  // 窗口
  sidebarShow: () => invoke('sidebar_show'),
  sidebarHide: () => invoke('sidebar_hide'),
  sidebarToggle: () => invoke('sidebar_toggle'),
  mainShow: () => invoke('main_show'),
};

/* ============================================================
   Mock —— 浏览器调试模式
   用内存数据模拟后端，便于脱离 Tauri 调 UI
   ============================================================ */
const Mock = (() => {
  const store = {
    words: [],
    states: {},
    session: null,
    config: {
      llm: { base_url: 'http://127.0.0.1:1234/v1', model: '', temperature: 0.6, max_tokens: 1024,
             timeout_secs: 120, api_key: 'lm-studio', system_prompt: '' },
      study: {
        show_inflections: true, show_examples: true, show_related: false,
        show_mnemonic: true, show_phonetic: true, auto_popup_on_wrong: true,
        ai_explain: true, batch_size: 20, daily_limit: 120,
      },
      target_lang: 'en', ui_lang: 'zh-CN', sidebar_always_on_top: true, sidebar_width: 380,
      explain_lang: 'zh', explain_auto_translate: true, explain_translate_template: '',
      srs: { base_intervals: [1,2,4,7,15,30,90,180] },
      dict_sources: [],
    },
  };

  const demo = [
    ['abandon', '/əˈbæn.dən/', 'v. 放弃；抛弃', 'He abandoned his car.'],
    ['ability', '/əˈbɪl.ə.ti/', 'n. 能力；才能', 'She has the ability.'],
    ['accurate', '/ˈæk.jə.rət/', 'adj. 准确的；精确的', 'The report is accurate.'],
    ['benefit', '/ˈben.ɪ.fɪt/', 'n. 好处；益处', 'Exercise has benefits.'],
    ['consider', '/kənˈsɪd.ər/', 'v. 考虑；认为', 'Please consider it.'],
    ['determine', '/dɪˈtɜː.mɪn/', 'v. 决定；确定', 'She determined to go.'],
    ['efficient', '/ɪˈfɪʃ.ənt/', 'adj. 高效的', 'A more efficient way.'],
    ['generate', '/ˈdʒen.ə.reɪt/', 'v. 产生；生成', 'Panels generate power.'],
    ['hesitate', '/ˈhez.ɪ.teɪt/', 'v. 犹豫；踌躇', "Don't hesitate to ask."],
    ['improve', '/ɪmˈpruːv/', 'v. 改进；提高', 'Practice improves skill.'],
  ];

  function entry(word, phon, def, ex) {
    return {
      word, lang: 'en',
      phonetic: { uk: phon, us: phon, audio: '' },
      senses: [{ pos: def.split(' ')[0], definition: def.replace(/^\S+\s*/, ''),
                 examples: ex ? [{ text: ex, translation: '（示例译文）' }] : [] }],
      inflections: [{ label: '过去式', form: word + 'ed' }],
      related: ['synonym1', 'synonym2'],
      mnemonic: '（调试模式）词根记忆法示例',
      source: 'mock', extra: {},
    };
  }

  function init() {
    if (store.words.length) return;
    store.words = demo.map(d => entry(d[0], d[1], d[2], d[3]));
  }

  return {
    async call(cmd, args) {
      init();
      await new Promise(r => setTimeout(r, 60));

      switch (cmd) {
        case 'cmd_app_info':
          return { version: '1.0.0', name: 'WordWise', now: Math.floor(Date.now()/1000),
                   now_text: new Date().toLocaleString('zh-CN'), today: new Date().toISOString().slice(0,10),
                   greeting: '你好', data_dir: '(浏览器调试)', db_path: '(内存)' };
        case 'cmd_get_config': return store.config;
        case 'cmd_save_config': store.config = args.config; return null;
        case 'cmd_set_study_options': store.config.study = args.options; return store.config;
        case 'cmd_set_llm_config': store.config.llm = args.llm; return store.config;
        case 'cmd_llm_status':
          return { online: false, base_url: store.config.llm.base_url, models: [], active_model: '',
                   message: '浏览器调试模式：未连接本地模型服务' };
        case 'cmd_llm_autoconnect': return Mock.call('cmd_llm_status', args);
        case 'cmd_lookup':
        case 'cmd_get_word': {
          const w = store.words.find(x => x.word === (args.word || '').toLowerCase());
          if (cmd === 'cmd_get_word') return w || null;
          if (!w) throw new Error('（调试模式）未收录该词：' + args.word);
          return { word: w.word, lang: 'en', entry: w, sources: ['调试数据'],
                   from_cache: false, from_llm: false, trace: [] };
        }
        case 'cmd_suggest': return [];
        case 'cmd_search': return { query: args.query, suggestions: [], wiki: null };
        case 'cmd_wiki': return null;
        case 'cmd_dict_links': {
          const e = encodeURIComponent(args.word || '');
          return [
            { name: '有道词典', url: `https://dict.youdao.com/result?word=${e}&lang=en`, note: '中文释义·例句·发音', cn_friendly: true },
            { name: '剑桥词典', url: `https://dictionary.cambridge.org/dictionary/english-chinese-simplified/${e}`, note: '英汉双解·权威', cn_friendly: true },
            { name: '牛津学习词典', url: `https://www.oxfordlearnersdictionaries.com/definition/english/${e}`, note: '牛津·释义与搭配', cn_friendly: false },
          ];
        }
        case 'cmd_open_url': return null;
        // 网络与代理：调试模式一律报「直连」，与真实默认配置一致
        case 'cmd_network_info':
        case 'cmd_reload_network':
          return { url: null, origin: '未启用代理（直连）· 浏览器调试模式', explicit: true };
        case 'cmd_network_report':
          return {
            proxy: '未启用代理（直连）· 浏览器调试模式',
            proxy_url: null,
            using_proxy: false,
            proxy_origin: '未启用代理（直连）',
            items: [{
              name: '（调试模式）',
              url: '',
              ok: true,
              elapsed_ms: 0,
              detail: '浏览器调试模式下不发起真实网络请求',
            }],
          };
        case 'cmd_search_words':
          return store.words.filter(w => w.word.includes((args.query||'').toLowerCase()))
            .map(w => ({ word: w.word, lang: 'en', entry: w, added_at: 0 }));
        case 'cmd_recent_searches': return [];
        case 'cmd_list_words':
          return store.words.map(w => ({ word: w.word, lang: 'en', entry: w, added_at: 0 }));
        case 'cmd_add_word': store.words.push(args.entry); return null;
        case 'cmd_import_words': return { added: (args.words||[]).length, skipped: 0, prefetched: 0 };
        case 'cmd_delete_word':
          store.words = store.words.filter(w => w.word !== args.word); return null;
        case 'cmd_word_count': return store.words.length;
        case 'cmd_seed_demo': init(); return store.words.length;
        case 'cmd_start_session': {
          store.session = { idx: 0, queue: store.words.slice(), correct: 0, wrong: 0,
                            mode: args.mode || 'en_to_zh' };
          return { mode: store.session.mode, total: store.session.queue.length, index: 0,
                   correct: 0, wrong: 0, leech_only: false };
        }
        case 'cmd_current_question': {
          const s = store.session;
          if (!s || s.idx >= s.queue.length) return null;
          const e = s.queue[s.idx];
          const en = s.mode === 'en_to_zh';
          const others = store.words.filter(x => x.word !== e.word).slice(0, 3)
            .map(x => en ? x.senses[0].definition : x.word);
          return {
            prompt: en ? e.word : e.senses[0].definition,
            answer: en ? e.senses[0].definition : e.word,
            entry: e, options: others, mode: s.mode, is_leech: false,
            index: s.idx + 1, total: s.queue.length,
            correct_count: s.correct, wrong_count: s.wrong,
          };
        }
        case 'cmd_submit_answer': {
          const s = store.session;
          const e = store.words.find(x => x.word === args.word) || store.words[0];
          if (s) { if (args.grade === 'wrong') s.wrong++; else s.correct++; s.idx++; }
          return {
            word: args.word, grade: args.grade, correct: args.grade !== 'wrong', entry: e,
            schedule: { state: {}, interval_days: 1, due_at: 0, due_text: '明天', became_leech: false, became_mastered: false },
            session_finished: s ? s.idx >= s.queue.length : true,
            total: s ? s.queue.length : 0,
            correct_count: s ? s.correct : 0, wrong_count: s ? s.wrong : 0, retention: 0.6,
          };
        }
        case 'cmd_skip': {
          if (store.session) store.session.idx++;
          return { mode: 'en_to_zh', total: store.session.queue.length, index: store.session.idx,
                   correct: store.session.correct, wrong: store.session.wrong, leech_only: false };
        }
        case 'cmd_end_session': store.session = null;
          return { mode: 'en_to_zh', total: 0, index: 0, correct: 0, wrong: 0, leech_only: false };
        case 'cmd_word_state': return null;
        case 'cmd_stats':
          return { total_words: store.words.length, learned: 5, mastered: 2, leeches: 1, due_today: 3,
                   reviewed_today: 8, correct_today: 6, wrong_today: 2, streak_days: 3,
                   history: Array.from({length:14}, (_,i) => ({
                     date: new Date(Date.now() - (13-i)*86400000).toISOString().slice(0,10),
                     count: Math.floor(Math.random()*20), correct: Math.floor(Math.random()*15) })) };
        case 'cmd_review_plan':
          return Array.from({length: args.days||14}, (_,i) => ({
            date: new Date(Date.now() + i*86400000).toISOString().slice(0,10),
            weekday: '周' + '日一二三四五六'[new Date(Date.now()+i*86400000).getDay()],
            count: Math.floor(Math.random()*25), overdue: i===0?2:0, is_today: i===0 }));
        case 'cmd_leech_list': return [];
        case 'cmd_clear_leech': return null;
        case 'cmd_get_sources': return store.config.dict_sources;
        case 'cmd_save_sources': store.config.dict_sources = args.sources; return null;
        case 'cmd_test_source':
          return { ok: false, message: '（调试模式）不发起真实请求', sample: null, elapsed_ms: 0 };
        case 'cmd_reset_sources': return [];
        case 'cmd_clear_cache': return null;
        case 'cmd_export': return '(调试模式) 未实际导出';
        case 'cmd_import': return { added: 0, skipped: 0, prefetched: 0 };
        case 'cmd_ai_explain':
        case 'cmd_ai_explain_sync': {
          const t = '## 调试模式\n\n当前运行在浏览器调试环境中，未连接本地大模型服务。\n\n- 请通过 Tauri 桌面程序启动以使用 AI 讲解';
          return { word: args.word || '', text: t, original: t,
                   lang: (store.config.explain_lang || 'zh'), translated: false };
        }
        // 切换讲解语言：立刻记住，下次讲解生效
        case 'cmd_set_explain_lang':
          store.config.explain_lang = args.lang; return args.lang;
        case 'cmd_translate_text':
          return { text: args.text || '', original: args.text || '',
                   lang: args.lang || store.config.explain_lang || 'zh', translated: false,
                   note: '（调试模式）未连接模型，未实际翻译' };
        case 'cmd_ai_generate_entry': throw new Error('（调试模式）无法生成词条');

        // 多级词库（需求 3 / 5）
        case 'cmd_list_wordbooks':
          return [
            { id: 'root', name: '全部词库', category: 'root', level: 0, parent_id: '',
              lang: 'en', description: '所有已安装词库的汇总视图', source_url: '', license: '',
              word_count: store.words.length, builtin: true, installed: true, ord: 0, created_at: 0,
              learned: 0, mastered: 0, due_today: 0 },
            { id: 'exam-en', name: '英语考试', category: 'exam', level: 1, parent_id: 'root',
              lang: 'en', description: '四六级、考研等', source_url: '', license: '',
              word_count: 0, builtin: true, installed: true, ord: 1, created_at: 0,
              learned: 0, mastered: 0, due_today: 0 },
            { id: 'cet4-core', name: '四级核心词汇（示例）', category: 'cet4', level: 2,
              parent_id: 'exam-en', lang: 'en', description: '演示数据',
              source_url: 'https://github.com/mahavivo/english-wordlists', license: 'MIT',
              word_count: store.words.length, builtin: false, installed: true, ord: 2, created_at: 0,
              learned: 3, mastered: 1, due_today: 2 },
          ];
        case 'cmd_get_wordbook':
          return (Mock.call('cmd_list_wordbooks', {}) || []).find(b => b.id === args.id) || null;
        case 'cmd_create_wordbook':
          return { id: 'user-1', name: args.name, category: args.category || 'other', level: 2,
                   parent_id: args.parentId || 'root', lang: 'en', description: args.description || '',
                   source_url: args.sourceUrl || '', license: args.license || '', word_count: 0,
                   builtin: false, installed: true, ord: 2, created_at: 0 };
        case 'cmd_delete_wordbook': return null;
        case 'cmd_import_words_to_book': {
          const n = (args.content || '').split('\n').filter(Boolean).length;
          return { book_id: args.bookId || 'user-1', total: n, imported: n, skipped: 0, failed: 0,
                   message: `（调试模式）已解析 ${n} 行` };
        }
        case 'cmd_remote_catalog':
          // 调试模式下也要带 `lang`：前端按它显示语言标签、后端按它决定入库语言
          return [
            { id: 'cet4-core', name: '四级核心词汇', category: 'cet4', description: '大学英语四级高频词汇',
              lang: 'en', mirrors: [],
              url: 'https://raw.githubusercontent.com/mahavivo/english-wordlists/master/CET4_edited.txt',
              format: 'txt', approx_words: 2600,
              source_url: 'https://github.com/mahavivo/english-wordlists', license: 'MIT', installed: false },
            { id: 'kaoyan-core', name: '考研核心词汇', category: 'kaoyan', description: '考研英语高频词汇',
              lang: 'en', mirrors: [],
              url: 'https://raw.githubusercontent.com/mahavivo/english-wordlists/master/KAOYAN_edited.txt',
              format: 'txt', approx_words: 4500,
              source_url: 'https://github.com/mahavivo/english-wordlists', license: 'MIT', installed: false },
            { id: 'ielts-core', name: '雅思核心词汇', category: 'ielts', description: 'IELTS 高频词汇',
              lang: 'en', mirrors: [], url: '', format: 'txt', approx_words: 3400,
              source_url: 'https://github.com/KyleBing/english-vocabulary', license: '见源仓库', installed: false },
            { id: 'jlpt-n5', name: 'JLPT N5 词汇', category: 'jlpt', description: '日语能力考试 N5 词汇',
              lang: 'ja', mirrors: [], url: '', format: 'json', approx_words: 675,
              source_url: 'https://github.com/evanclan/OpenJLPT', license: 'CC-BY-SA-4.0', installed: false },
            { id: 'moji-jp', name: 'MOJI 日语常用词', category: 'other', description: '日语常用词表',
              lang: 'ja', mirrors: [],
              url: 'https://github.com/NoHeartPen/mojidict-anki', format: 'txt', approx_words: 10000,
              source_url: 'https://github.com/NoHeartPen/mojidict-anki', license: '见源仓库', installed: false },
          ];
        case 'cmd_download_book':
          return { book_id: args.id, total: 2600, imported: 2600, skipped: 0, failed: 0,
                   message: '（调试模式）模拟下载完成' };
        case 'cmd_reviewed_words':
        case 'cmd_words_in_book':
          return store.words.map(w => ({ word: w.word, lang: 'en', entry: w, added_at: 0 }));

        // 在线搜索 / 方向 / 进阶练习（需求 4 / 6 / 7）
        case 'cmd_search_engines':
          return [
            { id: 'bing', label: '必应', cn_friendly: true },
            { id: 'baidu', label: '百度', cn_friendly: true },
            { id: 'bingintl', label: '必应国际', cn_friendly: false },
          ];
        case 'cmd_web_search':
          return [
            { title: `${args.query} - 百度百科`, url: 'https://baike.baidu.com/item/x',
              snippet: '（调试模式）示例结果', engine: '百度' },
            { title: `${args.query} 释义 - 必应`, url: 'https://cn.bing.com/x',
              snippet: '（调试模式）示例结果', engine: '必应' },
          ];
        case 'cmd_lookup_links':
          return Mock.call('cmd_dict_links', args);
        case 'cmd_set_direction':
          if (args.sourceLang !== null && args.sourceLang !== undefined) store.config.source_lang = args.sourceLang;
          if (args.targetLang) store.config.target_lang = args.targetLang;
          if (args.searchEngine) store.config.search_engine = args.searchEngine;
          return null;
        case 'cmd_get_direction':
          return { source_lang: store.config.source_lang || '', target_lang: store.config.target_lang || 'en',
                   search_engine: store.config.search_engine || 'bing' };
        case 'cmd_example_coverage': return Math.floor(store.words.length * 0.6);
        case 'cmd_quiz_modes':
          return [
            { id: 'EnToZh', label: '英→中', desc: '看英文选中文释义', typing: false, example: false, audio: false },
            { id: 'ZhToEn', label: '中→英', desc: '看中文选英文单词', typing: false, example: false, audio: false },
            { id: 'Spelling', label: '拼写练习', desc: '看释义拼单词', typing: true, example: false, audio: false },
            { id: 'ExToZh', label: '例句选义', desc: '看例句选释义', typing: false, example: true, audio: false },
            { id: 'ExPickWord', label: '例句识词', desc: '例句中识别单词', typing: false, example: true, audio: false },
            { id: 'ListenSpell', label: '听音拼写', desc: '听发音拼词', typing: true, example: false, audio: true },
          ];
        case 'cmd_start_book_session': {
          store.session = { idx: 0, queue: store.words.slice(), correct: 0, wrong: 0,
                            mode: args.mode || 'en_to_zh' };
          return { mode: store.session.mode, total: store.session.queue.length, index: 0,
                   correct: 0, wrong: 0, leech_only: false };
        }
        case 'cmd_check_spelling': {
          const u = (args.input || '').trim().toLowerCase();
          const a = (args.answer || '').trim().toLowerCase();
          let p = 0;
          for (let i = 0; i < Math.min(u.length, a.length); i++) {
            if (u[i] === a[i]) p++; else break;
          }
          return { correct: !!u && u === a, user_input: u, answer: a, matched_prefix: p };
        }
        case 'cmd_spell_hint': {
          const r = args.reveal || 0;
          const cs = String(args.word || '').split('');
          return cs.map((c, i) => (r > 0 && (i < r || i + r >= cs.length)) ? c : '_').join(' ');
        }
        case 'cmd_mask_example':
          return String(args.sentence || '').replace(
            new RegExp(String(args.word || '').replace(/[.*+?^${}()|[\]\\]/g, '\\$&'), 'i'), '____');
        case 'cmd_build_advanced_card': {
          const s = store.session;
          if (!s || s.idx >= s.queue.length) return null;
          const e = s.queue[s.idx];
          const m = args.mode || 'EnToZh';
          const ex = (e.senses[0].examples[0] || {}).text || '';
          const masked = ex.replace(new RegExp(e.word, 'i'), '____');
          const showEx = m === 'ExToZh' || m === 'ExPickWord';
          return {
            prompt: showEx ? masked : e.word,
            answer: (m === 'ExToZh') ? e.senses[0].definition : e.word,
            entry: e, options: store.words.filter(x => x.word !== e.word).slice(0, 3)
              .map(x => (m === 'ExToZh' ? x.senses[0].definition : x.word)),
            mode: m, is_leech: false,
            example_masked: masked, example_raw: ex, example_translation: '（示例译文）',
            spell_hint: e.word.split('').map(() => '_').join(' '), audio: '',
            index: s.idx + 1, total: s.queue.length, correct_count: s.correct, wrong_count: s.wrong,
          };
        }

        default:
          if (cmd.startsWith('sidebar_') || cmd === 'main_show') return true;
          return null;
      }
    },
  };
})();

/* ---------------- AI 讲解语言 ---------------- */

/**
 * 可选的讲解语言。与后端 `llm::explain_lang_name` 保持一致：
 * value 是 ISO 639-1 代码，label 用中文标出，模型侧会换成该语言的自称。
 */
const EXPLAIN_LANGS = [
  { value: 'zh', label: '中文' },
  { value: 'en', label: 'English' },
  { value: 'ja', label: '日本語' },
  { value: 'ko', label: '한국어' },
  { value: 'fr', label: 'Français' },
  { value: 'de', label: 'Deutsch' },
  { value: 'es', label: 'Español' },
  { value: 'ru', label: 'Русский' },
];

/** 生成讲解语言下拉的 <option> 列表 HTML。 */
function explainLangOptions(current) {
  const cur = (current || 'zh').toLowerCase();
  return EXPLAIN_LANGS
    .map(l => `<option value="${l.value}"${l.value === cur ? ' selected' : ''}>${l.label}</option>`)
    .join('');
}

/** 讲解语言的代码 → 显示名（用于「查看原文」按钮文案等）。 */
function explainLangLabel(code) {
  const l = EXPLAIN_LANGS.find(x => x.value === (code || '').toLowerCase());
  return l ? l.label : (code || 'zh');
}

window.WordWiseAPI = {
  API, invoke, listen, HAS_TAURI,
  EXPLAIN_LANGS, explainLangOptions, explainLangLabel,
};
