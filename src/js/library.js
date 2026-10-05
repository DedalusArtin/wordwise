/* ============================================================
   library.js —— 词库管理、错词本、复习计划、统计
   ============================================================ */

/* ---------------- 多级词库（需求 3 / 5） ---------------- */

const Books = (() => {
  const { API } = window.WordWiseAPI;
  const U = () => window.WW;

  let books = [];
  let catalog = [];
  let currentTab = 'books';

  const CAT_LABEL = {
    root: '全部', exam: '考试大类', cet4: '四级', cet6: '六级', kaoyan: '考研',
    ielts: '雅思', toefl: '托福', gre: 'GRE', gaokao: '高考', jlpt: '日语 JLPT',
    other: '其他',
  };
  const catLabel = (c) => CAT_LABEL[c] || c || '未分类';

  function bind() {
    // 标签切换
    document.querySelectorAll('.lib-tab').forEach(t => {
      t.addEventListener('click', () => switchTab(t.dataset.libtab));
    });

    document.getElementById('btn-new-book')?.addEventListener('click', newBook);
    document.getElementById('btn-import-book-file')?.addEventListener('click', () => openImportDialog());
    document.getElementById('bi-pickfile')?.addEventListener('click', () => {
      document.getElementById('bi-file').click();
    });
    document.getElementById('bi-file')?.addEventListener('change', onFilePicked);
    document.getElementById('bi-close')?.addEventListener('click', closeImportDialog);
    document.getElementById('bookimport-close')?.addEventListener('click', closeImportDialog);
    document.getElementById('bi-cancel')?.addEventListener('click', closeImportDialog);
    document.getElementById('bookimport-overlay')?.addEventListener('click', (e) => {
      if (e.target.id === 'bookimport-overlay') closeImportDialog();
    });
    document.getElementById('bi-confirm')?.addEventListener('click', doBookImport);
    document.getElementById('catalog-cat')?.addEventListener('change', () => loadCatalog());

    // 在线词库的按钮用**事件委托**绑定在容器上，而不是每次重绘后逐个绑。
    //
    // 为什么改成委托：原来在 loadCatalog() 渲染完立刻 querySelectorAll 绑一次，
    // 只要渲染与绑定之间有任何一个环节重画了容器（切分类、搜索过滤、异步回填），
    // 按钮就会变成「看得见但点不动」——这正是用户报的「下载点了没反应」。
    // 委托到容器上后，无论列表重画多少次，点击永远有效。
    document.getElementById('lib-catalog')?.addEventListener('click', (e) => {
      const dl = e.target.closest('.btn-book-download');
      if (dl) { downloadBook(dl); return; }
      const src = e.target.closest('.btn-book-src');
      if (src) { API.openUrl(src.dataset.url).catch(() => {}); }
    });
  }

  function switchTab(tab) {
    currentTab = tab;
    document.querySelectorAll('.lib-tab').forEach(t =>
      t.classList.toggle('active', t.dataset.libtab === tab));
    ['books', 'catalog', 'words', 'learned'].forEach(t => {
      document.getElementById('lib-pane-' + t)?.classList.toggle('hidden', t !== tab);
    });
    if (tab === 'books') loadBooks();
    if (tab === 'catalog') loadCatalog();
    if (tab === 'words') window.Library.loadList();
    if (tab === 'learned') loadLearned();
    // 顶部搜索框是按标签生效的，切标签后要把当前关键词重新套到新列表上
    if (window.Library && window.Library.applySearch) window.Library.applySearch();
  }

  /** 列表重绘后重新套用搜索关键词。 */
  function reapplySearch() {
    if (window.Library && window.Library.applySearch) window.Library.applySearch();
  }

  /** 加载词库树。 */
  async function loadBooks() {
    const box = document.getElementById('lib-books');
    if (!box) return;
    box.innerHTML = U().loadingHtml();
    try {
      books = await API.listWordbooks();
    } catch (e) {
      box.innerHTML = `<div class="muted" style="padding:20px">加载失败：${U().esc(e.message)}</div>`;
      return;
    }

    // 按 parent_id 组装层级
    const byParent = {};
    books.forEach(b => {
      const p = b.parent_id || '';
      (byParent[p] = byParent[p] || []).push(b);
    });
    const rendered = [];
    const renderNode = (b, depth) => {
      const kids = byParent[b.id] || [];
      rendered.push(nodeHtml(b, depth));
      kids.forEach(k => renderNode(k, depth + 1));
    };
    (byParent[''] || []).forEach(b => renderNode(b, 0));

    box.innerHTML = rendered.join('');
    bindBookActions(box);
    reapplySearch();
  }

  function nodeHtml(b, depth) {
    const isLeaf = b.level >= 2;
    const pct = b.word_count > 0 ? Math.round((b.learned / b.word_count) * 100) : 0;
    const badges = [];
    if (b.source_url) {
      badges.push(`<a class="tag blue" href="${U().esc(b.source_url)}" data-ext="1"
        title="${U().esc(b.license || '来源')}">来源</a>`);
    }
    if (b.license) badges.push(`<span class="tag">${U().esc(b.license)}</span>`);
    if (b.builtin) badges.push('<span class="tag">内置</span>');

    return `<div class="book-node lv${b.level}" style="margin-left:${depth * 20}px"
        data-book-id="${U().esc(b.id)}">
      <div class="bn-main">
        <span class="bn-icon">${b.level === 0 ? '&#128218;' : b.level === 1 ? '&#128193;' : '&#128196;'}</span>
        <span class="bn-name">${U().esc(b.name)}</span>
        <span class="tag cat">${U().esc(catLabel(b.category))}</span>
        ${badges.join('')}
      </div>
      <div class="bn-meta">
        <span class="muted">${b.word_count || 0} 词</span>
        ${isLeaf && b.word_count ? `<span class="muted">已学 ${b.learned} · 待复习 ${b.due_today}</span>
          <div class="mastery-bar" title="已学 ${pct}%"><div class="mastery-fill" style="width:${pct}%"></div></div>` : ''}
      </div>
      <div class="bn-actions">
        ${isLeaf ? `<button class="ghost-btn xs btn-book-study" data-book-id="${U().esc(b.id)}">背这本</button>` : ''}
        ${isLeaf ? `<button class="ghost-btn xs btn-book-view" data-book-id="${U().esc(b.id)}">查看</button>` : ''}
        ${!b.builtin ? `<button class="ghost-btn xs btn-book-del" data-book-id="${U().esc(b.id)}">删除</button>` : ''}
      </div>
    </div>`;
  }

  function bindBookActions(box) {
    box.querySelectorAll('[data-ext="1"]').forEach(a => {
      a.addEventListener('click', async (e) => {
        e.preventDefault();
        try { await API.openUrl(a.getAttribute('href')); } catch (err) {}
      });
    });
    box.querySelectorAll('.btn-book-study').forEach(b => {
      b.addEventListener('click', () => window.Study.startWithBook(b.dataset.bookId));
    });
    box.querySelectorAll('.btn-book-view').forEach(b => {
      b.addEventListener('click', () => {
        switchTab('words');
        window.Library.filterByBook(b.dataset.bookId);
      });
    });
    box.querySelectorAll('.btn-book-del').forEach(b => {
      b.addEventListener('click', async () => {
        if (!confirm('确定删除该词库？（仅解除关联，单词本身保留）')) return;
        try {
          await API.deleteWordbook(b.dataset.bookId);
          U().toast('已删除词库', 'ok');
          loadBooks();
        } catch (e) { U().toast(e.message, 'err'); }
      });
    });
  }

  async function newBook() {
    const name = prompt('新词库名称：');
    if (!name) return;
    try {
      await API.createWordbook(name.trim(), 'other', 'root');
      U().toast('已创建词库', 'ok');
      loadBooks();
    } catch (e) { U().toast(e.message, 'err'); }
  }

  /** 加载在线词库目录。 */
  async function loadCatalog() {
    const box = document.getElementById('lib-catalog');
    if (!box) return;
    box.innerHTML = U().loadingHtml();
    const cat = document.getElementById('catalog-cat')?.value || 'all';
    try {
      catalog = await API.remoteCatalog(cat === 'all' ? null : cat);
    } catch (e) {
      box.innerHTML = `<div class="muted" style="padding:20px">加载失败：${U().esc(e.message)}</div>`;
      return;
    }
    if (!catalog.length) {
      box.innerHTML = '<div class="muted" style="padding:20px">该分类暂无在线词库</div>';
      return;
    }
    box.innerHTML = catalog.map(r => `
      <div class="catalog-card" data-id="${U().esc(r.id)}" data-lang="${U().esc(r.lang || 'en')}">
        <div class="cc-head">
          <span class="cc-name">${U().esc(r.name)}</span>
          <span class="tag cat">${U().esc(catLabel(r.category))}</span>
          ${r.lang && r.lang !== 'en' ? `<span class="tag blue">${U().esc(U().langLabel(r.lang))}</span>` : ''}
          ${r.installed ? '<span class="tag">已安装</span>' : ''}
        </div>
        <div class="cc-desc">${U().esc(r.description || '')}</div>
        <div class="cc-meta">
          <span class="muted">约 ${r.approx_words} 词</span>
          <span class="muted">许可：${U().esc(r.license || '未知')}</span>
        </div>
        <div class="cc-actions">
          <button class="primary-btn xs btn-book-download" data-id="${U().esc(r.id)}"
            >${r.installed ? '重新下载' : '下载并导入'}</button>
          <button class="ghost-btn xs btn-book-src" data-url="${U().esc(r.source_url)}">查看来源</button>
        </div>
        <div class="cc-status hidden"></div>
      </div>`).join('');
    reapplySearch();
  }

  /**
   * 下载并导入一本在线词库。
   *
   * 关键点：**点击后立刻给出可见反馈**。
   * 过去只有按钮文案变化，而后端要在多个镜像间逐个试（最坏 90 秒），
   * 期间界面看不出在干活，用户就判定成「点了没反应」。
   * 现在点下去同时做三件事：按钮进入忙碌态、toast 提示、卡片内出现进度行。
   */
  async function downloadBook(btn) {
    const id = btn.dataset.id;
    const card = btn.closest('.catalog-card');
    const meta = catalog.find(x => x.id === id) || {};
    const status = card ? card.querySelector('.cc-status') : null;
    const say = (text, cls) => {
      if (!status) return;
      status.textContent = text || '';
      status.className = 'cc-status' + (text ? '' : ' hidden') + (cls ? ' ' + cls : '');
    };

    if (btn.dataset.busy === '1') return; // 连点保护
    btn.dataset.busy = '1';
    btn.disabled = true;
    btn.textContent = '下载中…';
    say('正在连接镜像（jsDelivr → gh-proxy → GitHub 原文），大词库可能要十几秒…');
    // 提示失败绝不能把按钮卡在忙碌态（那就是「点了没反应」），单独兜住
    try { U().toast(`正在下载「${meta.name || id}」…`); } catch (_) { /* 忽略 */ }

    try {
      const r = await API.downloadBook(id, card?.dataset.lang || null);
      say(`已导入 ${r.imported} 词${r.skipped ? `，跳过 ${r.skipped}` : ''}`, 'ok');
      btn.textContent = '重新下载';
      U().toast(r.message || '下载完成', 'ok');
      loadCatalog();
      loadBooks();
      window.Library.loadList();
    } catch (e) {
      // 失败原因往往有好几行（每个镜像各一行），卡片内更易读、也方便复制
      say('下载失败。\n' + (e && e.message ? e.message : String(e)), 'err');
      btn.textContent = '重试下载';
      U().toast(`「${meta.name || id}」下载失败，详见卡片提示`, 'err');
    } finally {
      btn.disabled = false;
      btn.dataset.busy = '';
    }
  }

  /** 已背过的单词（需求 4）。 */
  async function loadLearned() {
    const box = document.getElementById('learned-list');
    const cnt = document.getElementById('learned-count');
    if (!box) return;
    box.innerHTML = U().loadingHtml();
    let rows;
    try {
      rows = await API.reviewedWords(null, 300, 0);
    } catch (e) {
      box.innerHTML = `<div class="muted" style="padding:20px">加载失败：${U().esc(e.message)}</div>`;
      return;
    }
    if (cnt) cnt.textContent = `共 ${rows.length} 个`;
    if (!rows.length) {
      box.innerHTML = `<div class="empty-state"><div class="es-icon">&#128214;</div>
        <p>还没有背过的单词</p><p class="muted">开始一轮背诵后，这里会记录你的学习足迹</p></div>`;
      return;
    }
    renderWordRows(box, rows);
    reapplySearch();
  }

  /** 通用：渲染单词行（可点击开详情卡）。 */
  function renderWordRows(box, rows) {
    box.innerHTML = rows.map(r => {
      const e = r.entry || {};
      const def = (e.senses && e.senses[0])
        ? `${e.senses[0].pos || ''} ${e.senses[0].definition || ''}`.trim()
        : '<span class="muted">暂无释义</span>';
      return `<div class="word-row" data-word="${U().esc(r.word)}">
        <div class="wr-word">${U().esc(r.word)}</div>
        <div class="wr-phon">${U().esc((e.phonetic && e.phonetic.uk) || '')}</div>
        <div class="wr-def">${def}</div>
        <div class="wr-meta">${U().speakBtn(e, 'us', '发音')}</div>
      </div>`;
    }).join('');
    box.querySelectorAll('.word-row').forEach(row => {
      row.addEventListener('click', async (ev) => {
        if (ev.target.closest('.speak-btn')) return;
        try {
          const res = await API.lookup(row.dataset.word);
          Detail.open(res.entry);
        } catch (e) { U().toast(e.message, 'err'); }
      });
    });
    U().speakBind(box);
  }

  /* ---- 词库文件导入 ---- */

  function openImportDialog(bookId) {
    const ov = document.getElementById('bookimport-overlay');
    if (!ov) return;
    ov.classList.remove('hidden');
    const fileInput = document.getElementById('bi-file');
    if (fileInput) fileInput.value = '';
    const info = document.getElementById('bi-fileinfo');
    if (info) info.textContent = '';
    ov.dataset.targetBook = bookId || '';
  }

  function closeImportDialog() {
    document.getElementById('bookimport-overlay')?.classList.add('hidden');
  }

  function onFilePicked(e) {
    const f = e.target.files && e.target.files[0];
    if (!f) return;
    const info = document.getElementById('bi-fileinfo');
    if (info) info.textContent = `${f.name}（${(f.size / 1024).toFixed(1)} KB）`;
    const reader = new FileReader();
    reader.onload = () => {
      document.getElementById('bi-content').value = String(reader.result || '');
      if (!document.getElementById('bi-name').value) {
        document.getElementById('bi-name').value = f.name.replace(/\.[^.]+$/, '');
      }
    };
    reader.readAsText(f, 'utf-8');
  }

  async function doBookImport() {
    const name = document.getElementById('bi-name').value.trim();
    const category = document.getElementById('bi-category').value;
    const source = document.getElementById('bi-source').value.trim();
    const content = document.getElementById('bi-content').value;
    if (!content.trim()) { U().toast('请粘贴内容或选择文件', 'err'); return; }
    if (!name) { U().toast('请填写词库名称', 'err'); return; }

    U().toast('正在导入…');
    try {
      const r = await API.importWordsToBook(content, null, name, null, source || 'local', null);
      U().toast(r.message || `导入完成，共 ${r.imported} 词`, 'ok');
      closeImportDialog();
      document.getElementById('bi-content').value = '';
      switchTab('books');
    } catch (e) {
      U().toast('导入失败：' + e.message, 'err');
    }
  }

  return { bind, loadBooks, loadCatalog, loadLearned, switchTab, renderWordRows, openImportDialog };
})();

