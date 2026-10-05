/* ============================================================
   demo.js —— 演示模式（需求 3）

   刚装好、还没开始背的时候，统计页/计划页/错词本/图谱全是空的，
   看上去像坏了。演示模式就是给这些页面填一份**看得懂的示例数据**，
   让用户先知道「这里将来会长成什么样」。

   两条硬约束：

   1. **绝不入库**。示例数据只在前端内存里生成，写操作照常走真实后端。
      拦截点用白名单，只接管「读」命令；任何会改数据的命令都不拦，
      所以不存在「示例数据污染真实词库」的可能。

   2. **一眼能认出来**。开启后有一条常驻横幅，且每个用到示例数据的页面
      都能看到。用户不该误把示例当成自己的成绩。

   数据全部写死，不用随机数：刷新一次数字就变一次的「演示」只会让人困惑。
   ============================================================ */

const Demo = (() => {
  const KEY = 'wordwise.demo_mode';

  let on = false;
  try { on = localStorage.getItem(KEY) === '1'; } catch (e) { on = false; }

  /* ---------------- 示例词表 ---------------- */
  //
  // 挑一批四六级高频词，释义照抄常见词典义，保证看起来真实。
  const WORDS = [
    { w: 'abandon',   phon: '/əˈbændən/',   pos: 'v.',  def: '放弃；抛弃' },
    { w: 'benefit',   phon: '/ˈbenɪfɪt/',   pos: 'n.',  def: '益处，好处' },
    { w: 'capture',   phon: '/ˈkæptʃə(r)/', pos: 'v.',  def: '捕获；占领' },
    { w: 'decline',   phon: '/dɪˈklaɪn/',   pos: 'v.',  def: '下降；婉拒' },
    { w: 'elaborate', phon: '/ɪˈlæbərət/',  pos: 'adj.', def: '精心制作的；详尽的' },
    { w: 'fragile',   phon: '/ˈfrædʒaɪl/', pos: 'adj.', def: '易碎的；脆弱的' },
    { w: 'generate',  phon: '/ˈdʒenəreɪt/', pos: 'v.',  def: '产生，生成' },
    { w: 'hesitate',  phon: '/ˈhezɪteɪt/', pos: 'v.',  def: '犹豫，迟疑' },
    { w: 'justify',   phon: '/ˈdʒʌstɪfaɪ/', pos: 'v.',  def: '证明…正当；为…辩护' },
    { w: 'maintain',  phon: '/meɪnˈteɪn/', pos: 'v.',  def: '维持；保养；主张' },
    { w: 'negotiate', phon: '/nɪˈɡəʊʃieɪt/', pos: 'v.', def: '谈判，协商' },
    { w: 'reluctant', phon: '/rɪˈlʌktənt/', pos: 'adj.', def: '不情愿的，勉强的' },
  ];

  function entry(w) {
    return {
      word: w.w, lang: 'en',
      phonetic: { uk: w.phon, us: w.phon },
      senses: [{ pos: w.pos, definition: w.def }],
      related: [], inflections: [], examples: [],
    };
  }

  const ENTRIES = WORDS.map(entry);
  const BY_WORD = new Map(ENTRIES.map(e => [e.word, e]));

  /* ---------------- 示例：近 14 天答题量 ---------------- */
  //
  // [当天答题数, 答对数]。第 3 天和第 11 天是 0，用来表现「断了两天」，
  // 这样柱状图才有高低起伏，而不是一条平线。
  const HIST = [
    [12, 9], [18, 14], [0, 0], [22, 17], [25, 20], [30, 24], [14, 11],
    [28, 22], [35, 27], [40, 33], [0, 0], [45, 38], [52, 44], [64, 51],
  ];

  function dayKey(offsetDays) {
    const d = new Date(Date.now() + offsetDays * 86400000);
    return d.toISOString().slice(0, 10);
  }

  function stats() {
    const history = HIST.map(([count, correct], i) => ({
      date: dayKey(i - (HIST.length - 1)),
      count,
      correct,
    }));
    return {
      total_words: 128,
      learned: 86,
      mastered: 41,
      leeches: 12,
      due_today: 23,
      reviewed_today: 64,
      correct_today: 51,
      wrong_today: 13,
      streak_days: 9,
      history,
    };
  }

  /* ---------------- 示例：未来 14 天计划 ---------------- */
  //
  // 今天最多（含逾期），往后递减再在「第 7 / 15 天」这种记忆节点抬头，
  // 这样能直观看出遗忘曲线在起作用。
  const PLAN_SHAPE = [23, 9, 6, 12, 5, 4, 17, 8, 5, 3, 11, 6, 4, 26];

  function plan(days) {
    const n = Math.max(1, Math.min(90, days || 14));
    const WEEK = ['周日', '周一', '周二', '周三', '周四', '周五', '周六'];
    return Array.from({ length: n }, (_, i) => ({
      date: dayKey(i),
      weekday: WEEK[new Date(Date.now() + i * 86400000).getDay()],
      count: PLAN_SHAPE[i % PLAN_SHAPE.length],
      overdue: i === 0 ? 6 : 0,
      is_today: i === 0,
    }));
  }

  /* ---------------- 示例：到期清单 ---------------- */
  const DUE = [
    { w: 'abandon',   over: 6, wrong: 7, leech: true },
    { w: 'reluctant', over: 3, wrong: 4, leech: true },
    { w: 'negotiate', over: 1, wrong: 2, leech: false },
    { w: 'hesitate',  over: 0, wrong: 3, leech: true },
    { w: 'fragile',   over: 0, wrong: 1, leech: false },
    { w: 'justify',   over: 0, wrong: 0, leech: false },
  ];

  function dueWords() {
    const base = Math.floor(Date.now() / 1000);
    return DUE.map(d => {
      const meta = WORDS.find(x => x.w === d.w) || { pos: '', def: '' };
      return {
        word: d.w,
        gloss: `${meta.pos} ${meta.def}`.trim(),
        due_at: base - d.over * 86400,
        due_label: d.over > 0 ? `逾期 ${d.over} 天` : '今天 09:00',
        overdue: d.over > 0,
        overdue_days: d.over,
        mastery: Math.max(5, 55 - d.wrong * 7),
        wrong_count: d.wrong,
        is_leech: d.leech,
      };
    });
  }

  /* ---------------- 示例：错词本 ---------------- */
  const LEECH = [
    { w: 'abandon',   wrong: 9, correct: 2, mastery: 12 },
    { w: 'reluctant', wrong: 7, correct: 3, mastery: 24 },
    { w: 'elaborate', wrong: 6, correct: 2, mastery: 31 },
    { w: 'hesitate',  wrong: 5, correct: 4, mastery: 44 },
    { w: 'negotiate', wrong: 4, correct: 3, mastery: 38 },
    { w: 'fragile',   wrong: 3, correct: 5, mastery: 52 },
    { w: 'decline',   wrong: 3, correct: 6, mastery: 58 },
    { w: 'justify',   wrong: 2, correct: 4, mastery: 61 },
    { w: 'capture',   wrong: 2, correct: 7, mastery: 66 },
    { w: 'generate',  wrong: 1, correct: 6, mastery: 73 },
    { w: 'maintain',  wrong: 1, correct: 8, mastery: 78 },
    { w: 'benefit',   wrong: 1, correct: 9, mastery: 81 },
  ];

  function leechRows() {
    const base = Math.floor(Date.now() / 1000);
    return LEECH.map((l, i) => {
      const e = BY_WORD.get(l.w);
      return {
        entry: e || entry(WORDS.find(x => x.w === l.w) || WORDS[0]),
        state: {
          word: l.w, lang: 'en', ease_factor: 2.5, interval_days: 1,
          repetitions: 0, due_at: base + 3600 * (i + 1),
          last_review_at: base - 86400,
          correct_count: l.correct, wrong_count: l.wrong,
          is_leech: true, mastery: l.mastery, is_mastered: false,
        },
        retention: Math.max(0.05, 0.92 - i * 0.07),
        error_rate: l.wrong / (l.wrong + l.correct),
      };
    });
  }

  function leechQuery(args) {
    let rows = leechRows();
    const minWrong = args.minWrong || 0;
    const maxMastery = args.maxMastery == null ? 100 : args.maxMastery;
    const minRate = args.minErrorRate || 0;
    rows = rows.filter(x =>
      x.state.wrong_count >= minWrong &&
      x.state.mastery <= maxMastery &&
      x.error_rate >= minRate);
    const order = args.order || 'wrong';
    rows.sort((a, b) => {
      if (order === 'mastery') return a.state.mastery - b.state.mastery;
      if (order === 'word') return a.entry.word.localeCompare(b.entry.word);
      if (order === 'rate') return b.error_rate - a.error_rate;
      if (order === 'recent') return b.state.last_review_at - a.state.last_review_at;
      return b.state.wrong_count - a.state.wrong_count;
    });
    return rows.slice(0, args.limit || 300);
  }

  function leechSummary() {
    const rows = leechRows();
    const tw = rows.reduce((s, x) => s + x.state.wrong_count, 0);
    return {
      total: rows.length,
      total_wrong: tw,
      stubborn: rows.filter(x => x.state.wrong_count >= 5).length,
      avg_mastery: Math.round(rows.reduce((s, x) => s + x.state.mastery, 0) / rows.length * 10) / 10,
      dict_size: 128,
    };
  }

  /* ---------------- 示例：知识图谱 ---------------- */
  //
  // 手搭一张小网：同义 / 反义 / 派生各来几条，够看出颜色与虚实的区别。
  const EDGES = [
    ['abandon', 'reluctant', 'related'],
    ['abandon', 'maintain', 'antonym'],
    ['benefit', 'justify', 'related'],
    ['capture', 'abandon', 'antonym'],
    ['decline', 'generate', 'antonym'],
    ['elaborate', 'fragile', 'related'],
    ['generate', 'generate', 'derived'],
    ['hesitate', 'reluctant', 'synonym'],
    ['justify', 'maintain', 'synonym'],
    ['maintain', 'generate', 'related'],
    ['negotiate', 'justify', 'related'],
    ['reluctant', 'hesitate', 'related'],
  ].filter(e => e[0] !== e[1]);

  const RELS = [
    { code: 'synonym', name: '同义' },
    { code: 'antonym', name: '反义' },
    { code: 'derived', name: '派生' },
    { code: 'related', name: '相关' },
    { code: 'hypernym', name: '上义' },
    { code: 'hyponym', name: '下义' },
  ];

  function graphEdges() {
    return EDGES.map(([a, b, rel]) => ({
      src: a, dst: b, rel,
      weight: rel === 'derived' ? 0.6 : 1.0,
      source: 'local',
    }));
  }

  function graphView(center) {
    const edges = graphEdges();
    let words = new Set();
    edges.forEach(e => { words.add(e.src); words.add(e.dst); });

    // 指定中心词时，只保留它 1-2 跳内的邻居，模拟后端 BFS
    if (center) {
      const keep = new Set([center]);
      for (let hop = 0; hop < 2; hop++) {
        for (const e of edges) {
          if (keep.has(e.src)) keep.add(e.dst);
          if (keep.has(e.dst)) keep.add(e.src);
        }
      }
      words = keep;
    }

    const shown = edges.filter(e => words.has(e.src) && words.has(e.dst));
    const deg = new Map();
    shown.forEach(e => {
      deg.set(e.src, (deg.get(e.src) || 0) + 1);
      deg.set(e.dst, (deg.get(e.dst) || 0) + 1);
    });

    const nodes = [...words].map(w => {
      const e = BY_WORD.get(w);
      return {
        word: w, lang: 'en', degree: deg.get(w) || 0,
        in_dict: !!e,
        gloss: (e && e.senses && e.senses[0] && e.senses[0].definition) || '',
        mastery: null,
      };
    });

    return {
      nodes, edges: shown, center: center || null,
      total_nodes: nodes.length, total_edges: shown.length,
    };
  }

  /* ---------------- 拦截白名单 ---------------- */
  //
  // 只列「读」。写命令（cmd_submit_answer / cmd_clear_leech / cmd_save_config …）
  // 一律不拦 —— 演示模式下用户真去背单词，成绩照常进真实库。
  const READERS = {
    cmd_stats: () => stats(),
    cmd_review_plan: (a) => plan(a.days),
    cmd_due_words: () => dueWords(),
    cmd_leech_query: (a) => leechQuery(a),
    cmd_leech_summary: () => leechSummary(),
    cmd_graph_rels: () => RELS,
    cmd_graph_view: (a) => graphView(a.center),
    cmd_graph_stats: () => {
      const e = graphEdges();
      const ws = new Set();
      e.forEach(x => { ws.add(x.src); ws.add(x.dst); });
      return { nodes: ws.size, edges: e.length };
    },
    cmd_graph_search: (a) => {
      const q = String((a && a.q) || '').trim().toLowerCase();
      if (!q) return WORDS.slice(0, 12).map(x => x.w);
      return WORDS.filter(x => x.w.startsWith(q)).map(x => x.w);
    },
  };

  function has(cmd) {
    return Object.prototype.hasOwnProperty.call(READERS, cmd);
  }

  function forCmd(cmd, args) {
    return READERS[cmd](args || {});
  }

  /* ---------------- 开关 ---------------- */

  function set(on_) {
    on = !!on_;
    try { localStorage.setItem(KEY, on ? '1' : '0'); } catch (e) { /* 隐私模式下忽略 */ }
    renderBanner();
    const box = document.getElementById('set-demo-mode');
    if (box) box.checked = on;
  }

  function renderBanner() {
    const el = document.getElementById('demo-banner');
    if (!el) return;
    el.classList.toggle('hidden', !on);
  }

  /** 开启后把受影响的页面重刷一遍，否则要手动切页才看得到示例数据。 */
  function refreshPages() {
    if (!window.Pages) return;
    const p = window.Pages.current;
    if (p) window.Pages.go(p);
  }

  function bind() {
    renderBanner();
    const box = document.getElementById('set-demo-mode');
    if (box) {
      box.checked = on;
      box.addEventListener('change', () => {
        set(box.checked);
        refreshPages();
        const t = window.WW;
        if (t) {
          t.toast(box.checked
            ? '演示模式已开启：统计/计划/错词本/图谱显示示例数据，不会写入你的词库'
            : '演示模式已关闭，恢复显示真实数据', 'ok');
        }
      });
    }
    const off = document.getElementById('demo-banner-off');
    if (off) off.addEventListener('click', () => {
      set(false);
      refreshPages();
    });
  }

  return {
    bind, set,
    has, for: forCmd,
    get enabled() { return on; },
  };
})();

window.Demo = Demo;
