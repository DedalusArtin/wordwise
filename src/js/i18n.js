/**
 * 界面多语言（需求：在系统语言设置里选择界面语言，并统一管理 AI 输出语言）。
 *
 * ## 关键取舍：字典的**键就是中文原文**
 *
 * 不是在配置之外再发明一套 `dict.xxx.yyy` 的 key。理由很实际：
 * 这个项目已经有 ~29k 字、两千多行硬编码中文文案分布在 index.html 与 16 个
 * js 文件里。再发明 key 意味着：
 *   1. 每一处调用都要改写（`加入词库` → `t('lookup.add_to_book')`）；
 *   2. 翻译遗漏时界面显示的是 **`t('lookup.add_to_book')`** 这种鬼东西，
 *      而不是至少还能看懂的「加入词库」。
 * 用中文原文做键，遗漏的代价退化成「这句没翻」，永远不会更糟。
 *
 * ## 落回界面：为什么带一层 DOM 自动局部刷新
 *
 * 文案大量写在模板字符串里（`innerHTML = \`…<button>加入词库</button>…\``）。
 * 逐个调用点改包裹函数既慢又容易漏。所以这里用 MutationObserver 统一接管：
 * 任何新挂到 DOM 上的节点，其内部文本与下列属性都自动过一遍字典：
 * `title` / `placeholder` / `aria-label`。
 *
 * 两种键：
 *   - 精确键：`'加入词库' → 'Add to Wordbook'`
 *   - 模板键：含 `{n}` 的键会把 `{n}` 编译成捕获组，用来对付
 *     `共 12 条`、`第 3 / 20 题` 这种把数字拼进文案里的情况。
 *     只在文本里含数字时才尝试匹配，避免给每条文本都跑一遍全部模板键。
 *
 * ## 切语言为什么要留原文备份
 *
 * 「从英文翻回中文」是不可逆的——同一个英文词可能对应多句原文，切两次就把
 * 原文彻底丢了。所以每个被替换过的文本节点 / 属性都留了一份原文，切语言时
 * 永远是「回到原文、重译成目标语言」。
 */