const Library = (() => {
  const { API } = window.WordWiseAPI;
  const U = () => window.WW;

  let page = 0;
  const PAGE_SIZE = 50;
  let keyword = '';
  let bookFilter = '';

  /**
   * 顶部搜索框在每个标签页都要有反应：
   * 「单词列表」走后端搜索（结果可能很多，分页更合适），
   * 「我的词库 / 在线词库 / 已背过的词」直接在已渲染的列表上做本地过滤。
   */
  const SEARCH_TARGETS = {
    books: { list: 'lib-books', row: '.book-node', title: '没有匹配的词库' },
    catalog: { list: 'lib-catalog', row: '.catalog-card', title: '没有匹配的在线词库' },
    learned: { list: 'learned-list', row: '.word-row', title: '没有匹配的单词' },
  };
  const searchers = {};

  function currentTab() {
    const t = document.querySelector('.lib-tab.active');
    return t ? t.dataset.libtab : 'books';
  }

  /** 列表重绘后重新套用当前关键词。 */
  function applySearch() {
    const s = searchers[currentTab()];
    if (s) s.apply();
  }

  function bind() {
    // 本地过滤型的搜索（与顶部 lib-search 共用同一个输入框）
    const searchInput = document.getElementById('lib-search');
    if (searchInput) {
      Object.keys(SEARCH_TARGETS).forEach(tab => {
        const t = SEARCH_TARGETS[tab];
        searchers[tab] = U().attachListSearch({
          input: searchInput,
          list: t.list,
          row: t.row,
          empty: { title: t.title, hint: '换个关键词试试，或清空搜索框查看全部' },
        });
      });
    }

    // 后端搜索型（仅「单词列表」标签）
    searchInput?.addEventListener('input', U().debounce((e) => {
      if (currentTab() !== 'words') return;
      keyword = e.target.value.trim();
      page = 0;
      loadList();
    }, 300));

    document.getElementById('lib-prev')?.addEventListener('click', () => {
      if (page > 0) { page--; loadList(); }
    });
    document.getElementById('lib-next')?.addEventListener('click', () => {
      page++;
      loadList();
    });

    // 批量导入对话框
    document.getElementById('btn-import-dialog')?.addEventListener('click', () => {
      document.getElementById('import-overlay').classList.remove('hidden');
    });
    document.getElementById('import-close')?.addEventListener('click', closeImport);
    document.getElementById('import-cancel')?.addEventListener('click', closeImport);
    document.getElementById('import-overlay')?.addEventListener('click', (e) => {
      if (e.target.id === 'import-overlay') closeImport();
    });
    document.getElementById('import-confirm')?.addEventListener('click', doImport);

    document.getElementById('btn-export')?.addEventListener('click', async () => {
      try {
        const p = await API.exportData();
        U().toast('已导出到：' + p, 'ok');
      } catch (e) { U().toast(e.message, 'err'); }
    });

    document.getElementById('btn-import-file')?.addEventListener('click', async () => {
      // 提示用户手动选择文件路径（Tauri 文件对话框需额外插件，这里用输入框更稳妥）
      const p = prompt('请输入备份文件路径（JSON）：');
      if (!p) return;
      try {
        const r = await API.importData(p);
        U().toast(`导入完成：新增 ${r.added} 条，跳过 ${r.skipped} 条`, 'ok');
        loadList();
      } catch (e) { U().toast(e.message, 'err'); }
    });
  }

  function closeImport() {
    document.getElementById('import-overlay').classList.add('hidden');
  }

  async function doImport() {
    const raw = document.getElementById('import-text').value;
    const prefetch = document.getElementById('import-prefetch').checked;

    // 支持换行、逗号、分号、空格分隔
    const words = raw.split(/[\n,;，；\s]+/)
      .map(s => s.trim())
      .filter(Boolean);

    if (!words.length) { U().toast('请输入至少一个单词', 'err'); return; }

    U().toast(`正在导入 ${words.length} 个单词…`);
    try {
      const r = await API.importWords(words, null, prefetch);
      U().toast(`导入完成：新增 ${r.added}，跳过 ${r.skipped}，补全释义 ${r.prefetched}`, 'ok');
      closeImport();
      document.getElementById('import-text').value = '';
      page = 0;
      loadList();
    } catch (e) {
      U().toast('导入失败：' + e.message, 'err');
    }
  }

  async function loadList() {
    const box = document.getElementById('lib-list');
    if (!box) return;
    box.innerHTML = U().loadingHtml();

    let rows;
    try {
      if (bookFilter) {
        rows = await API.wordsInBook(bookFilter, null, PAGE_SIZE, page * PAGE_SIZE);
      } else if (keyword) {
        rows = await API.searchWords(keyword, null, 100);
      } else {
        rows = await API.listWords(null, PAGE_SIZE, page * PAGE_SIZE);
      }
    } catch (e) {
      box.innerHTML = `<div class="muted" style="padding:20px">加载失败：${U().esc(e.message)}</div>`;
      return;
    }

    if (!rows || !rows.length) {
      box.innerHTML = `<div class="empty-state">
        <div class="es-icon">&#128218;</div>
        <p>${keyword ? '没有匹配的单词' : '词库还是空的'}</p>
        <p class="muted">点击右上角「批量导入」添加单词，或在背诵页载入示例词库</p>
      </div>`;
      document.getElementById('lib-page-info').textContent = '第 0 页';
      return;
    }

    // 并行取每个词的学习状态
    const states = await Promise.all(rows.map(r =>
      API.wordState(r.word).catch(() => null)));

    box.innerHTML = rows.map((r, i) => {
      const st = states[i];
      const e = r.entry;
      const def = (e.senses && e.senses[0])
        ? `${e.senses[0].pos || ''} ${e.senses[0].definition || ''}`.trim()
        : '<span class="muted">暂无释义</span>';
      const mastery = st ? st.state.mastery : 0;
      const cls = U().masteryClass(mastery);
      const flags = [];
      if (st && st.state.is_leech) flags.push('<span class="tag orange">强化</span>');
      if (st && st.state.is_mastered) flags.push('<span class="tag green">已掌握</span>');
      return `<div class="word-row" data-word="${U().esc(r.word)}">
        <div class="wr-word">${U().esc(r.word)}</div>
        <div class="wr-phon">${U().esc((e.phonetic && e.phonetic.uk) || '')}</div>
        <div class="wr-def">${def}</div>
        <div class="wr-meta">
          ${flags.join('')}
          <div class="mastery-bar" title="熟练度 ${mastery}%">
            <div class="mastery-fill ${cls}" style="width:${mastery}%"></div>
          </div>
        </div>
      </div>`;
    }).join('');

    box.querySelectorAll('.word-row').forEach(row => {
      row.addEventListener('click', async () => {
        const w = row.dataset.word;
        try {
          const res = await API.lookup(w);
          Detail.open(res.entry);
        } catch (e) {
          U().toast(e.message, 'err');
        }
      });
    });

    const info = document.getElementById('lib-page-info');
    if (info) {
      info.textContent = bookFilter
        ? `词库内 ${rows.length} 条`
        : keyword ? `搜索结果 ${rows.length} 条` : `第 ${page + 1} 页`;
    }
    document.getElementById('lib-prev').disabled = page === 0;
    document.getElementById('lib-next').disabled = rows.length < PAGE_SIZE;

    const cnt = document.getElementById('lib-count');
    if (cnt) cnt.textContent = `共 ${rows.length} 个单词${keyword ? '（已筛选）' : ''}`;
  }

  /** 按词库筛选单词列表（需求 5）。 */
  function filterByBook(bookId) {
    bookFilter = bookId || '';
    page = 0;
    loadList();
  }

  return { bind, loadList, filterByBook, applySearch };
})();

