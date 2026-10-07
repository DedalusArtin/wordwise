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
  // 演示模式：只接管白名单里的「读」命令。写操作（提交答案、清错词、存设置…）
  // 一律照常走后端，所以示例数据永远不可能落进真实数据库。
  if (window.Demo && window.Demo.enabled && window.Demo.has(cmd)) {
    return window.Demo.for(cmd, args);
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

/* ---------------- 窗口控制 ----------------
 *
 * ★ 这里**必须走 invoke**，不能写 window.__TAURI__.window.getCurrentWindow()。
 *   tauri.conf.json 并没有开 withGlobalTauri，所以 webview 里 window.__TAURI__
 *   是 undefined，那样写会直接抛 TypeError；而它发生在 async 的事件回调里，
 *   没人接这个异常 —— 表现就是「点了没反应、也没有任何报错」。
 *   标题栏的最小化 / 最大化 / 关闭三个按钮一度全废，就是这么来的，
 *   而且查起来毫无线索。invoke 走的是 __TAURI_INTERNALS__，一直可用。
 */

/** 当前窗口的 label（主窗口 main / 侧边栏 sidebar）。 */
function currentWindowLabel() {
  // 侧边栏用的就是同一个 index.html，只是加了 ?view=sidebar，
  // 与 windows.rs 的 SIDEBAR_LABEL + WebviewUrl::App("index.html?view=sidebar") 对应。
  try {
    if (new URLSearchParams(window.location.search).get('view') === 'sidebar') return 'sidebar';
  } catch (e) { /* 解析不了就当主窗口 */ }
  return 'main';
}

const winMinimize = () => invoke('plugin:window|minimize', { label: currentWindowLabel() });
const winToggleMaximize = () => invoke('plugin:window|toggle_maximize', { label: currentWindowLabel() });
const winIsMaximized = () => invoke('plugin:window|is_maximized', { label: currentWindowLabel() });
/** 隐藏窗口（退到托盘）。注意这是「隐藏」不是「退出」，退出要走托盘菜单。 */
const winHide = () => invoke('plugin:window|hide', { label: currentWindowLabel() });

/** 窗口控制统一入口。 */
const WIN = {
  label: currentWindowLabel,
  minimize: winMinimize,
  toggleMaximize: winToggleMaximize,
  isMaximized: winIsMaximized,
  hide: winHide,
};

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
  // N 卡 / GPU 加速状态（nvidia-smi 探测 + Vulkan 引擎）
  gpuStatus: () => invoke('cmd_gpu_status'),
  // 真正退出应用（关闭行为设为「直接退出」时用）
  appExit: () => invoke('cmd_app_exit'),
  // 运行日志：自动分析（错误/警告 → 分类 + 建议）与前端写日志
  logAnalysis: () => invoke('cmd_log_analysis'),
  logWrite: (level, message) => invoke('cmd_log_write', { level, message }),
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

  // 翻译（需求 1-5）：三级链路 —— 本地缓存 → 有道在线 → 本地大模型兜底
  translate: (text, from, to, force) =>
    invoke('cmd_translate', { text, from: from || null, to: to || null, force: force || false }),
  translateAi: (action, text, translated, sourceLang, targetLang) =>
    invoke('cmd_translate_ai', {
      action, text, translated: translated || null,
      sourceLang: sourceLang || null, targetLang: targetLang || null,
    }),
  translateHistory: (limit, onlyFavorite) =>
    invoke('cmd_translate_history', { limit: limit || 200, onlyFavorite: onlyFavorite || false }),
  translateFavorite: (id, on) => invoke('cmd_translate_favorite', { id, on: !!on }),
  translateDelete: (id) => invoke('cmd_translate_delete', { id }),
  translateClear: (keepFavorite) =>
    invoke('cmd_translate_clear', { keepFavorite: keepFavorite !== false }),
  swapDirection: () => invoke('cmd_swap_direction'),
  translateLangs: () => invoke('cmd_translate_langs'),
  translateStatus: () => invoke('cmd_translate_status'),

  // 查词搜索
  lookup: (word, lang, forceRefresh) =>
    invoke('cmd_lookup', { word, lang: lang || null, forceRefresh: !!forceRefresh }),
  // 双向词条：目标语言侧的对应词及其完整词条（[{ word, lang, entry, via }]）。
  // 失败一律返回空数组，绝不抛错 —— 没有对应词不等于查词失败。
  // translated / alternatives 是「已经翻译过了」的意思：查词页的「词级译文」
  // 会把结果递过来，避免同一个限频很严的翻译接口被打两次。
  lookupPairs: (word, from, to, translated, alternatives) =>
    invoke('cmd_lookup_pairs', {
      word,
      from: from || null,
      to: to || null,
      alreadyTranslated: translated || null,
      alreadyAlternatives: alternatives && alternatives.length ? alternatives : null,
    }),
  suggest: (query, lang) => invoke('cmd_suggest', { query, lang: lang || null }),
  search: (query, lang, withWiki) =>
    invoke('cmd_search', { query, lang: lang || null, withWiki: withWiki !== false }),
  wiki: (word, lang) => invoke('cmd_wiki', { word, lang: lang || null }),
  dictLinks: (word, lang) => invoke('cmd_dict_links', { word, lang: lang || null }),
  /**
   * 打开外部链接。
   *
   * 分流收在后端命令层：设置里开了「链接在软件内打开」就走应用内浏览窗口，
   * 否则弹系统浏览器 —— 前端调用点不用各自判断。
   * `openExternal` 给「必须离开软件」的动作（下载安装包更新）留一条
   * 直通系统浏览器的路：应用内 WebView 接不住安装包下载。
   */
  openUrl: (url) => invoke('cmd_open_url', { url }),
  /** 无视「软件内打开」开关，始终用系统浏览器（更新下载等场景专用）。 */
  openExternal: (url) => invoke('cmd_open_url', { url, forceExternal: true }),
  /**
   * 在**应用内**打开网页（需求 11）。
   *
   * 与 `openUrl` 的分工：`openUrl` 一律弹系统默认浏览器（「权威辞书」那排
   * 跳转仍然走它 —— 那里的意图就是「离开软件去官网看原文」）；本接口走应用内
   * 浏览窗口，用于「在线搜索结果」这类**看个片段就够了**的场景。
   * 桌面端开应用内窗口，移动端没有多窗口语义，后端会自己回退到系统浏览器。
   */
  openInApp: (url) => invoke('cmd_open_in_app', { url }),
  getWord: (word, lang) => invoke('cmd_get_word', { word, lang: lang || null }),
  /**
   * 同族派生词（需求 5）：`happy → happiness / happier …`。
   *
   * 后端由词形规则生成候选、再去**本地词库**确认哪些真的存在，所以返回的
   * 每一条都点得动、查得到。不联网、不调模型，瞬间返回。
   */
  wordFamily: (word, lang) => invoke('cmd_word_family', { word, lang: lang || null }),
  searchWords: (query, lang, limit) =>
    invoke('cmd_search_words', { query, lang: lang || null, limit: limit || 50 }),
  recentSearches: () => invoke('cmd_recent_searches'),

  // AI 讲解存档（「讲解也是一种存储，可以在词库里搜到」）
  // 讲解在生成时后端已自动落库，这里的 saveExplain 主要是为了**拿回存档行**
  // （里面带 saved 标记），让「并入词库」按钮的状态在重开面板后依然正确。
  saveExplain: (word, lang, explainLang, text, original, translated) =>
    invoke('cmd_save_explain', {
      word, lang: lang || null, explainLang: explainLang || null,
      text, original: original || null, translated: !!translated,
    }),
  getExplain: (word, lang, explainLang) =>
    invoke('cmd_get_explain', { word, lang: lang || null, explainLang: explainLang || null }),
  listExplains: (word, lang) => invoke('cmd_list_explains', { word, lang: lang || null }),
  searchExplains: (query, lang, limit) =>
    invoke('cmd_search_explains', { query, lang: lang || null, limit: limit || 50 }),
  deleteExplain: (word, lang, explainLang) =>
    invoke('cmd_delete_explain', { word, lang: lang || null, explainLang: explainLang || null }),
  clearExplains: (keepSaved) =>
    invoke('cmd_clear_explains', { keepSaved: keepSaved !== false }),
  explainCount: () => invoke('cmd_explain_count'),
  // 把一段讲解整理成结构化词条并写入词库（text 可选：不传就读存档）
  explainToEntry: (word, lang, explainLang, text) =>
    invoke('cmd_explain_to_entry', {
      word, lang: lang || null, explainLang: explainLang || null, text: text || null,
    }),

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
  startBookSession: (bookId, mode, size, lang, defLang) =>
    invoke('cmd_start_book_session', {
      bookId: bookId || null,
      mode: mode || null,
      size: size || null,
      lang: lang || null,
      defLang: defLang || null,
    }),
  checkSpelling: (input, answer, strict) =>
    invoke('cmd_check_spelling', { input, answer, strict: !!strict }),
  spellHint: (word, reveal) => invoke('cmd_spell_hint', { word, reveal: reveal || 0 }),
  maskExample: (sentence, word) => invoke('cmd_mask_example', { sentence, word }),
  buildAdvancedCard: (mode, lang, kind) =>
    invoke('cmd_build_advanced_card', { mode, lang: lang || null, kind: kind || null }),

  /* ---------------- 背诵 / 复习是两个独立会话 ----------------
     `kind` 一路透传到后端：缺省 / 'study' → 背诵槽位，'review' → 复习槽位。
     两者互不覆盖，所以「正在背新词」时点开今日复习，不会把背诵进度清掉，
     反之亦然（需求 14：不要复用背诵和复习的容器）。

     ★ 所有会话类命令都必须带同一个 kind，漏一个的后果是
       「题从复习槽出、答案记进背诵槽」—— 判分与计数会静默错位。 */

  // 背诵
  startSession: (mode, size, leechOnly, lang, defLang, kind) =>
    invoke('cmd_start_session', {
      mode: mode || null,
      size: size || null,
      leechOnly: !!leechOnly,
      lang: lang || null,
      defLang: defLang || null,
      kind: kind || null,
    }),
  /**
   * 开始一轮**今日复习**。
   *
   * ★ 与 startSession 的关键区别：队列**只由今天到期的词构成，绝不补足**。
   *   入口按钮上写「12 个」，点进去就必须是 12 个 —— 补到 batch_size 会让
   *   用户觉得数字是假的（需求 14）。一个到期的都没有时返回 total: 0，
   *   属于正常状态，不是错误。
   * @param {number|null} size 截断上限，null = 全部到期词
   */
  startReviewSession: (mode, size, lang, defLang) =>
    invoke('cmd_start_review_session', {
      mode: mode || null,
      size: size || null,
      lang: lang || null,
      defLang: defLang || null,
    }),
  currentQuestion: (lang, kind) =>
    invoke('cmd_current_question', { lang: lang || null, kind: kind || null }),
  submitAnswer: (word, grade, elapsedMs, lang, kind) =>
    invoke('cmd_submit_answer', {
      word: word || null,
      grade,
      elapsedMs: elapsedMs || 0,
      lang: lang || null,
      kind: kind || null,
    }),
  skip: (kind) => invoke('cmd_skip', { kind: kind || null }),
  endSession: (kind) => invoke('cmd_end_session', { kind: kind || null }),
  wordState: (word, lang) => invoke('cmd_word_state', { word, lang: lang || null }),

  /* ---------------- 学习目标（需求 15） ----------------
     一个目标 = 模式(off/days/per_day) + 天数 + 每日词量 + 词库范围。
     后端一次算出今日目标、完成度、预计完成日、复习压力与是否达标，
     前端只负责显示 —— 两边各算一遍迟早会出现「界面说还差 3 个、
     点进去却已经完成」这种自相矛盾。 */
  studyGoal: (bookId, lang) =>
    invoke('cmd_study_goal', { bookId: bookId || null, lang: lang || null }),
  setStudyGoal: (opts) => invoke('cmd_set_study_goal', {
    mode: (opts && opts.mode) || 'off',
    days: (opts && opts.days != null) ? opts.days : null,
    perDay: (opts && opts.perDay != null) ? opts.perDay : null,
    bookId: (opts && opts.bookId != null) ? opts.bookId : null,
  }),
  /** 「多背一点」：只抬高**今天**的目标，跨天自动失效。 */
  extraStudy: (extra, bookId, lang) =>
    invoke('cmd_extra_study', { extra, bookId: bookId || null, lang: lang || null }),

  // 统计
  stats: (lang) => invoke('cmd_stats', { lang: lang || null }),
  reviewPlan: (days, lang) => invoke('cmd_review_plan', { days: days || 14, lang: lang || null }),
  dueWords: (limit, lang) => invoke('cmd_due_words', { limit: limit || 60, lang: lang || null }),
  leechList: (limit, lang) => invoke('cmd_leech_list', { limit: limit || 100, lang: lang || null }),
  clearLeech: (word, lang) => invoke('cmd_clear_leech', { word, lang: lang || null }),

  // 错题本增强：筛选 / 概览 / 批量移出 / 导出
  leechQuery: (opts, lang) => invoke('cmd_leech_query', {
    lang: lang || null,
    minWrong: (opts && opts.minWrong) || 0,
    maxMastery: opts && opts.maxMastery != null ? opts.maxMastery : 100,
    minErrorRate: (opts && opts.minErrorRate) || 0,
    order: (opts && opts.order) || 'wrong',
    limit: (opts && opts.limit) || 300,
  }),
  leechSummary: (lang) => invoke('cmd_leech_summary', { lang: lang || null }),
  leechRemoveMany: (words, lang) =>
    invoke('cmd_leech_remove_many', { words, lang: lang || null }),
  leechExport: (opts, lang) => invoke('cmd_leech_export', {
    format: (opts && opts.format) || 'md',
    path: (opts && opts.path) || null,
    lang: lang || null,
    minWrong: (opts && opts.minWrong) || 0,
    maxMastery: opts && opts.maxMastery != null ? opts.maxMastery : 100,
    minErrorRate: (opts && opts.minErrorRate) || 0,
    order: (opts && opts.order) || 'wrong',
  }),

  // 知识图谱（独立栏目）
  graphView: (center, depth, lang) =>
    invoke('cmd_graph_view', { center: center || null, depth: depth || 1, lang: lang || null }),
  graphBuild: (lang) => invoke('cmd_graph_build', { lang: lang || null }),
  graphExpand: (word, lang) => invoke('cmd_graph_expand', { word, lang: lang || null }),
  graphSearch: (q, lang) => invoke('cmd_graph_search', { q: q || '', lang: lang || null }),
  graphRels: () => invoke('cmd_graph_rels'),
  graphClear: (lang) => invoke('cmd_graph_clear', { lang: lang || null }),
  graphStats: (lang) => invoke('cmd_graph_stats', { lang: lang || null }),

  // 数据库维护
  dbInfo: () => invoke('cmd_db_info'),
  dbMaintain: (action) => invoke('cmd_db_maintain', { action }),
  openDir: (path) => invoke('cmd_open_dir', { path }),
  openUrl: (url) => invoke('cmd_open_url', { url }),
  openExternal: (url) => invoke('cmd_open_url', { url, forceExternal: true }),

  // 数据与模型的存放位置（装到哪，数据就落哪，默认不写 C 盘）
  storageInfo: () => invoke('cmd_storage_info'),
  setDataDir: (path, migrate) =>
    invoke('cmd_set_data_dir', { path: path || '', migrate: migrate !== false }),
  setModelsDir: (path, migrate) =>
    invoke('cmd_set_models_dir', { path: path || '', migrate: migrate !== false }),
  restartApp: () => invoke('cmd_restart_app'),

  // 本地大模型一键部署
  localLlmStatus: () => invoke('cmd_local_llm_status'),
  localLlmModels: () => invoke('cmd_local_llm_models'),
  localLlmInstallEngine: () => invoke('cmd_local_llm_install_engine'),
  localLlmInstallModel: (modelId) => invoke('cmd_local_llm_install_model', { modelId }),
  localLlmStart: (modelId) => invoke('cmd_local_llm_start', { modelId: modelId || null }),
  localLlmStop: () => invoke('cmd_local_llm_stop'),
  localLlmProbe: () => invoke('cmd_local_llm_probe'),
  localLlmCancel: () => invoke('cmd_local_llm_cancel'),
  localLlmRemoveModel: (modelId) => invoke('cmd_local_llm_remove_model', { modelId }),
  setLocalLlmAuto: (enabled) => invoke('cmd_set_local_llm_auto', { enabled: !!enabled }),
  // 部署进度是后端主动推的事件
  onLocalLlmProgress: (fn) => listen('local-llm://progress', fn),

  // 词典源
  getSources: () => invoke('cmd_get_sources'),
  saveSources: (sources) => invoke('cmd_save_sources', { sources }),
  testSource: (source, word) => invoke('cmd_test_source', { source, word: word || null }),
  resetSources: () => invoke('cmd_reset_sources'),
  clearCache: () => invoke('cmd_clear_cache'),

  // 数据
  exportData: (path) => invoke('cmd_export', { path: path || null }),
  importData: (path) => invoke('cmd_import', { path }),

  // 在线更新（版本检测 / 下载安装包 / 启动安装程序）
  checkUpdate: () => invoke('cmd_check_update'),
  downloadUpdate: () => invoke('cmd_download_update'),
  updateCancel: () => invoke('cmd_update_cancel'),
  runUpdate: (path) => invoke('cmd_run_update', { path }),
  openUpdateDir: () => invoke('cmd_open_update_dir'),
  updatePrefs: () => invoke('cmd_update_prefs'),
  // 只传要改的那个：两个都传 null 等于什么都不改（后端用 Option 区分）
  setUpdatePrefs: (opts) => invoke('cmd_set_update_prefs', {
    checkOnStart: (opts && opts.checkOnStart != null) ? !!opts.checkOnStart : null,
    skipVersion: (opts && opts.skipVersion != null) ? String(opts.skipVersion) : null,
  }),
  // 更新包下载进度也是后端主动推的事件（与本地模型部署分开，互不覆盖）
  onUpdateProgress: (fn) => listen('update://progress', fn),

  // 朗读（本地 Piper 神经语音）
  ttsStatus: () => invoke('cmd_tts_status'),
  ttsInstallEngine: () => invoke('cmd_tts_install_engine'),
  ttsInstallVoice: (voiceId) => invoke('cmd_tts_install_voice', { voiceId }),
  ttsRemoveVoice: (voiceId) => invoke('cmd_tts_remove_voice', { voiceId }),
  ttsCancel: () => invoke('cmd_tts_cancel'),
  ttsPrefs: () => invoke('cmd_tts_prefs'),
  setTtsPrefs: (opts) => invoke('cmd_set_tts_prefs', {
    engine: (opts && opts.engine != null) ? String(opts.engine) : null,
    voiceLocal: (opts && opts.voiceLocal != null) ? String(opts.voiceLocal) : null,
    voiceOnline: (opts && opts.voiceOnline != null) ? String(opts.voiceOnline) : null,
    rate: (opts && typeof opts.rate === 'number') ? opts.rate : null,
  }),
  ttsSpeak: (text, lang, accent) => invoke('cmd_tts_speak', {
    text, lang: lang || null, accent: accent || null,
  }),
  ttsClearCache: () => invoke('cmd_tts_clear_cache'),
  // 后台词库内容增强的进度（AI 空闲时把单薄词条补成详解）
  enrichStatus: () => invoke('cmd_enrich_status'),
  // 语音包下载进度（与更新、本地模型三条流各自独立）
  onTtsProgress: (fn) => listen('tts://progress', fn),
  onTtsDone: (fn) => listen('tts://done', fn),

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
    // 背诵与复习各自一个会话槽位（需求 14：不要复用容器）。
    // 后端是 `AppState.study` / `AppState.review` 两个 RwLock，
    // 调试模式必须同构，否则「一边背一边复习」的交互在调试模式里验不出来。
    sessions: { study: null, review: null },
    // AI 讲解存档：key = `${word}|${lang}|${explain_lang}`（与后端主键同构）
    explains: new Map(),
    config: {
      llm: { base_url: 'http://127.0.0.1:1234/v1', model: '', temperature: 0.6, max_tokens: 1024,
             timeout_secs: 120, api_key: 'lm-studio', system_prompt: '' },
      study: {
        show_inflections: true, show_examples: true, show_related: false,
        show_mnemonic: true, show_phonetic: true, auto_popup_on_wrong: true,
        ai_explain: true, batch_size: 20, daily_limit: 120,
        // 与后端 StudyOptions 的新字段一一对应（选项个数 / 学习目标）
        option_count: 4, goal_mode: 'off', goal_days: 30, goal_per_day: 30,
        goal_book_id: '', goal_started_at: '', goal_extra_today: 0, goal_extra_date: '',
      },
      target_lang: 'en', ui_lang: 'zh-CN', sidebar_always_on_top: true, sidebar_width: 380,
      source_lang: 'auto',
      explain_lang: 'zh', explain_auto_translate: true, explain_translate_template: '',
      srs: { base_intervals: [1,2,4,7,15,30,90,180] },
      dict_sources: [],
      auto_start_local_llm: false,
      check_update_on_start: true,
      skip_update_version: '',
    },
    transHistory: [],
  };

  /** kind → 槽位名。无法识别的 kind 一律当背诵（与后端 session_slot 同口径）。 */
  const slotOf = (kind) => (kind === 'review' ? 'review' : 'study');

  /* ---- 调试模式的翻译小词典：让界面能显示出「真的翻了」的样子 ---- */
  const MOCK_TRANS = {
    '你好|ja': 'こんにちは',
    '你好|en': 'Hello',
    '你好|ko': '안녕하세요',
    '你好|fr': 'Bonjour',
    '谢谢|ja': 'ありがとう',
    '谢谢|en': 'Thank you',
    '早上好|ja': 'おはようございます',
    'hello|zh': '你好',
    'hello world|zh': '你好世界',
    'こんにちは|zh': '你好',
    'happy|zh': '快乐的',
    '快乐|en': 'happy',
  };

  /**
   * 调试专用的「词性与释义」样本（需求 4）。
   *
   * 后端这两个字段来自有道 `basic.explains` / `web`，调试环境里没有网络。
   * 与其编一份假的通用释义污染所有词，不如只给少数常用词备真实内容 ——
   * 其余词不出这块 UI，正好也验证了「没数据就不渲染」这条降级路径。
   */
  const MOCK_DICT = {
    'happy|zh': {
      explains: [
        'adj. 快乐的；幸福的；愉快的',
        'adj. 乐意的；甘愿的',
        'adj. 幸运的；恰到好处的',
        'adj. （言语或行为）得体的，恰当的',
      ],
      web: ['快乐的', '幸福的', '高兴的', '愉快的'],
    },
    '快乐|en': {
      explains: ['n. happiness; joy; delight', 'adj. happy; cheerful; joyful'],
      web: ['happy', 'joyful', 'cheerful', 'merry'],
    },
  };

  const MOCK_LANG_NAMES = {
    auto: '自动检测', zh: '中文', en: '英语', ja: '日语', ko: '韩语',
    fr: '法语', de: '德语', es: '西班牙语', ru: '俄语', pt: '葡萄牙语', it: '意大利语',
  };
  function mockLangName(code) { return MOCK_LANG_NAMES[code] || code; }

  /** 与后端 dict::detect_lang 同源的粗判（调试模式够用）。 */
  function mockGuessLang(s) {
    for (const ch of String(s || '')) {
      const c = ch.codePointAt(0);
      if ((c >= 0x3040 && c <= 0x30ff) || (c >= 0x31f0 && c <= 0x31ff)) return 'ja';
      if ((c >= 0x1100 && c <= 0x11ff) || (c >= 0xac00 && c <= 0xd7af)) return 'ko';
      if ((c >= 0x4e00 && c <= 0x9fff) || (c >= 0x3400 && c <= 0x4dbf)) return 'zh';
      if (c >= 0x0400 && c <= 0x04ff) return 'ru';
    }
    return 'en';
  }

  /* ---- 错题本 mock：从示例词库派生稳定的错词记录 ---- */
  //
  // 为什么必须是**确定性**的：冒烟测试要断言「筛选后条数为 X」，
  // 用 Math.random 会让断言随机失败，那比不做测试更糟。
  function mockLeech() {
    init();
    const removed = store.leechRemoved || new Set();
    return store.words.slice(0, 8)
      .filter(w => !removed.has(w.word))
      .map((e, i) => {
        const wrong = 9 - i;
        const correct = Math.max(0, 3 - i);
        return {
          entry: e,
          state: {
            word: e.word, lang: 'en', ease_factor: 2.5, interval_days: 1,
            repetitions: 0, due_at: Math.floor(Date.now() / 1000) + 3600 * (i + 1),
            last_review_at: Math.floor(Date.now() / 1000) - 86400,
            correct_count: correct, wrong_count: wrong,
            is_leech: true, mastery: Math.min(90, 10 + i * 9), is_mastered: false,
          },
          retention: Math.max(0.05, 0.9 - i * 0.11),
          error_rate: wrong / Math.max(1, wrong + correct),
        };
      });
  }

  /* ---- 图谱 mock：从词条已有的 related / inflections 抽边 ---- */
  function mockEdges() {
    init();
    const out = [];
    for (const e of store.words) {
      for (const r of (e.related || [])) {
        const s = String(r).trim();
        if (s && s.length <= 24 && s.toLowerCase() !== e.word.toLowerCase()) {
          out.push({ src: e.word, dst: s, rel: 'related', weight: 1, source: 'local' });
        }
      }
      for (const inf of (e.inflections || [])) {
        const f = String((inf && inf.form) || '').trim();
        if (f && f.toLowerCase() !== e.word.toLowerCase()) {
          out.push({ src: e.word, dst: f, rel: 'derived', weight: 0.6, source: 'local' });
        }
      }
    }
    out.push(...(store.graphExtra || []));
    return out;
  }

  function mockNodes(words, edges) {
    const deg = new Map();
    for (const e of edges) {
      deg.set(e.src, (deg.get(e.src) || 0) + 1);
      deg.set(e.dst, (deg.get(e.dst) || 0) + 1);
    }
    const byWord = new Map(store.words.map(w => [w.word, w]));
    return words.map(w => {
      const e = byWord.get(w);
      return {
        word: w, lang: 'en', degree: deg.get(w) || 0,
        in_dict: !!e,
        gloss: (e && e.senses && e.senses[0] && e.senses[0].definition) || '',
        mastery: null,
      };
    });
  }

  function mockTop(words) {
    const deg = new Map();
    for (const e of mockEdges()) {
      deg.set(e.src, (deg.get(e.src) || 0) + 1);
      deg.set(e.dst, (deg.get(e.dst) || 0) + 1);
    }
    return words.map(w => ({ word: w, degree: deg.get(w) || 0 }))
      .sort((a, b) => b.degree - a.degree)
      .slice(0, 12);
  }

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

  /**
   * 学习目标（调试模式）。
   *
   * 与后端 `compute_study_goal` **同一套口径**，不是随便造几个数：
   * 今天的目标 = 新词摊派 + 今天到期；已完成 = 今天答过的去重词数。
   * 界面要验的是「数字自洽 + 进度条 + 压力提示 + 达标判定」，
   * 口径一旦不同，调试模式里通过、真机上打脸，比没有调试模式更糟。
   */
  function mockGoal(bookId) {
    const st = store.config.study || {};
    const today = new Date().toISOString().slice(0, 10);
    const mode = st.goal_mode || 'off';
    const extra = (st.goal_extra_date === today) ? (st.goal_extra_today || 0) : 0;

    const total = store.words.length;
    const learned = Math.max(0, Math.min(total, Math.round(total * 0.4)));
    const remaining = Math.max(0, total - learned);
    // 调试模式的「今天到期」就取 60%（与 cmd_start_review_session 同源）
    const dueToday = total ? Math.max(1, Math.round(total * 0.6)) : 0;
    const todayDone = Math.max(0, Math.round(dueToday * 0.5));

    let daily = 0;
    let days = 0;
    if (mode === 'days') {
      days = st.goal_days || 30;
      daily = Math.max(1, Math.ceil(remaining / days));
    } else if (mode === 'per_day') {
      daily = st.goal_per_day || 30;
    }
    const todayTarget = mode === 'off' ? 0 : daily + dueToday + extra;
    const todayRemaining = Math.max(0, todayTarget - todayDone);
    // 复习压力：今天的目标一旦超过未来 7 天平均复习量的 3 倍，就判 high
    const load7d = dueToday * 3;
    const pressure = (todayTarget > 0 && todayTarget > (load7d / 7) * 3) ? 'high' : 'normal';
    const finished = remaining === 0;
    // ★ 与后端同一口径：按**计划的每日新词量**外推，不是按今天已背了多少。
    //   后端曾经写成 remaining / today_done，结果「30 天目标」报出「2028 年」。
    //   没有目标（off）或速率为 0 时不编造，返回空串让前端整条不显示。
    const eta_date = (!daily || remaining === 0) ? (remaining === 0 ? today : '')
      : new Date(new Date(`${today}T00:00:00`).getTime()
        + Math.ceil(remaining / daily) * 86400000).toISOString().slice(0, 10);

    return {
      mode, days, per_day: mode === 'per_day' ? daily : 0,
      book_id: bookId || st.goal_book_id || '',
      book_name: (bookId || st.goal_book_id) ? '（调试模式词库）' : '全部词库',
      total_words: total, learned, remaining,
      today_target: todayTarget, today_done: todayDone, today_remaining: todayRemaining,
      eta_date,
      days_left: mode === 'days' ? days : 0,
      due_today: dueToday, review_load_7d: load7d,
      pressure, on_track: todayDone >= todayTarget, finished,
    };
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
        case 'cmd_llm_status': {
          // endpoint_kind 与后端同义：managed / local / cloud。
          // 调试模式没有托管服务，127.0.0.1 就算 local。
          const b = (store.config.llm && store.config.llm.base_url) || '';
          const kind = /127\.0\.0\.1|localhost|\[::1\]/.test(b) ? 'local' : 'cloud';
          return { online: false, base_url: b, models: [], active_model: '',
                   endpoint_kind: kind,
                   message: '（调试模式）未连接 AI 服务' };
        }
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
        case 'cmd_lookup_pairs': return [];
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
        case 'cmd_open_in_app': return null;
        // 同族派生词（需求 5）：调试模式的词库只有少量样本词，
        // 给 happy 备一份与真实词库同构的数据，界面才验得到。
        case 'cmd_word_family': {
          const w = String(args.word || '').toLowerCase();
          if (w === 'happy' || w === 'happiness') {
            return [
              { word: 'happy', pos: 'adj.', definition: '快乐的；幸福的' },
              { word: 'happiness', pos: 'n.', definition: '幸福；快乐' },
              { word: 'happily', pos: 'adv.', definition: '快乐地；幸福地' },
            ].filter(d => d.word !== w).slice(0, 8);
          }
          return [];
        }
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
          const s = {
            idx: 0, queue: store.words.slice(), correct: 0, wrong: 0,
            mode: args.mode || 'en_to_zh',
          };
          store.sessions[slotOf(args.kind)] = s;
          store.session = s;   // 兼容旧调用点（调试模式内部）
          return { mode: s.mode, total: s.queue.length, index: 0,
                   correct: 0, wrong: 0, leech_only: !!args.leechOnly };
        }
        /* 今日复习（需求 14）：**不补足**到 batch_size。
           调试模式里没有 SRS 到期信息，就用「前 60%」模拟一批到期词 ——
           这不是为了精确，是为了让「按钮写 12 就一定是 12 题」这条约束
           在调试模式里也能被验证。 */
        case 'cmd_start_review_session': {
          const all = store.words.slice();
          const size = args.size ? Math.min(args.size, all.length)
            : Math.max(1, Math.round(all.length * 0.6));
          const s = {
            idx: 0, queue: all.slice(0, size), correct: 0, wrong: 0,
            mode: args.mode || 'en_to_zh',
          };
          store.sessions.review = s;
          return { mode: s.mode, total: s.queue.length, index: 0,
                   correct: 0, wrong: 0, leech_only: false };
        }
        case 'cmd_current_question': {
          const s = store.sessions[slotOf(args.kind)];
          if (!s || s.idx >= s.queue.length) return null;
          const e = s.queue[s.idx];
          const en = s.mode === 'en_to_zh';
          // 选项个数跟着配置走（2~8），并按后端同样口径去掉正确答案自身
          const want = Math.min(8, Math.max(2, Number(store.config.study.option_count) || 4));
          const others = store.words.filter(x => x.word !== e.word).slice(0, want - 1)
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
          const s = store.sessions[slotOf(args.kind)];
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
          const s = store.sessions[slotOf(args.kind)];
          if (s) s.idx++;
          return { mode: 'en_to_zh', total: s ? s.queue.length : 0, index: s ? s.idx : 0,
                   correct: s ? s.correct : 0, wrong: s ? s.wrong : 0, leech_only: false };
        }
        case 'cmd_end_session': {
          // 只清被指定的那个槽位：结束复习不该把背诵会话一起清掉
          store.sessions[slotOf(args.kind)] = null;
          return { mode: 'en_to_zh', total: 0, index: 0, correct: 0, wrong: 0, leech_only: false };
        }
        case 'cmd_word_state': return null;

        /* ---- 学习目标（调试模式：按同一套公式算，便于验证界面） ---- */
        case 'cmd_study_goal': return mockGoal(args.bookId);
        case 'cmd_set_study_goal': {
          const st = store.config.study;
          const wasOff = st.goal_mode === 'off';
          st.goal_mode = args.mode || 'off';
          if (args.days != null) st.goal_days = args.days;
          if (args.perDay != null) st.goal_per_day = args.perDay;
          if (args.bookId != null) st.goal_book_id = args.bookId;
          // 只有首次从「不设目标」切过来才记开始日期（与后端口径一致）
          if (wasOff && st.goal_mode !== 'off') {
            st.goal_started_at = new Date().toISOString().slice(0, 10);
          }
          if (st.goal_mode === 'off') { st.goal_extra_today = 0; st.goal_extra_date = ''; }
          return mockGoal(null);
        }
        case 'cmd_extra_study': {
          const st = store.config.study;
          const today = new Date().toISOString().slice(0, 10);
          st.goal_extra_today = (st.goal_extra_date === today)
            ? (st.goal_extra_today || 0) + (args.extra || 0) : (args.extra || 0);
          st.goal_extra_date = today;
          return mockGoal(args.bookId);
        }
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
        case 'cmd_due_words': {
          // 固定几条，别用随机数：冒烟测试要断言列表条数
          const base = Date.now();
          const mk = (w, daysAgo, wrong, leech) => ({
            word: w,
            gloss: `n. ${w} 的释义`,
            due_at: Math.floor((base - daysAgo * 86400000) / 1000),
            due_label: daysAgo > 0 ? `逾期 ${daysAgo} 天` : '今天 09:00',
            overdue: daysAgo > 0,
            overdue_days: daysAgo > 0 ? daysAgo : 0,
            mastery: 40 - daysAgo * 5,
            wrong_count: wrong,
            is_leech: !!leech,
          });
          // 排序要和后端 cmd_due_words 一致（逾期久的在前），否则前端测试断言
          // 「逾期排在今天前面」在调试模式下会失效
          return [mk('abandon', 5, 6, true), mk('benefit', 2, 3, false),
                  mk('capture', 0, 1, false), mk('decline', 0, 0, false)]
            .sort((a, b) => b.overdue_days - a.overdue_days || a.due_at - b.due_at)
            .slice(0, args.limit || 60);
        }
        case 'cmd_leech_list': return mockLeech().map(x => ({
          entry: x.entry, state: x.state, retention: x.retention,
        }));
        case 'cmd_clear_leech': return null;
        case 'cmd_get_sources': return store.config.dict_sources;
        case 'cmd_save_sources': store.config.dict_sources = args.sources; return null;
        case 'cmd_test_source':
          return { ok: false, message: '（调试模式）不发起真实请求', sample: null, elapsed_ms: 0 };
        case 'cmd_reset_sources': return [];
        case 'cmd_clear_cache': return null;
        case 'cmd_export': return '(调试模式) 未实际导出';
        case 'cmd_import': return { added: 0, skipped: 0, prefetched: 0 };
        /* ---- 错题本增强（调试模式） ---- */
        case 'cmd_leech_query': {
          let rows = mockLeech();
          const minWrong = args.minWrong || 0;
          const maxMastery = args.maxMastery == null ? 100 : args.maxMastery;
          const minRate = args.minErrorRate || 0;
          rows = rows.filter(x => x.state.wrong_count >= minWrong
            && x.state.mastery <= maxMastery && x.error_rate >= minRate);
          const order = args.order || 'wrong';
          rows.sort((a, b) => {
            if (order === 'mastery') return a.state.mastery - b.state.mastery;
            if (order === 'word') return a.entry.word.localeCompare(b.entry.word);
            if (order === 'rate') return b.error_rate - a.error_rate;
            return b.state.wrong_count - a.state.wrong_count;
          });
          return rows.slice(0, args.limit || 300);
        }
        case 'cmd_leech_summary': {
          const rows = mockLeech();
          const total = rows.length;
          const tw = rows.reduce((s, x) => s + x.state.wrong_count, 0);
          return {
            total,
            total_wrong: tw,
            stubborn: rows.filter(x => x.state.wrong_count >= 5).length,
            avg_mastery: total ? Math.round(rows.reduce((s, x) => s + x.state.mastery, 0) / total * 10) / 10 : 0,
            dict_size: store.words.length,
          };
        }
        case 'cmd_leech_remove_many': {
          const set = new Set(args.words || []);
          const before = store.leechRemoved || (store.leechRemoved = new Set());
          set.forEach(w => before.add(w));
          return set.size;
        }
        case 'cmd_leech_export': {
          const rows = mockLeech();
          if (!rows.length) throw new Error('当前筛选条件下没有错词可导出');
          const fmt = args.format === 'csv' ? 'csv' : 'md';
          return {
            path: `(调试模式) 未实际写出 /错词本-${fmt === 'csv' ? 'x.csv' : 'x.md'}`,
            count: rows.length,
            format: fmt,
          };
        }

        /* ---- 知识图谱（调试模式） ---- */
        case 'cmd_graph_rels':
          return [
            { code: 'synonym', name: '同义' },
            { code: 'antonym', name: '反义' },
            { code: 'derived', name: '派生' },
            { code: 'related', name: '相关' },
            { code: 'hypernym', name: '上义' },
            { code: 'hyponym', name: '下义' },
          ];
        case 'cmd_graph_build':
          return mockEdges().length;
        case 'cmd_graph_clear':
          store.graphExtra = [];
          return mockEdges().length;
        case 'cmd_graph_stats': {
          const es = mockEdges();
          const words = new Set();
          es.forEach(e => { words.add(e.src); words.add(e.dst); });
          return {
            nodes: words.size, edges: es.length, rel_count: 6,
            top: mockTop([...words]),
            dict_size: store.words.length,
          };
        }
        case 'cmd_graph_view': {
          const all = mockEdges();
          const center = args.center || null;
          if (!center) {
            const words = new Set();
            all.forEach(e => { words.add(e.src); words.add(e.dst); });
            return {
              nodes: mockNodes([...words], all), edges: all,
              center: null, total_nodes: words.size, total_edges: all.length,
            };
          }
          const depth = Math.min(args.depth || 1, 2);
          let frontier = [center];
          const seenW = new Set([center]);
          const seenE = new Set();
          const picked = [];
          for (let d = 0; d < depth; d++) {
            const next = [];
            for (const w of frontier) {
              for (const e of all) {
                if (e.src !== w && e.dst !== w) continue;
                const k = `${e.src}|${e.dst}|${e.rel}`;
                if (seenE.has(k)) continue;
                seenE.add(k);
                picked.push(e);
                const other = e.src === w ? e.dst : e.src;
                if (!seenW.has(other)) { seenW.add(other); next.push(other); }
              }
            }
            frontier = next;
            if (!next.length) break;
          }
          const words = new Set();
          picked.forEach(e => { words.add(e.src); words.add(e.dst); });
          return {
            nodes: mockNodes([...words], picked), edges: picked,
            center, total_nodes: words.size, total_edges: picked.length,
          };
        }
        case 'cmd_graph_expand': {
          const w = args.word;
          const extra = store.graphExtra || (store.graphExtra = []);
          const mock = {
            abandon: [['synonym', 'desert'], ['synonym', 'forsake']],
            desert: [['synonym', 'abandon'], ['antonym', 'keep']],
            ability: [['synonym', 'capability'], ['hypernym', 'skill']],
          }[w] || [['related', 'hello']];
          let n = 0;
          for (const [rel, dst] of mock) {
            if (extra.some(e => e.src === w && e.dst === dst && e.rel === rel)) continue;
            extra.push({ src: w, dst, rel, weight: 0.8, source: 'llm' });
            n++;
          }
          return n;
        }
        case 'cmd_graph_search': {
          const words = new Set();
          mockEdges().forEach(e => { words.add(e.src); words.add(e.dst); });
          const q = String(args.q || '').trim().toLowerCase();
          const list = [...words].filter(w => !q || w.toLowerCase().includes(q));
          return list.slice(0, 30);
        }

        /* ---- 数据库维护（调试模式） ---- */
        case 'cmd_db_info':
          return {
            path: '(调试模式) 未连接真实数据库',
            data_dir: '(调试模式)',
            size_bytes: 0, size_text: '—', wal_bytes: 0, wal_text: '—',
            tables: [
              { name: 'words', label: '词库', rows: store.words.length },
              { name: 'study_state', label: '学习进度', rows: store.words.length },
              { name: 'review_log', label: '复习日志', rows: 0 },
              { name: 'word_edges', label: '知识图谱关系', rows: mockEdges().length },
            ],
            total_rows: store.words.length * 2 + mockEdges().length,
            export_dir: '(调试模式)', models_dir: '(调试模式)',
            now: new Date().toISOString().slice(0, 16).replace('T', ' '),
          };
        case 'cmd_db_maintain':
          if (args.action === 'check') {
            return { action: 'check', ok: true, message: '（调试模式）数据库结构完整，没有发现问题',
                     before_text: '—', after_text: '—' };
          }
          if (args.action === 'backup') {
            return { action: 'backup', ok: true, message: '（调试模式）未实际备份',
                     before_text: '—', after_text: '—', path: '(调试模式)/backups/wordwise.db' };
          }
          return { action: 'vacuum', ok: true, message: '（调试模式）整理完成，数据库已经很紧凑',
                   before_text: '—', after_text: '—' };
        case 'cmd_open_dir': return null;
        case 'cmd_open_url': return null;

        /* ---- 数据与模型的存放位置（调试模式） ---- */
        case 'cmd_storage_info':
          return {
            data_dir: '(调试模式)\\data',
            source: 'beside_exe',
            source_label: '软件所在目录（默认，跟着程序走）',
            travels_with_app: true,
            from_env: false,
            pointer_file: '(调试模式)\\location.txt',
            portable: false, portable_file: '(调试模式)\\portable.txt',
            env_data_dir: '',
            exe_dir: '(调试模式)',
            data_bytes: 0, data_text: '—', data_files: 0,
            db_path: '(调试模式)\\data\\wordwise.db',
            db_bytes: 0, db_text: '—', db_wal_text: '—',
            models_dir: '(调试模式)\\data\\models',
            models_dir_default: '(调试模式)\\data\\models',
            models_custom: false,
            models_bytes: 0, models_text: '—', models_files: 0,
            models_pointer_file: '(调试模式)\\data\\models_dir.txt',
            models_env: '',
            models_installed: [],
            models_missing: [
              { id: 'qwen3-1.7b', name: 'Qwen3 1.7B（推荐）',
                file: 'Qwen3-1.7B-Q4_K_M.gguf', size_bytes: 1107400000,
                size_text: '1056 MB', installed: false },
              { id: 'qwen3-0.6b', name: 'Qwen3 0.6B（极速）',
                file: 'Qwen3-0.6B-Q4_K_M.gguf', size_bytes: 396700000,
                size_text: '378 MB', installed: false },
            ],
            engine_dir: '(调试模式)\\vendor\\llama',
            engine_ready: true, engine_from_bundle: true,
            engine_bytes: 0, engine_files: 0, engine_text: '—',
            total_text: '—',
            drives: [
              { letter: 'C', root: 'C:\\', kind: 'fixed', kind_label: '本地磁盘',
                system: true, total_bytes: 0, free_bytes: 0, free_text: '—' },
              { letter: 'D', root: 'D:\\', kind: 'fixed', kind_label: '本地磁盘',
                system: false, total_bytes: 0, free_bytes: 0, free_text: '—' },
            ],
          };
        case 'cmd_set_data_dir':
        case 'cmd_set_models_dir':
          return {
            ok: true, changed: true, restart_required: true,
            migrated: args.migrate !== false,
            copied_files: 0, copied_text: '0 B',
            path: args.path || '',
            message: '（调试模式）未实际改动任何目录。',
          };
        case 'cmd_restart_app': return { ok: true };

        /* ---- 本地大模型一键部署（调试模式） ---- */
        case 'cmd_local_llm_models':
          return [
            { id: 'qwen3-1.7b', name: 'Qwen3 1.7B（推荐）',
              note: '中文与多语言理解好，讲词、造句、翻译兜底都够用。',
              file: 'Qwen3-1.7B-Q4_K_M.gguf', size_bytes: 1107400000, recommended: true },
            { id: 'qwen3-0.6b', name: 'Qwen3 0.6B（极速）',
              note: '只有 400 MB，老机器也能流畅跑。',
              file: 'Qwen3-0.6B-Q4_K_M.gguf', size_bytes: 396700000, recommended: false },
            { id: 'qwen2.5-1.5b', name: 'Qwen2.5 1.5B（稳定）',
              note: '上一代模型，指令遵循非常稳、不说怪话。',
              file: 'qwen2.5-1.5b-instruct-q4_k_m.gguf', size_bytes: 1117300000, recommended: false },
          ];
        case 'cmd_local_llm_status':
          return {
            engine: { ready: false, dir: null, bundled_dir: '(调试模式)/vendor/llama',
                      bundled_ready: false, downloaded_dir: '(调试模式)/engine',
                      tag: 'b11414', asset: 'llama-b11414-bin-win-vulkan-x64.zip',
                      exe: 'llama-server.exe' },
            models: { dir: '(调试模式)/models', installed: [], detail: [] },
            server: { running: false, port: 0, model: '', using_ours: false,
                      base_url: store.config.llm.base_url },
            threads: 4, cpu: 8, app_dir: '(调试模式)',
            engine_needs_download: true,
            auto_start: !!store.autoStartLocal,
          };
        case 'cmd_set_local_llm_auto':
          store.autoStartLocal = !!args.enabled;
          return { ok: true, enabled: store.autoStartLocal, message: '（调试模式）已更新自动启动设置' };
        case 'cmd_local_llm_install_engine':
        case 'cmd_local_llm_install_model':
          throw new Error('（调试模式）不会真的下载，请在桌面程序里操作');
        case 'cmd_local_llm_start':
          store.config.llm.base_url = 'http://127.0.0.1:18080/v1';
          return { ok: true, port: 18080, base_url: store.config.llm.base_url,
                   model: 'Qwen3 1.7B', threads: 4,
                   message: '（调试模式）Qwen3 1.7B 已启动（127.0.0.1:18080，4 线程）' };
        case 'cmd_local_llm_stop':
          // 与后端一致：装了引擎/模型后停服会把 AI 地址还回原样
          return { ok: true, restored: true, message: '（调试模式）本地模型服务已停止，AI 地址已还原' };
        case 'cmd_local_llm_probe':
          return { ok: false, message: '（调试模式）本地模型服务未运行' };
        case 'cmd_local_llm_cancel': return null;
        case 'cmd_local_llm_remove_model':
          return '（调试模式）未实际删除';

        /* ---- 翻译（调试模式） ---- */
        case 'cmd_translate_langs':
          return [
            { code: 'auto', name: '自动检测', self_name: '自动检测' },
            { code: 'zh', name: '中文', self_name: '简体中文' },
            { code: 'en', name: '英语', self_name: 'English' },
            { code: 'ja', name: '日语', self_name: '日本語' },
            { code: 'ko', name: '韩语', self_name: '한국어' },
            { code: 'fr', name: '法语', self_name: 'Français' },
            { code: 'de', name: '德语', self_name: 'Deutsch' },
            { code: 'es', name: '西班牙语', self_name: 'Español' },
            { code: 'ru', name: '俄语', self_name: 'Русский' },
            { code: 'pt', name: '葡萄牙语', self_name: 'Português' },
            { code: 'it', name: '意大利语', self_name: 'Italiano' },
          ];
        case 'cmd_translate_status':
          return { online_ready: true, cooldown_secs: 0 };
        case 'cmd_swap_direction': {
          const from = store.config.source_lang || 'auto';
          const to = store.config.target_lang || 'en';
          let nf, nt;
          if (from === 'auto') { nf = to; nt = to === 'zh' ? 'en' : 'zh'; }
          else { nf = to; nt = from; }
          store.config.source_lang = nf;
          store.config.target_lang = nt;
          return [nf, nt];
        }
        case 'cmd_translate': {
          const from = args.from || store.config.source_lang || 'auto';
          const to = args.to || store.config.target_lang || 'en';
          const key = args.text + '|' + to;
          const hit = MOCK_TRANS[key];
          const detected = from === 'auto' ? mockGuessLang(args.text) : from;
          const text = hit || `（调试模式）${args.text} 的${mockLangName(to)}译文`;

          // 与后端一致：写历史（同方向同原文只占一行），并把 record_id 回传，
          // 否则「收藏」按钮会一直禁用。
          const list = store.transHistory || (store.transHistory = []);
          let rec = list.find(x => x.src_text === args.text && x.target_lang === to);
          if (rec) {
            rec.dst_text = text; rec.source_lang = detected; rec.engine = 'youdao';
          } else {
            rec = {
              id: list.length ? Math.max(...list.map(x => x.id)) + 1 : 1,
              source_lang: detected, target_lang: to,
              src_text: args.text, dst_text: text,
              engine: 'youdao', favorite: false, created_at: Date.now(),
            };
            list.unshift(rec);
          }

          return {
            source: args.text, text,
            alternatives: [],
            from: detected, to,
            phonetic: to === 'ja' ? 'konnichiwa' : '',
            tts_url: '', engine: 'youdao', from_cache: false,
            record_id: rec.id, favorite: rec.favorite,
            // 需求 4：真实后端会带上有道 basic.explains（「词性 + 释义」的行）
            // 与 web（网络释义）。调试环境里没有这些数据，但界面结构必须能验，
            // 所以给常用词备一份**真实内容**，其它词就老老实实不出这块。
            dict: (MOCK_DICT[key] && { phonetic: '', explains: MOCK_DICT[key].explains }) || undefined,
            web: (MOCK_DICT[key] && MOCK_DICT[key].web) || [],
          };
        }
        case 'cmd_translate_ai':
          return `## ${mockLangName('zh')}（调试模式）\n\n当前运行在浏览器调试环境中，未连接本地大模型服务。\n\n- 请通过 Tauri 桌面程序启动以使用 AI 增强`;
        case 'cmd_translate_history': {
          const all = store.transHistory || [];
          return args.onlyFavorite ? all.filter(x => x.favorite) : all;
        }
        case 'cmd_translate_favorite': {
          const r = (store.transHistory || []).find(x => x.id === args.id);
          if (r) r.favorite = !!args.on;
          return null;
        }
        case 'cmd_translate_delete':
          store.transHistory = (store.transHistory || []).filter(x => x.id !== args.id);
          return null;
        case 'cmd_translate_clear': {
          const before = (store.transHistory || []).length;
          store.transHistory = args.keepFavorite === false
            ? [] : (store.transHistory || []).filter(x => x.favorite);
          return before - store.transHistory.length;
        }

        case 'cmd_ai_explain':
        case 'cmd_ai_explain_sync': {
          const t = '## 调试模式\n\n当前运行在浏览器调试环境中，未连接本地大模型服务。\n\n- 请通过 Tauri 桌面程序启动以使用 AI 讲解';
          // 调试模式没有后端检索。给两条**真实可用**的辞书链接占位，
          // 否则「联网参考资料」那块 UI 在调试环境下永远看不到、也点不动。
          // 非流式版（批量导出）在后端刻意不联网，这里也保持一致。
          const refs = cmd === 'cmd_ai_explain'
            ? [
                { title: 'Cambridge Dictionary', engine: '调试模式',
                  url: `https://dictionary.cambridge.org/dictionary/english/${encodeURIComponent(args.word || '')}`,
                  snippet: '（调试模式占位）联网资料的标题与摘要会显示在这里。' },
                { title: 'Merriam-Webster', engine: '调试模式',
                  url: `https://www.merriam-webster.com/dictionary/${encodeURIComponent(args.word || '')}`,
                  snippet: '（调试模式占位）实时检索到的内容仅供 AI 补充例句与派生变形。' },
              ]
            : [];
          return { word: args.word || '', text: t, original: t,
                   lang: (store.config.explain_lang || 'zh'), translated: false,
                   web_refs: refs };
        }
        // 切换讲解语言：立刻记住，下次讲解生效
        case 'cmd_set_explain_lang':
          store.config.explain_lang = args.lang; return args.lang;
        case 'cmd_translate_text':
          return { text: args.text || '', original: args.text || '',
                   lang: args.lang || store.config.explain_lang || 'zh', translated: false,
                   note: '（调试模式）未连接模型，未实际翻译' };
        case 'cmd_ai_generate_entry': throw new Error('（调试模式）无法生成词条');

        // AI 讲解存档（调试模式：进程内 Map，key 与后端主键同构）
        case 'cmd_save_explain': {
          const k = Mock.explainKey(args);
          const prev = store.explains.get(k) || { entry_json: '', saved: false };
          const row = {
            word: (args.word || '').toLowerCase(),
            lang: args.lang || '', explain_lang: args.explainLang || '',
            text: args.text || '', original: args.original || '',
            translated: !!args.translated,
            entry_json: prev.entry_json, saved: prev.saved, updated_at: Date.now(),
          };
          store.explains.set(k, row);
          return row;
        }
        case 'cmd_get_explain':
          return store.explains.get(Mock.explainKey(args)) || null;
        case 'cmd_list_explains': {
          const w = (args.word || '').toLowerCase();
          return [...store.explains.values()].filter((r) => r.word === w);
        }
        case 'cmd_search_explains': {
          const q = (args.query || '').toLowerCase();
          return [...store.explains.values()].filter(
            (r) => r.word.includes(q) || (r.text || '').toLowerCase().includes(q));
        }
        case 'cmd_delete_explain':
          return store.explains.delete(Mock.explainKey(args)) ? 1 : 0;
        case 'cmd_clear_explains': {
          const n = store.explains.size;
          store.explains.clear();
          return n;
        }
        case 'cmd_explain_count':
          return store.explains.size;
        case 'cmd_explain_to_entry':
          throw new Error('（调试模式）未连接模型，无法把讲解整理成词条');

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
          const s = store.sessions[slotOf(args.kind)];
          if (!s || s.idx >= s.queue.length) return null;
          const e = s.queue[s.idx];
          const m = args.mode || 'EnToZh';
          const ex = (e.senses[0].examples[0] || {}).text || '';
          const masked = ex.replace(new RegExp(e.word, 'i'), '____');
          const showEx = m === 'ExToZh' || m === 'ExPickWord';
          const want = Math.min(8, Math.max(2, Number(store.config.study.option_count) || 4));
          return {
            prompt: showEx ? masked : e.word,
            answer: (m === 'ExToZh') ? e.senses[0].definition : e.word,
            entry: e, options: store.words.filter(x => x.word !== e.word).slice(0, want - 1)
              .map(x => (m === 'ExToZh' ? x.senses[0].definition : x.word)),
            mode: m, is_leech: false,
            example_masked: masked, example_raw: ex, example_translation: '（示例译文）',
            spell_hint: e.word.split('').map(() => '_').join(' '), audio: '',
            index: s.idx + 1, total: s.queue.length, correct_count: s.correct, wrong_count: s.wrong,
          };
        }

      // 在线更新（调试模式：假装已经有一个新版本，方便看「有更新」的样子）
      case 'cmd_check_update':
        return {
          current: '0.43.0', repo: 'DedalusArtin/wordwise',
          latest: '0.43.0', tag: 'v0.43.0', has_update: true, prerelease: false,
          name: 'WordWise v0.43.0',
          notes: '## 调试模式\n\n- 这是一条示例更新说明\n- 真实数据来自 GitHub Releases',
          published_at: new Date().toISOString().slice(0, 10),
          page_url: 'https://github.com/DedalusArtin/wordwise/releases',
          asset: {
            name: 'WordWise-Setup-0.43.0.exe', url: 'https://example.com/WordWise-Setup-0.43.0.exe',
            size: 19000000, size_text: '18 MB', kind: 'installer', installable: true, digest: '',
          },
          assets: [{
            name: 'WordWise-Setup-0.43.0.exe', url: 'https://example.com/WordWise-Setup-0.43.0.exe',
            size: 19000000, size_text: '18 MB', kind: 'installer', installable: true, digest: '',
          }, {
            name: 'WordWise-0.43.0-portable.zip', url: 'https://example.com/WordWise-0.43.0-portable.zip',
            size: 37000000, size_text: '35 MB', kind: 'portable', installable: false, digest: '',
          }],
          checked_at: Math.floor(Date.now() / 1000),
          skipped: (store.config.skip_update_version || '') === '0.43.0',
          note: null,
          source: '调试模式（假数据，不联网）',
        };
      case 'cmd_download_update':
        return {
          ok: true, already: false,
          path: '(调试模式)\\data\\updates\\WordWise-Setup-0.43.0.exe',
          size: 19000000, size_text: '18 MB', sha256: 'deadbeefdeadbeef', version: '0.43.0',
          message: '调试模式：假装下载完成',
        };
      case 'cmd_update_cancel':
        return null;
      case 'cmd_run_update':
        return { ok: true, path: args.path, message: '调试模式：不会真的启动安装程序' };
      case 'cmd_open_update_dir':
        return { ok: true, path: '(调试模式)\\data\\updates' };
      case 'cmd_update_prefs':
        return {
          check_on_start: store.config.check_update_on_start !== false,
          skip_version: store.config.skip_update_version || '',
        };
      case 'cmd_set_update_prefs':
        if (args.checkOnStart != null) store.config.check_update_on_start = !!args.checkOnStart;
        if (args.skipVersion != null) store.config.skip_update_version = String(args.skipVersion);
        return {
          ok: true,
          check_on_start: store.config.check_update_on_start !== false,
          skip_version: store.config.skip_update_version || '',
          message: '已保存（调试模式）',
        };

      // ---- 朗读（TTS）：调试模式下假装引擎没装，好把「下载引导」这套 UI 走通 ----
      case 'cmd_tts_status': {
        const t = store.config.tts || (store.config.tts = { engine: 'auto', voice_local: '', voice_online: '', rate: 0.95 });
        return {
          engine_ready: false,
          engine_bytes: 22477236,
          engine_size_text: '21 MB',
          engine_path: '(调试模式)\\tts\\piper\\piper.exe',
          voices_dir: '(调试模式)\\tts\\voices',
          models_dir: '(调试模式)',
          installed: [],
          voices: [
            { id: 'en_US-amy-medium', label: 'Amy · 美式女声', lang: 'en', accent: 'us', gender: 'female', quality: 'medium', bytes: 63201294, size_text: '60 MB', preset: true, note: '', installed: false },
            { id: 'en_GB-alba-medium', label: 'Alba · 英式女声', lang: 'en', accent: 'gb', gender: 'female', quality: 'medium', bytes: 63201294, size_text: '60 MB', preset: false, note: '', installed: false },
            { id: 'zh_CN-huayan-medium', label: '华言 · 中文女声', lang: 'zh', accent: '', gender: 'female', quality: 'medium', bytes: 63201294, size_text: '60 MB', preset: false, note: '', installed: false },
            { id: 'zh_CN-huayan-x_low', label: '华言 · 中文女声（小体积）', lang: 'zh', accent: '', gender: 'female', quality: 'x_low', bytes: 20628813, size_text: '20 MB', preset: false, note: '音素表较小，个别汉字可能读不出（追求准确请选上一档）', installed: false },
          ],
          config: t,
        };
      }
      case 'cmd_tts_install_engine':
        return { ok: true, already: false, files: 42, message: '调试模式：不会真的下载引擎' };
      case 'cmd_tts_install_voice':
        return { ok: true, id: args.voiceId, message: '调试模式：不会真的下载语音包' };
      case 'cmd_tts_remove_voice':
        return { ok: true, message: '已删除（调试模式）' };
      case 'cmd_tts_cancel':
        return true;
      case 'cmd_tts_prefs':
        return store.config.tts || { engine: 'auto', voice_local: '', voice_online: '', rate: 0.95 };
      case 'cmd_set_tts_prefs': {
        const t = store.config.tts || (store.config.tts = { engine: 'auto', voice_local: '', voice_online: '', rate: 0.95 });
        if (args.engine != null) t.engine = String(args.engine);
        if (args.voiceLocal != null) t.voice_local = String(args.voiceLocal);
        if (args.voiceOnline != null) t.voice_online = String(args.voiceOnline);
        if (typeof args.rate === 'number') t.rate = args.rate;
        return { ok: true };
      }
      case 'cmd_tts_clear_cache':
        return { ok: true, removed: 0, freed: '0 KB' };
      case 'cmd_tts_speak':
        // 调试模式没有引擎 → 明确抛错，让前端走系统语音回退这条路径
        throw new Error('调试模式没有本地语音引擎');
      default:
        if (cmd.startsWith('sidebar_') || cmd === 'main_show') return true;
        return null;
      }
    },

    /** 讲解存档的主键，与后端 `explain_store` 的 (word, lang, explain_lang) 同构。 */
    explainKey(args) {
      // 与后端一致：lang / explain_lang 缺省时回落到配置里的当前值，
      // 否则「保存时不传、读取时也不传」会算出两个不同的 key 而永远读不到。
      const lang = args.lang || store.config.target_lang || 'en';
      const el = args.explainLang || store.config.explain_lang || 'zh';
      return [(args.word || '').trim().toLowerCase(), lang, el].join('|');
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
  // 窗口控制（标题栏自绘三键用）
  WIN, currentWindowLabel,
};