(function () {
  'use strict';

  const LS_KEY = 'ww.ui_lang';

  /** 可选界面语言。id 用 BCP-47，和后端 `AppConfig::ui_lang` 同一个值域。 */
  const LANGS = [
    { id: 'zh-CN', name: '简体中文' },
    { id: 'en', name: 'English' },
  ];

  /**
   * 英文文案表。键 = 中文原文，值 = 英文译文。
   *
   * 维护约定：
   *   - 只在**界面上真会出现**的字符串上建条目，别把注释里的中文也搬进来；
   *   - 值是纯文本，不含 HTML —— 这条不能有例外，否则渲染方式一变就会产生注入；
   *   - 含 `{n}` 的键是模板键，匹配时会把 `{n}` 当「任意内容」处理。
   */
  const EN = {
    // ---- 全局 / 窗口 ----
    '单词随笔': 'WordWise',
    '最小化': 'Minimize',
    '最大化': 'Maximize',
    '关闭': 'Close',
    '取消': 'Cancel',
    '确定': 'OK',
    '保存': 'Save',
    '删除': 'Delete',
    '编辑': 'Edit',
    '刷新': 'Refresh',
    '搜索': 'Search',
    '返回': 'Back',
    '重置': 'Reset',
    '导入': 'Import',
    '导出': 'Export',
    '复制': 'Copy',
    '更多': 'More',
    '全部': 'All',
    '无': 'None',
    '是': 'Yes',
    '否': 'No',
    '启用': 'Enabled',
    '已启用': 'Enabled',
    '禁用': 'Disabled',
    '加载中…': 'Loading…',
    '暂无数据': 'No data',
    '操作成功': 'Done',
    '设置': 'Settings',

    // ---- 侧边导航 ----
    '查词': 'Lookup',
    '翻译': 'Translate',
    '背诵': 'Study',
    '学习': 'Learn',
    '词库': 'Wordbook',
    '词图': 'Word Graph',
    '维护': 'Maintenance',
    '关于': 'About',
    '更新': 'Update',

    // ---- 查词页 ----
    '加入词库': 'Add to Wordbook',
    'AI 讲解': 'AI Explain',
    '查看详情卡': 'Details',
    '释义': 'Definitions',
    '参考释义': 'Original-language Definitions',
    '单词变形': 'Inflections',
    '相关词': 'Related Words',
    // 需求 5：同族派生词（happy → happiness…）
    '同族派生词': 'Word Family',
    '本地词库已收录': 'In your dictionary',
    '记忆法': 'Mnemonic',
    '例句': 'Examples',
    '数据来源': 'Sources',
    '暂无释义，可点击「AI 讲解」让本地模型生成。': 'No definition yet — click "AI Explain" to generate one with the local model.',
    '对应词汇': 'Counterpart Words',
    '主译': 'Primary',
    '其他译法': 'Also used',
    '发音': 'Pronounce',
    '朗读': 'Speak',
    '数据来源：': 'Sources: ',
    '知识点': 'Key Points',
    '词根词缀': 'Roots & Affixes',
    '网络释义': 'Web Definitions',
    // 需求 4：翻译结果里的「缩略多义项」区块
    '词性与释义': 'Parts of Speech & Meanings',
    '展开全部': 'Show all',
    '详细讲解': 'Full explanation',
    '语言': 'Language',
    '界面语言': 'Interface Language',
    '讲解语言': 'AI Output Language',
    '设置已保存': 'Settings saved',
    '保存失败：': 'Failed to save: ',

    // ---- 学习 / 背诵 ----
    '天': 'd', '对': '✓', '错': '✗', '个': '',
    '隐藏': 'Hide', '放大': 'Zoom in', '缩小': 'Zoom out', '复位': 'Reset view',
    '不限': 'No limit', '拼写': 'Spelling', '累计': 'Total',
    '答对': 'Correct', '答错': 'Wrong', '本轮': 'This round',
    '查询': 'Look up', '提问': 'Ask', '必应': 'Bing',
    '原文': 'Original', '清空': 'Clear', '译文': 'Translation',
    '收藏': 'Favorite', '重译': 'Re-translate', '实线': 'Solid', '虚线': 'Dashed',
    '四级': 'CET-4', '六级': 'CET-6', '考研': 'Kaoyan', '雅思': 'IELTS',
    '托福': 'TOEFL', '高考': 'Gaokao', '其他': 'Other', '排序': 'Sort',
    '明暗': 'Appearance', '语速': 'Speed', '试听': 'Preview',
    '显示': 'Show', '温度': 'Temperature', '明文': 'Plain text', '默认': 'Default',
    '整理': 'Vacuum', '备份': 'Backup', '变形': 'Inflections',
    '内容': 'Content', '源语言': 'Source language', '目标语言': 'Target language',
    '互换方向': 'Swap direction',
    '错词本': 'Troublesome Words', '待复习': 'Due for review',
    '常错词': 'Frequently missed', '已掌握': 'Mastered', '保持住': 'Keep it up',
    '上一个': 'Previous', '待翻译': 'Untranslated', '候选词': 'Candidates',
    '实心点': 'Filled dot', '空心点': 'Hollow dot',
    '上一页': 'Previous page', '下一页': 'Next page',
    '最少错': 'Fewest errors', '错误率': 'Error rate', '掌握度': 'Mastery',
    '字母序': 'Alphabetical', '主题色': 'Accent color', '模型名': 'Model name',
    '朗读题面': 'Read the question', '搜索引擎': 'Search engine',
    '清空输入': 'Clear input', '朗读译文': 'Read translation',
    '复制译文': 'Copy translation', '朗读单词': 'Read the word',
    '快速背词': 'Quick review', '知识图谱': 'Knowledge graph',
    '复习计划': 'Review plan', '学习统计': 'Statistics',
    '检测中…': 'Checking…', '演示模式': 'Demo mode', '退出演示': 'Exit demo',
    '中文释义': 'Chinese definitions', '原文释义': 'Original-language definitions',
    '看英选中': 'EN → pick CN', '看中选英': 'CN → pick EN',
    '例句选义': 'Example → pick meaning', '例句识词': 'Example → pick word',
    '听音拼写': 'Listen and spell',
    '开始背诵': 'Start review', '今日到期': 'Due today', '强化记忆': 'Intensive practice',
    '连续学习': 'Study streak', '背诵词库': 'Book to review', '每轮题量': 'Words per round',
    '结束本轮': 'End this round', '本轮已背': 'Reviewed this round',
    '最近背过': 'Recently reviewed', '收起右栏': 'Collapse the right panel',
    // 分词库容器（已背列表按词库分组）用到的三条新文案
    '全部词库': 'All wordbooks',
    '未入词库': 'Not in any wordbook',
    '{n} 词': '{n} words',
    // 固定专栏的空态占位（有道式栏目：无数据也保留位置，点名 AI 补齐）
    '暂无释义 —— 点下方「AI 讲解」可生成完整词条。':
      'No definitions yet — click "AI Explain" below to generate the full entry.',
    '暂无例句 —— 点下方「AI 讲解」可生成例句与词形变化。':
      'No examples yet — click "AI Explain" below to generate examples and inflections.',
    '暂无变形数据 —— 「AI 讲解」会一并生成。':
      'No inflection data — "AI Explain" generates it together with the entry.',
    '强制刷新': 'Force refresh', '本地模型': 'Local model', '在线搜索': 'Web search',
    '历史记录': 'History', '例句生成': 'Generate examples',
    '我的词库': 'My Wordbook', '在线词库': 'Online Wordbook',

    // ---- 语言名（下拉里的选项）----
    '英语': 'English', '中文': 'Chinese', '日语': 'Japanese', '韩语': 'Korean',
    '法语': 'French', '德语': 'German', '俄语': 'Russian', '西班牙语': 'Spanish',
    // 语种统计里会列出**词库里真实存在**的语种，不止上面那几个
    '意大利语': 'Italian', '葡萄牙语': 'Portuguese', '阿拉伯语': 'Arabic',
    '泰语': 'Thai', '印地语': 'Hindi', '希伯来语': 'Hebrew', '希腊语': 'Greek',

    // ---- 词库 / 学习 / 统计 ----
    '单词列表': 'Word list', '多级词库': 'Nested book', '新建词库': 'New book',
    '全部分类': 'All categories', '强化背诵': 'Intensive practice', '最近答错': 'Recently missed',
    '移出列表': 'Remove from list', '重置筛选': 'Reset filters', '开始复习': 'Start review',
    '保存设置': 'Save settings', '显示音标': 'Show phonetics', '显示例句': 'Show examples',
    '每日上限': 'Daily limit', '恢复默认': 'Restore defaults', '示例数据': 'Sample data',
    '记忆辅助内容': 'Mnemonic content', '显示单词变形': 'Show inflections',
    '显示词根词缀 / 记忆法': 'Show roots & mnemonics', '显示相关词': 'Show related words',
    '启用 AI 讲解': 'Enable AI explanation', '答错自动弹出详情卡': 'Open details on a wrong answer',
    '归类（考试类型）': 'Category (exam type)', '高考 / 高中': 'Gaokao / High school',
    '日语 JLPT': 'Japanese JLPT', '当前词库的语言': 'Language of this book',
    '从指定词库出题': 'Quiz from a specific book', '选择要背诵的词库': 'Choose a book to review',
    '只背强化记忆的常错词': 'Review only intensive-practice words',
    '开始新的一轮': 'Start a new round', '用这些词直接开一轮': 'Start a round with these words',
    '准备开始今天的学习': 'Ready for today’s review',
    '已背过的词': 'Already reviewed', '已背过的单词': 'Words already reviewed',
    '重新拉取最近背过的单词': 'Reload recently reviewed words',
    '朗读上一个单词': 'Read the previous word', '朗读最近背过的单词': 'Read recently reviewed words',
    '点击查看完整释义': 'Click to see the full entry',
    '点开可回看释义，随时听发音': 'Click to review the entry and replay audio',
    '记忆周期表': 'Retention intervals', '掌握度构成': 'Mastery breakdown',
    '掌握度分布': 'Mastery distribution', '近 14 天学习量': 'Words studied · last 14 days',
    '近 14 天答题量': 'Answers · last 14 days', '未来 14 天安排': 'Plan · next 14 days',
    '现在该背哪些（到期清单）': 'What to review now (due list)',

    // ---- 设置面板分区与按钮 ----
    '外观与主题': 'Appearance & theme', '切换深色 / 浅色': 'Toggle dark / light',
    '网络与代理': 'Network & proxy', '直连，不使用代理': 'Direct connection, no proxy',
    '自动使用系统代理': 'Use the system proxy', '启用代理': 'Enable proxy',
    '代理地址（留空 = 自动检测）': 'Proxy address (blank = auto-detect)',
    '检测网络': 'Check network', '应用并重连': 'Apply and reconnect',
    '请求超时（秒）': 'Request timeout (sec)', '单源查词超时（秒）': 'Per-source lookup timeout (sec)',
    '语言与搜索': 'Language & search', '默认搜索引擎（需求 6）': 'Default search engine',
    '必应国际': 'Bing (international)', '百度（受反爬限制）': 'Baidu (anti-bot limits apply)',
    'DuckDuckGo（需代理）': 'DuckDuckGo (needs proxy)', '360 搜索': '360 Search',
    '词典与翻译源（可扩展小语种）': 'Dictionary & translation sources',
    '新增源': 'Add source', '恢复内置': 'Restore built-ins',
    '清空缓存': 'Clear dictionary cache', '完整性检查': 'Integrity check',
    '数据库': 'Database', '打开数据目录': 'Open data folder',
    '更换数据目录': 'Change data folder', '跟着软件走': 'Follow the app folder',
    '新目录（完整路径，要带盘符）': 'New folder (full path, drive letter required)',
    '把现有文件一起搬过去（推荐）': 'Move existing files too (recommended)',
    '清掉指定，回到「跟着软件目录走」': 'Clear the override, follow the app folder again',
    '数据与模型的存放位置': 'Where data and models are stored',
    '关于与更新': 'About & updates', '当前版本': 'Current version',
    '检查更新': 'Check for updates', '打开发布页': 'Open release page',
    '有新版本可用': 'A new version is available', '保存并生效': 'Save and apply',

    // ---- AI 服务 / 本地模型 ----
    'AI 服务': 'AI service', '服务来源': 'Service source', '服务地址': 'Service URL',
    '在线 API': 'Online API', '本机模型': 'Local model', '本机服务可留空': 'Blank for a local service',
    '最大输出长度': 'Max output length', '测试连接': 'Test connection',
    '打开模型目录': 'Open models folder', '本地大模型一键部署': 'One-click local LLM setup',
    '安装引擎': 'Install engine', '检测本机': 'Auto-detect',
    '启动时自动拉起本地服务': 'Start the local service automatically',
    'AI 不按讲解语言输出时自动翻译': 'Translate automatically when AI ignores the output language',
    '清理讲解存档': 'Clear saved explanations', '占位符说明：': 'Placeholders: ',
    '翻译模板（留空用内置模板）': 'Translation template (blank = built-in)',

    // ---- 朗读 / 发音 ----
    '朗读与发音': 'Speech & pronunciation', '朗读引擎': 'Speech engine',
    '系统语音': 'System voices', '系统音色': 'System voice',
    '系统语音 · 内置兜底，音质一般': 'System voices · built-in fallback, average quality',
    '本地神经语音': 'Local neural voices',
    '本地神经语音 · 离线、音质好': 'Local neural voices · offline, high quality',
    '本地语音包': 'Local voices', '下载语音引擎': 'Download the speech engine',
    '清理语音缓存': 'Clear speech cache', '永远优先播录音': 'Always prefer real recordings',
    '词典自带真人录音时': 'When the dictionary ships a real recording',

    // ---- 翻译页 / 图谱 / 词库 / 维护 ----
    'AI 增强': 'AI Enhance', '多版本译文': 'Multiple versions', 'AI 发散': 'AI Explore',
    'AI 释义与语境': 'AI meaning & context', '自动润色': 'Auto-polish',
    '直译 / 口语 / 书面三种风格': 'Literal / conversational / formal',
    '把译文改得更地道自然': 'Make the translation sound natural',
    '用目标语言造句并附译文': 'Write sentences in the target language with translations',
    '译文会显示在这里。': 'The translation appears here.',
    '还没有翻译记录。': 'No translation history yet.',
    '还没有关系数据。': 'No relation data yet.',
    '收藏这条翻译': 'Favorite this translation', '清空历史': 'Clear history',
    '清空历史，但保留已收藏的条目': 'Clear history but keep favorites',
    '构建图谱': 'Build the graph', '关系类型': 'Relation type', '当前视图': 'Current view',
    '列出词库里关系最多的词': 'List the most connected words',
    '点一下可隐藏／显示某类关系。': 'Click to hide or show a relation type.',
    '批量导入单词': 'Bulk import words', '批量导入': 'Bulk import',
    '从文件导入': 'Import from a file', '开始导入': 'Start import',
    '导入词库': 'Import a wordbook', '词库名称': 'Book name',
    '导入后立即联网补全释义': 'Fetch definitions right after importing',
    '导出备份': 'Export backup', '导入备份': 'Import backup',
    '导出 CSV': 'Export CSV', '导出 MD': 'Export Markdown',
    '选择文件…': 'Choose file…', '隐藏到托盘': 'Hide to tray',
    '打开主界面': 'Open main window', '查词侧边栏': 'Lookup sidebar',
    '打开侧边栏': 'Open sidebar', '侧边栏始终置顶': 'Keep sidebar always on top',
    '侧边栏宽度（px）': 'Sidebar width (px)', '最大化 / 还原': 'Maximize / Restore',
    '打开完整设置': 'Open full settings', '打开所在文件夹': 'Open containing folder',
    '显示示例数据': 'Show sample data', '载入示例词库': 'Load the sample wordbook',
    '不会写入数据库': 'Nothing is written to the database',
    '共 0 个单词': '0 words', '第 1 页': 'Page 1',
    '错误率 ↓': 'Error rate ↓', '掌握度 ↑': 'Mastery ↑', '错误次数 ↓': 'Errors ↓',
    '输入单词即可查词': 'Type a word to look it up',
    '输入单词开始查询': 'Type a word to start',
    '本地大模型背单词': 'Local-LLM vocabulary trainer',
    '忽略缓存重新查询': 'Bypass cache and re-lookup',
    '忽略缓存重新翻译': 'Bypass cache and re-translate',
    '查询结果会缓存到本地，离线也能看': 'Results are cached locally for offline use',
    '＝只出现在关系里（双击去查词）。': '= only appears in relations (double-click to look up).',
    '留空自动选择第一个': 'Blank auto-picks the first one',
    '例如：四级核心词汇': 'e.g. CET-4 core vocabulary',
    '完整释义 ›': 'Full entry ›',
    '← 返回': '← Back',
    // 主题切换按钮：图标改成内联 SVG 后，按钮里**只剩这两个纯文字**，
    // 所以键也得跟着换成纯文字（原来键里带着 ☾ / ☀ 那两个字形）。
    '深色': 'Dark',
    '浅色': 'Light',
    '跟随系统': 'Follow system',
    '查询语言（要学习的语言）': 'Lookup language (the language you are learning)',

    // ---- 背诵：今日复习入口 / 连对 / 错词攻坚 ----
    '连对': 'Streak',
    '正在统计…': 'Counting…',
    '开始今日复习': "Start today's review",
    '背的时候可以完全不动鼠标：': 'You never have to touch the mouse while reciting:',

    // ---- 模板键（含 {n}，会把中间那段当值塞回译文同一位置）----
    '已载入 {n} 个示例单词': '{n} sample words loaded',
    '共 {n} 条': '{n} in total',
    '第 {n} 题': 'Question {n}',
    '还剩 {n} 个': '{n} remaining',
    // 多占位符按顺序回填：m[1] → 第一个 {n}，m[2] → 第二个 {n}
    '开始今日复习（{n} 词）': "Start today's review ({n} words)",
    '到期 {n} 词 + 常错 {n} 词。系统会先排到期与常错词，再补薄弱词与新词。':
      '{n} due + {n} frequently missed. Due and missed words come first, then weak and new ones.',
    // 攻坚提示里的 `<b>连对 3 次</b>` 是一个独立文本节点，只能靠模板键接住，
    // 精确键「连对」匹配不上它（多了一个数字）。
    '连对 {n} 次': '{n} correct in a row',
    '攻坚中：再连对 {n} 次出队': '{n} more correct in a row to clear it',
    '攻坚成功，{n} 已出队 —— 连对 {n} 次': 'Got it — {n} cleared after {n} correct in a row',

    /* ============================================================
       以下三块是 v0.49 新增界面的文案。

       为什么单列而不是随手混进上面的分组：这几处的**键是从渲染结果反推出来的**，
       不是从源码字面量抄的 —— 源码里是 `<span>共 <b>${n}</b> 个词 / …</span>`
       这种被标签拆散的模板，真正被翻译的是**每个文本节点**。
       抄源码字面量会写出一条永远命不中的死键（覆盖率还照算），
       所以这里只写「界面上真正会出现的那一小段」。
       ============================================================ */

    // ---- 迷你悬浮窗（v0.49.0 新增的独立窗口）----
    '背词': 'Review',
    '正在取词…': 'Loading a word…',
    '认识': 'Know',
    '不认识': 'Don’t know',
    '词库里还没有可背的词。先在主窗口导入或下载一本词库。':
      'Nothing to review yet. Import or download a wordbook in the main window first.',
    '取词失败：{n}': 'Failed to load a word: {n}',
    '这个词还没有释义 —— 点右上角「补全」让 AI 生成完整词条。':
      'No definition yet — click “Complete” in the top-right to have the AI generate the full entry.',
    '…共 {n} 个义项': '…{n} senses in total',
    '朗读例句': 'Read the example aloud',
    '词组 / 变形': 'Phrases / Inflections',
    '还没背过': 'Not reviewed yet',
    '熟练度 {n}% · 正确率 {n}% · 下次复习 {n}': 'Mastery {n}% · Accuracy {n}% · Next review {n}',
    '今日 {n} · 待复习 {n}': 'Today {n} · Due {n}',
    '认识 · 下次复习 {n}': 'Know · Next review {n}',
    '不认识 · 下次复习 {n}': 'Don’t know · Next review {n}',
    // 兜底文案「很快」会落进上面两条 {n} 的捕获组里原样带出来，
    // 而捕获的内容不会被二次翻译 —— 所以这两种组合要单独建键。
    '认识 · 下次复习 很快': 'Know · next review soon',
    '不认识 · 下次复习 很快': 'Don’t know · next review soon',
    '很快': 'soon',
    '「{n}」已掌握': '“{n}” mastered',
    '提交失败': 'Submit failed',
    '补全': 'Complete',
    '补全中…': 'Completing…',
    '补全失败': 'Completion failed',
    '补全这个词条（例句 / 变形 / 相关词）':
      'Complete this entry (examples / inflections / related words)',
    '没补出新内容（可先在设置页部署本地大模型）':
      'Nothing new to add (you can set up the local LLM on the Settings page first)',
    '已补全「{n}」的词条': 'Completed the entry for “{n}”',
    '窗口置顶': 'Keep the window on top',
    '已置顶': 'Pinned',
    '已取消置顶': 'Unpinned',
    '切换尺寸': 'Switch size',
    '尺寸 {n}×{n}': 'Size {n}×{n}',
    '切换透明度': 'Switch opacity',
    '不透明度 {n}%': 'Opacity {n}%',

    // ---- 详情卡的掌握度行（和迷你窗同一句话的两个渲染点）----
    '尚未学习': 'Not studied yet',
    '熟练度 {n}%': 'Mastery {n}%',
    '· 正确率': '· Accuracy',
    '% · 下次复习 {n}': '% · Next review {n}',

    // ---- 词图：介绍面板与状态条（v0.49.0 新增）----
    '同义': 'Synonym',
    '反义': 'Antonym',
    '派生': 'Derived',
    '相关': 'Related',
    '上义': 'Hypernym',
    '下义': 'Hyponym',
    '构建中…': 'Building…',
    '已清空图谱关系': 'Graph relations cleared',
    'AI 发散中…': 'AI exploring…',
    '先在图上点一个词，或搜索一个词': 'Click a word on the graph, or search for one',
    '没有匹配的词': 'No matching words',
    '以它为中心展开': 'Expand around it',
    '回到全局': 'Back to global',
    '全局视图 · 点任意词以它为中心展开': 'Global view · click any word to expand around it',
    '还没有关系数据。点「构建图谱」从词库抽取关系。':
      'No relation data yet. Click “Build the graph” to extract relations from your wordbook.',
    '暂无释义（这个词只有关系数据，没有完整词条）。':
      'No definition yet (this word only has relation data, not a full entry).',
    '连接 {n} 条': '{n} links',
    '· 熟练度 {n}%': '· Mastery {n}%',
    '连接 {n} 条 · 词库已收录': '{n} links · in your dictionary',
    '连接 {n} 条 · 熟练度 {n}% · 词库已收录': '{n} links · Mastery {n}% · in your dictionary',
    '连接 {n} 条 · 词库外（模型发散）': '{n} links · outside the wordbook (model exploration)',
    '连接 {n} 条 · 单击看介绍 · 双击发散': '{n} links · click for details · double-click to explore',
    '已从词库抽取 {n} 条新关系': 'Extracted {n} new relations from the wordbook',
    // ★ 多占位符的键，**译文里的 {n} 顺序必须与键里一致**（回填是按捕获组
    //   顺序来的，不是按语义）。键是「为「词」新增 N 条」，英文就只能
    //   把词放在前面 —— 写成 "Added {n} relations for “{n}”" 会输出
    //   `Added apple relations for “8”`。
    '为「{n}」新增 {n} 条关系': '“{n}” gained {n} new relations',
    '已隐藏 {n} 类': '{n} types hidden',
    '（可在设置页「本地大模型」一键部署）':
      '(one-click setup under “Local model” on the Settings page)',
    '搜索失败：{n}': 'Search failed: {n}',
    '读取图谱失败：{n}': 'Failed to load the graph: {n}',
    '（当前显示 {n} 条）': '({n} shown)',
    // ★ 下面四条是**被 <b> 拆散的文本节点**：`共 <b>128</b> 个词 / <b>45</b> 条关系`。
    //   整段 HTML 从来不会成为文本节点，只能按碎片建键（已确认全项目没有
    //   其它地方出现独立的「共」「以」「条关系」节点，不会误伤）。
    '共': 'Total',
    '个词 /': 'words /',
    '条关系': 'relations',
    '以': 'Centered on',
    '为中心': '',

    // ---- 设置页「词条质量」面板（v0.49.0 新增）----
    '词条质量': 'Entry quality',
    '语种统计': 'Language stats',
    '清理语种错标': 'Fix mislabeled languages',
    '把语种标错的存量词条搬回它真正该在的语言下（不删词）':
      'Move entries labeled with the wrong language back under the right one (nothing is deleted)',
    'AI 自检一批': 'Audit a batch with AI',
    '停止': 'Stop',
    '自检条数': 'Entries to audit',
    '语种闸门': 'Language gate',
    '：词条入库前按书写系统判定 —— 含假名判日语、含谚文判韩语、纯汉字判中文，只有拉丁字母的词才归入英语词表。第三方词表、粘贴导入、手动加词全部过这道闸门，混进来的日语会被拦下并计进报告。':
      ': every entry is classified by writing system before it is saved — kana means Japanese, hangul means Korean, pure Han means Chinese, and only Latin-script words go into an English wordbook. Third-party wordlists, pasted imports and manual additions all pass this gate; anything that slips in is blocked and counted in the report.',
    'AI 自检': 'AI audit',
    '：每个词条纳入背词表时由本地大模型校一遍拼写、词性、中文释义与例句；有问题就修正并':
      ': when an entry joins the review list, the local LLM checks its spelling, part of speech, Chinese definition and examples; problems are fixed and then',
    '复检一轮': 're-audited once',
    '，仍不合格才标记剔除（不真删，只是不再进出题队列）。每次校验都留日志，可在下面查。':
      ', and only then marked as rejected (nothing is deleted — it just stops appearing in the quiz queue). Every check is logged below.',
    '通过': 'Passed',
    '已修正': 'Fixed',
    '已剔除': 'Rejected',
    '自检中…': 'Auditing…',
    '自检失败': 'Audit failed',
    '还没有自检记录。': 'No audit records yet.',
    '已请求停止，当前这个词跑完就停': 'Stop requested — it will stop after the current word',
    '词库还是空的。': 'The wordbook is still empty.',
    '清理中…': 'Cleaning…',
    '学习数据': 'Study data',
    '读取失败：{n}': 'Failed to read: {n}',
    '读取自检日志失败：{n}': 'Failed to read the audit log: {n}',
  };

  /** `EN` 里所有含 `{n}` 的键，抽出来是为了跳过无关的精确键、少跑几次正则。 */
  const PATTERN_KEYS = Object.keys(EN).filter((k) => k.indexOf('{n}') >= 0);

  const DICTS = { 'zh-CN': null, en: EN };

  /** 当前语言。默认跟随后端配置，读不到就退回简体中文。 */
  let current = 'zh-CN';

  /** storage 监听只挂一次（init 可被多次调用，窗口里两个视图共用一个文档）。 */
  let storageHooked = false;

  /** 文本节点的原文备份（切语言时要从原文重翻，不能从英文翻回去）。 */
  const nodeSrc = new WeakMap();

  /**
   * 模板键的正则缓存：`{ lang → Map<键, RegExp> }`。
   *
   * 按语言分桶，切语言时不用清——另一语言的编译结果各自留着即可。
   */
  const patternCache = new Map();

  /** 把 `'共 {n} 条'` 编译成 `/^共 (.*?) 条$/`。 */
  function regexFor(lang, key) {
    let bucket = patternCache.get(lang);
    if (!bucket) { bucket = new Map(); patternCache.set(lang, bucket); }
    let re = bucket.get(key);
    if (re) return re;
    const parts = key.split('{n}');
    re = new RegExp('^' + parts.map(escapeRe).join('(.*?)') + '$');
    bucket.set(key, re);
    return re;
  }

  function escapeRe(s) {
    return s.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
  }

  /** 翻译入口。当前语言是中文时原样返回，零开销。 */
  function t(s) {
    if (!s || current === 'zh-CN') return s;
    const d = DICTS[current];
    if (!d) return s;
    const hit = d[s];
    return hit == null ? s : hit;
  }

  /**
   * 完整翻译：先查精确键，查不到再试模板键。
   *
   * ★ 不能写成 `t(src) || tPattern(src)`：`t` 漏翻时返回的是**原文**，
   *   非空串是 truthy，会把 `||` 短路掉，模板键永远没机会上场。
   *   单独留这段注释，是因为还原成一行之后光看代码发现不了这个坑。
   */
  /**
   * 带首尾空白保留的翻译。
   *
   * 为什么单独一层：HTML 里缩进会把一个句子撕成 `"\n    决定 "` 这种样子，
   * 字典键却必须是规整的单个空格版本。但**首尾的空白不能丢** —— 行内元素
   * （`<b>界面语言</b> 决定…`）之间那一个空格就是靠它撑出来的，直接写 trimmed
   * 的结果会让「词」和「词」粘在一起。
   */
  function translateWithSpace(src, raw) {
    /* ★ R1 修复：翻译必须从**原文备份**出发。
       旧实现拿 raw（当前显示值）去翻 —— 英文状态下 raw 是英文，
       切回中文时 translate() 对 zh-CN 早退、原样返回英文 → 英文永远
       切不回中文，界面越切越中英混杂。src（原文）才是唯一可靠输入；
       首尾空白也取自 src，保证切回原文时缩进一模一样。 */
    const lead = /^\s*/.exec(src)[0];
    const trail = /\s*$/.exec(src)[0];
    const body = src.replace(/\s+/g, ' ').trim();
    const out = translate(body);
    if (out === body) return src;
    return lead + out + trail;
  }

  function translate(src) {
    if (!src || current === 'zh-CN') return src;
    const d = DICTS[current];
    if (!d) return src;
    const hit = d[src];
    if (hit != null) return hit;
    return tPattern(src);
  }

  /**
   * 带占位符的翻译：把动态内容从原文里抠出来、再塞回译文的同一位置。
   *
   * 例如键 `'已收录 {n} 个单词'`，收到 `'已收录 128 个单词'` →
   * 取出 `128` → 产出 `'128 words saved'`。
   *
   * ★ 候选键按**首字符**分桶，而不是「文本里有没有数字」。
   *
   *   旧实现用 `/\d/.test(text)` 当「这条文本可能有插值」的近似判据，
   *   两个方向都会错：
   *     - 该翻的漏翻：`取词失败：网络不可用`、`「carpet」已掌握` 里没有数字，
   *       直接被挡在模板键之外 —— 界面切成英文后这几句永远是中文；
   *     - 白跑正则：一句含数字的普通文本会把**全部**模板键都试一遍。
   *   改成按首字符取候选：一次 Map 查询，命中不了就退出（绝大多数文本如此），
   *   命中也只跑同首字符的那两三条正则。**更快而且不漏。**
   */
  const patternByFirst = new Map();
  for (const k of PATTERN_KEYS) {
    const c = k[0];
    if (!patternByFirst.has(c)) patternByFirst.set(c, []);
    patternByFirst.get(c).push(k);
  }

  function tPattern(text) {
    if (!text || current === 'zh-CN') return text;
    const cands = patternByFirst.get(text[0]);
    if (!cands) return text;
    const d = DICTS[current];
    if (!d) return text;
    for (const key of cands) {
      if (d[key] == null) continue;
      const m = regexFor(current, key).exec(text);
      if (!m) continue;
      let out = d[key];
      for (let i = 1; i < m.length; i += 1) out = out.replace('{n}', m[i]);
      return out;
    }
    return text;
  }

  /** 可翻译的元素属性（值会被写回 DOM，所以必须都是纯文本语义的）。 */
  const ATTRS = [
    ['title', 'i18nTitleSrc'],
    ['placeholder', 'i18nPhSrc'],
    ['aria-label', 'i18nAriaSrc'],
  ];

  /** 这些子树里的文字不该碰（代码、样式、图形、正在输入的内容）。 */
  const SKIP_TAGS = { SCRIPT: 1, STYLE: 1, CODE: 1, PRE: 1, SVG: 1, TEXTAREA: 1 };

  /** 局部刷新一个元素：文本 + 可翻译属性。 */
  function localizeEl(el) {
    if (!el || el.nodeType !== 1) return;
    const tag = el.tagName;
    if (SKIP_TAGS[tag]) return;
    if (el.hasAttribute && el.hasAttribute('data-i18n-off')) return;

    for (const [attr, srcAttr] of ATTRS) {
      if (!el.hasAttribute || !el.hasAttribute(attr)) continue;
      let src = el.getAttribute(srcAttr);
      const cur = el.getAttribute(attr);
      /* ★ R5 修复：原文缓存必须能失效。旧实现备份一次就永不更新 ——
         业务代码后来把 title/placeholder 改成新值，下一轮 localize 还是
         拿旧原文去翻、把新值盖掉（「数字过期」类漂移）。
         判据：当前值既不是我们写出去的译文、也不是备份的原文
         → 说明被外部改写过 → 重新备份。 */
      const expected = src == null ? null : translate(src);
      if (src == null || (cur !== expected && cur !== src)) {
        src = cur;
        el.setAttribute(srcAttr, src);
      }
      const out = translate(src);
      if (out !== el.getAttribute(attr)) el.setAttribute(attr, out);
    }
  }

  /**
   * TreeWalker 的 whatToShow 掩码：元素(1) + 文本(4)。
   *
   * 刻意不用 `NodeFilter.SHOW_TEXT | NodeFilter.SHOW_ELEMENT`：`NodeFilter`
   * 在 webview 里一定有，但在无头/测试环境里未必挂载，一旦缺就是
   * `ReferenceError`——而 `localize` 跑在每一次渲染之后，炸这里等于整个
   * 界面卡在半渲染状态。写死常量就没这个依赖。
   */
  const SHOW = 5;

  /**
   * 局部刷新**一个文本节点**（R2 的关键零件）。
   *
   * 抽出来是因为 MutationObserver 现在要直接喂文本节点进来：
   * `el.textContent = '中文'` 这类纯文本改写产生的是 characterData
   * 变更 + 纯文本子节点，旧代码只收元素节点，全部漏网 ——
   * 叠加 60 秒一次的 CTA 重刷，就是「随机漂移」观感的主来源。
   */
  function localizeTextNode(n) {
    if (!n || n.nodeType !== 3) return;
    if (!n.nodeValue || !n.nodeValue.trim()) return;
    const parent = n.parentNode;
    if (parent && parent.nodeType === 1 && SKIP_TAGS[parent.tagName]) return;
    let src = nodeSrc.get(n);
    if (src == null) { src = n.nodeValue; nodeSrc.set(n, src); }
    const out = translateWithSpace(src, n.nodeValue);
    if (out !== n.nodeValue) n.nodeValue = out;
  }

  /** 局部刷新一棵子树（含起点自身）。 */
  function localize(root) {
    if (!root) return;
    if (root.nodeType === 3) { localizeTextNode(root); return; }
    localizeEl(root);
    if (!root.childNodes || !document.createTreeWalker) return;

    const walker = document.createTreeWalker(root, SHOW, null);
    let n = walker.currentNode;
    while (n) {
      if (n.nodeType === 1) {
        localizeEl(n);
      } else if (n.nodeType === 3) {
        localizeTextNode(n);
      }
      n = walker.nextNode();
    }
  }

  /** 观察者是否在自己触发的改动里 —— 防止改文本又触发一轮的死循环。 */
  let selfWrite = false;
  let observer = null;

  /** 接管后续所有动态渲染：新挂载的亚树自动过一遍字典。 */
  function watch() {
    if (observer || typeof MutationObserver === 'undefined') return;
    observer = new MutationObserver((list) => {
      if (selfWrite) return;
      const roots = [];
      for (const m of list) {
        // R2-a：`el.textContent = '中文'` 这类改写只发 characterData ——
        // 旧实现直接跳过，是「随机漂移」的最大漏网点。
        if (m.type === 'characterData') {
          if (m.target && m.target.nodeType === 3) roots.push(m.target);
          continue;
        }
        if (m.type !== 'childList') continue;
        // R2-b：新增节点也收文本节点（innerHTML 换成纯文本时新增的是 3 号节点）
        m.addedNodes.forEach((x) => {
          if (x.nodeType === 1 || x.nodeType === 3) roots.push(x);
        });
      }
      if (!roots.length) return;
      selfWrite = true;
      try { for (const r of roots) localize(r); } finally { selfWrite = false; }
    });
    observer.observe(document.body, {
      childList: true,
      subtree: true,
      characterData: true,
    });
  }

  /** 重新选择语言：先把现有界面从原文重翻一遍，再落盘。 */
  function setLang(code, opts = {}) {
    const known = LANGS.some((l) => l.id === code);
    const next = known ? code : 'zh-CN';
    if (next !== current) {
      current = next;
      localize(document.body);
      try { document.documentElement.lang = next; } catch (e) { /* DOM 还没好 */ }
    }
    if (opts.persist !== false) {
      try { localStorage.setItem(LS_KEY, next); } catch (e) { /* 隐私模式下可写失败，不影响本次显示 */ }
    }
    return next;
  }

  /**
   * 启动。
   *
   * 顺序很关键：必须在任何页面渲染**之前**跑一次 full pass —— 否则首屏那批
   * index.html 里的静态文案会赶在字典接入前就画好，切语言才生效、进应用时不生效。
   *
   * @param {string} [fromConfig] 后端 `AppConfig::ui_lang`，优先级高于本地缓存：
   *   用户可能在别处改过，配置文件比浏览器本地存储更可信。
   */
  function init(fromConfig) {
    let picked = '';
    if (fromConfig) picked = String(fromConfig).trim();
    if (!picked) {
      try { picked = localStorage.getItem(LS_KEY) || ''; } catch (e) { /* 同上 */ }
    }
    current = LANGS.some((l) => l.id === picked) ? picked : 'zh-CN';
    try { document.documentElement.lang = current; } catch (e) { /* 忽略 */ }
    localize(document.body);
    watch();

    /* ★ R3 后半：跨窗口语言同步。localStorage 的 `storage` 事件只在**别的**
       文档里触发 —— 主窗把语言下拉改了会写 LS，侧栏窗口在这里收到并直接
       切，不必重开；修掉「主窗英文、侧栏中文」。Rust 侧不广播也够用，
       因为 LS 现在与后端 config 是双写同步的（cmd_set_ui_lang）。 */
    if (!storageHooked && typeof window !== 'undefined' && window.addEventListener) {
      storageHooked = true;
      try {
        window.addEventListener('storage', (e) => {
          if (!e || e.key !== LS_KEY || !e.newValue || e.newValue === current) return;
          setLang(e.newValue, { persist: false });
        });
      } catch (e) { /* 事件环境异常不该影响启动 */ }
    }

    return current;
  }

  window.I18n = {
    LANGS,
    init,
    setLang,
    t,
    translate,
    tPattern,
    localize,
    current: () => current,
  };
})();