/* ---------------- 错词本 ---------------- */

const Leech = (() => {
  const { API } = window.WordWiseAPI;
  const U = () => window.WW;

  let search = null;

  async function load() {
    const box = document.getElementById('leech-list');
    if (!box) return;
    box.innerHTML = U().loadingHtml();

    let items;
    try {
      items = await API.leechList(200);
    } catch (e) {
      box.innerHTML = `<div class="muted" style="padding:20px">加载失败：${U().esc(e.message)}</div>`;
      return;
    }

    // 更新导航角标
    const badge = document.getElementById('nav-leech-badge');
    if (badge) {
      if (items && items.length) {
        badge.textContent = items.length > 99 ? '99+' : items.length;
        badge.classList.remove('hidden');
      } else {
        badge.classList.add('hidden');
      }
    }

    if (!items || !items.length) {
      box.innerHTML = `<div class="empty-state">
        <div class="es-icon">&#127881;</div>
        <p>太棒了，暂时没有需要强化的词</p>
        <p class="muted">答错的单词会自动进入这里，并按遗忘曲线高频重现</p>
      </div>`;
      return;
    }

    box.innerHTML = items.map(it => {
      const s = it.state;
      const e = it.entry;
      const def = (e.senses && e.senses[0])
        ? `${e.senses[0].pos || ''} ${e.senses[0].definition || ''}`.trim()
        : '暂无释义';
      const rate = (s.correct_count + s.wrong_count) > 0
        ? Math.round((s.wrong_count / (s.correct_count + s.wrong_count)) * 100) : 0;
      return `<div class="word-row" data-word="${U().esc(it.entry.word)}">
        <div class="wr-word">${U().esc(it.entry.word)}</div>
        <div class="wr-phon">${U().esc((e.phonetic && e.phonetic.uk) || '')}</div>
        <div class="wr-def">${U().esc(def)}</div>
        <div class="wr-meta">
          <span class="tag orange">错误率 ${rate}%</span>
          <span class="tag">错 ${s.wrong_count} 次</span>
          <button class="ghost-btn xs btn-clear" data-word="${U().esc(it.entry.word)}">移出</button>
        </div>
      </div>`;
    }).join('');

    box.querySelectorAll('.word-row').forEach(row => {
      row.addEventListener('click', (e) => {
        if (e.target.classList.contains('btn-clear')) return;
        Detail.open(
          items.find(x => x.entry.word === row.dataset.word).entry,
          { reason: '强化记忆词' }
        );
      });
    });

    box.querySelectorAll('.btn-clear').forEach(btn => {
      btn.addEventListener('click', async (e) => {
        e.stopPropagation();
        try {
          await API.clearLeech(btn.dataset.word);
          U().toast('已移出强化队列', 'ok');
          load();
        } catch (err) { U().toast(err.message, 'err'); }
      });
    });

    // 搜索：本地过滤已渲染的行，保留「移出 / 开详情卡」等已绑定事件
    if (!search) {
      search = U().attachListSearch({
        input: 'leech-search',
        list: 'leech-list',
        row: '.word-row',
        empty: { title: '没有匹配的错词', hint: '换个关键词试试，或清空搜索框查看全部' },
      });
    } else {
      search.apply();
    }
  }

  return { load };
})();

