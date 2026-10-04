/* ============================================================
   library.js —— 词库管理、错词本、复习计划、统计
   ============================================================ */

const Library = (() => {
  const { API } = window.WordWiseAPI;
  const U = () => window.WW;

  let page = 0;
  const PAGE_SIZE = 50;
  let keyword = '';

  function bind() {
    document.getElementById('lib-search')?.addEventListener('input', U().debounce((e) => {
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
      rows = keyword
        ? await API.searchWords(keyword, null, 100)
        : await API.listWords(null, PAGE_SIZE, page * PAGE_SIZE);
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
      info.textContent = keyword ? `搜索结果 ${rows.length} 条` : `第 ${page + 1} 页`;
    }
    document.getElementById('lib-prev').disabled = page === 0;
    document.getElementById('lib-next').disabled = rows.length < PAGE_SIZE;

    const cnt = document.getElementById('lib-count');
    if (cnt) cnt.textContent = `共 ${rows.length} 个单词${keyword ? '（已筛选）' : ''}`;
  }

  return { bind, loadList };
})();

/* ---------------- 错词本 ---------------- */

const Leech = (() => {
  const { API } = window.WordWiseAPI;
  const U = () => window.WW;

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
window.Leech = Leech;
window.Plan = Plan;
window.Stats = Stats;