/* ---------------- 复习计划 ---------------- */

const Plan = (() => {
  const { API } = window.WordWiseAPI;
  const U = () => window.WW;

  async function load() {
    const box = document.getElementById('plan-list');
    if (!box) return;
    box.innerHTML = U().loadingHtml();

    let plan;
    try {
      plan = await API.reviewPlan(14);
    } catch (e) {
      box.innerHTML = `<div class="muted" style="padding:20px">加载失败：${U().esc(e.message)}</div>`;
      return;
    }

    const max = Math.max(1, ...plan.map(p => p.count + p.overdue));

    if (!plan.length) {
      box.innerHTML = '<div class="muted">暂无安排</div>';
      return;
    }

    box.innerHTML = plan.map(p => {
      const total = p.count + p.overdue;
      const pct = Math.round((total / max) * 100);
      return `<div class="plan-row ${p.is_today ? 'today' : ''}">
        <div class="plan-date">${U().fmtDay(p.date)}</div>
        <div class="plan-week">${U().esc(p.weekday)}</div>
        <div class="plan-bar"><div class="plan-fill" style="width:${pct}%"></div></div>
        <div class="plan-count">
          ${total} 词
          ${p.overdue ? `<div class="plan-overdue">含逾期 ${p.overdue}</div>` : ''}
        </div>
      </div>`;
    }).join('');

    // 记忆周期表可视化
    const track = document.getElementById('plan-intervals');
    if (track) {
      let cfg = null;
      try { cfg = await API.getConfig(); } catch (e) {}
      const intervals = (cfg && cfg.srs && cfg.srs.base_intervals) || [1, 2, 4, 7, 15, 30, 90, 180];
      track.innerHTML = intervals.map((v, i) => `
        <div class="interval-node">
          <div class="iv-num">第 ${i + 1} 次</div>
          <div class="iv-val">${v < 1 ? Math.round(v * 24) + 'h' : v + 'd'}</div>
        </div>`).join('');
    }
  }

  return { load };
})();

/* ---------------- 统计 ---------------- */

const Stats = (() => {
  const { API } = window.WordWiseAPI;
  const U = () => window.WW;

  async function load() {
    const stamp = document.getElementById('stamp-now');
    if (stamp) {
      stamp.textContent = '数据更新于 ' + new Date().toLocaleString('zh-CN');
    }

    let s;
    try {
      s = await API.stats();
    } catch (e) {
      return;
    }

    const cards = document.getElementById('stats-cards');
    if (cards) {
      const acc = s.reviewed_today > 0
        ? Math.round((s.correct_today / s.reviewed_today) * 100) : 0;
      cards.innerHTML = `
        <div class="stat-card accent-blue">
          <div class="sc-label">词库总量</div>
          <div class="sc-value">${s.total_words}</div>
          <div class="sc-hint">已学习 ${s.learned}</div>
        </div>
        <div class="stat-card accent-green">
          <div class="sc-label">今日复习</div>
          <div class="sc-value">${s.reviewed_today}</div>
          <div class="sc-hint">正确率 ${acc}%</div>
        </div>
        <div class="stat-card accent-orange">
          <div class="sc-label">强化记忆</div>
          <div class="sc-value">${s.leeches}</div>
          <div class="sc-hint">已掌握 ${s.mastered}</div>
        </div>
        <div class="stat-card accent-purple">
          <div class="sc-label">连续学习</div>
          <div class="sc-value">${s.streak_days}<small>天</small></div>
          <div class="sc-hint">待复习 ${s.due_today}</div>
        </div>`;
    }

    U().renderBarChart(document.getElementById('stats-chart'), s.history);
    renderMasteryDist(s);
  }

  function renderMasteryDist(s) {
    const box = document.getElementById('mastery-dist');
    if (!box) return;
    const total = Math.max(1, s.total_words);
    const buckets = [
      { label: '未学习', value: Math.max(0, s.total_words - s.learned), color: '#ccd3e0' },
      { label: '学习中', value: Math.max(0, s.learned - s.mastered - s.leeches), color: '#1a6ce8' },
      { label: '需强化', value: s.leeches, color: '#f5a623' },
      { label: '已掌握', value: s.mastered, color: '#17a673' },
    ];
    box.innerHTML = buckets.map(b => `
      <div class="md-row">
        <div class="md-label">${b.label}</div>
        <div class="md-bar">
          <div class="md-fill" style="width:${Math.round((b.value / total) * 100)}%;background:${b.color}"></div>
        </div>
        <div class="md-count">${b.value}</div>
      </div>`).join('');
  }

  return { load };
})();

window.Library = Library;
window.Books = Books;
window.Leech = Leech;
window.Plan = Plan;
window.Stats = Stats;
