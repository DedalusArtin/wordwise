/* ============================================================
   graph.js —— 知识图谱（需求 1）

   一个独立于查词/翻译的栏目：把词与词的关系画成一张图。
   词是节点，关系是边。

   几个关键取舍：

   - **不引第三方可视化库**。项目前端是「<script> 顺序加载 + 全局作用域」，
     没有打包器，引 D3/ECharts 就意味着多一个 CDN 依赖（离线即失效）。
     自研一个力导向布局只有一百多行，反而更可控。

   - **布局在前端做，节点数上限 150**。
     力导向是 O(n²) 的斥力计算，300 个节点时每帧 9 万次计算，
     低配机器上会掉帧。后端已经按权重截断过边数，这里再兜一层。

   - **AI 发散出来的边用虚线**。词典里写明的关系是确定的，模型猜的是
     推测的，两者视觉上必须分开，否则用户会把模型编的词当真。

   - **点一下节点 = 以它为中心重新展开**（向后端要 1-2 跳子图），
     而不是前端在已有数据里过滤 —— 后端的边可能比当前这张图多得多。
   ============================================================ */

const Graph = (() => {
  const { API } = window.WordWiseAPI;
  const U = () => window.WW;

  /** 布局节点上限。超过就只画度数最高的一批。 */
  const MAX_NODES = 150;
  /** 布局迭代总帧数。收敛后停止动画，别一直烧 CPU。 */
  const MAX_TICKS = 320;

  /* 关系配色：与后端 RELS 顺序一致，前端按 code 查表 */
  const REL_COLOR = {
    synonym: '#30a46c',
    antonym: '#e5484d',
    derived: '#3e63dd',
    related: '#8e8c99',
    hypernym: '#8e4ec6',
    hyponym: '#f76b15',
  };
  function relColor(rel) {
    return REL_COLOR[rel] || '#8e8c99';
  }
  function relName(rel) {
    const hit = rels.find(r => r.code === rel);
    return hit ? hit.name : rel;
  }

  let rels = [];
  let data = null;
  let nodes = [];
  let edges = [];
  let stats = null;
  let center = null;
  let hiddenRels = new Set();
  let onlyDict = false;
  let busy = false;

  let view = { tx: 0, ty: 0, k: 1 };
  let tick = 0;
  let raf = null;
  let drag = null;
  let pan = null;
  let selected = null;

  const $ = (id) => document.getElementById(id);

  /* ---------------- 数据 ---------------- */

  async function loadRels() {
    try {
      rels = (await API.graphRels()) || [];
    } catch (e) {
      rels = [
        { code: 'synonym', name: '同义' },
        { code: 'antonym', name: '反义' },
        { code: 'derived', name: '派生' },
        { code: 'related', name: '相关' },
        { code: 'hypernym', name: '上义' },
        { code: 'hyponym', name: '下义' },
      ];
    }
  }

  /**
   * 进页面时调用。
   *
   * 只第一次真正取子图：之后切走再切回来只刷新统计数字，
   * 否则用户调好的缩放/平移每次都白费。
   */
  let ready = false;
  async function load(force) {
    if (!rels.length) await loadRels();
    renderRelFilter();
    await refreshStats();
    if (!ready || force) {
      ready = true;
      await viewGraph(center, 1);
    }
  }

  async function refreshStats() {
    try {
      stats = await API.graphStats();
    } catch (e) {
      stats = null;
    }
    renderStats();
  }

  /** 取一张子图并绘制。 */
  async function viewGraph(word, depth) {
    const svg = $('graph-svg');
    center = word || null;
    try {
      data = await API.graphView(center, depth || 1);
    } catch (e) {
      if (svg) {
        svg.innerHTML = `<text x="24" y="32" fill="#e5484d" font-size="14">读取图谱失败：${U().esc(e.message)}</text>`;
      }
      return;
    }
    applyData();
  }

  /** 把后端数据转成布局状态。 */
  function applyData() {
    if (!data) return;
    // 没有可画的东西时盖一层提示，别留一片空白让人以为卡住了
    $('graph-empty')?.classList.toggle('hidden', (data.nodes || []).length > 0);
    // 节点太多时按度数裁剪：低配机器上 O(n²) 的斥力算不动
    let ns = (data.nodes || []).slice();
    if (ns.length > MAX_NODES) {
      ns.sort((a, b) => b.degree - a.degree);
      ns = ns.slice(0, MAX_NODES);
      const keep = new Set(ns.map(n => n.word));
      edges = (data.edges || []).filter(e => keep.has(e.src) && keep.has(e.dst));
    } else {
      edges = (data.edges || []).slice();
    }

    nodes = ns.map((n, i) => {
      // 初始位置按圆环撒开 —— 从同一个点出发会让斥力把它们全推到一条直线上
      const a = (i / Math.max(1, ns.length)) * Math.PI * 2;
      const r = 90 + (n.degree || 0) * 4;
      return {
        word: n.word,
        degree: n.degree || 0,
        in_dict: !!n.in_dict,
        gloss: n.gloss || '',
        mastery: n.mastery,
        x: Math.cos(a) * r,
        y: Math.sin(a) * r,
        vx: 0, vy: 0,
      };
    });

    view = { tx: 0, ty: 0, k: 1 };
    tick = 0;
    selected = null;
    startLayout();
    renderRelFilter();
    renderStats();
    renderCenterBar();
  }

  /* ---------------- 力导向布局 ---------------- */

  function startLayout() {
    stopLayout();
    if (!nodes.length) {
      draw();
      return;
    }
    const step = () => {
      const moved = iterate();
      draw();
      tick++;
      // 收敛就早停：固定跑满 320 帧要 5 秒多，低配机器上纯属白烧 CPU。
      // 位移已经很小说明布局稳定了，继续画也看不出差别。
      const settled = moved < 0.4;
      if (tick < MAX_TICKS && !drag && !settled) {
        raf = requestAnimationFrame(step);
      } else {
        raf = null;
      }
    };
    raf = requestAnimationFrame(step);
  }

  function stopLayout() {
    if (raf && typeof cancelAnimationFrame === 'function') {
      cancelAnimationFrame(raf);
    }
    raf = null;
  }

  function iterate() {
    const n = nodes.length;
    const REP = 2600;      // 斥力强度
    const SPRING = 0.012;  // 弹簧系数
    const LEN = 110;       // 理想边长
    const CENTER = 0.004;  // 向心力（防止图飘走）
    const DAMP = 0.86;

    for (let i = 0; i < n; i++) {
      const a = nodes[i];
      for (let j = i + 1; j < n; j++) {
        const b = nodes[j];
        let dx = b.x - a.x;
        let dy = b.y - a.y;
        let d2 = dx * dx + dy * dy;
        if (d2 < 1) {
          // 完全重合时给一个确定性的微小偏移，避免除零和左右抖动
          dx = ((i + j) % 7) * 0.01 + 0.01;
          dy = ((i * j) % 5) * 0.01 + 0.01;
          d2 = dx * dx + dy * dy;
        }
        const d = Math.sqrt(d2);
        const f = REP / d2;
        const fx = (dx / d) * f;
        const fy = (dy / d) * f;
        a.vx -= fx; a.vy -= fy;
        b.vx += fx; b.vy += fy;
      }
    }

    for (const e of edges) {
      const a = nodes.find(x => x.word === e.src);
      const b = nodes.find(x => x.word === e.dst);
      if (!a || !b) continue;
      const dx = b.x - a.x;
      const dy = b.y - a.y;
      const d = Math.hypot(dx, dy) || 1;
      // 权重越高（关系越确定）拉得越紧；AI 的边权重低 → 排得松
      const f = (d - LEN) * SPRING * (e.weight || 1);
      const fx = (dx / d) * f;
      const fy = (dy / d) * f;
      a.vx += fx; a.vy += fy;
      b.vx -= fx; b.vy -= fy;
    }

    let moved = 0;
    for (const p of nodes) {
      p.vx -= p.x * CENTER * 100;
      p.vy -= p.y * CENTER * 100;
      if (p === drag) {
        p.vx = 0; p.vy = 0;
        continue;
      }
      p.vx *= DAMP; p.vy *= DAMP;
      p.x += p.vx;
      p.y += p.vy;
      moved += Math.abs(p.vx) + Math.abs(p.vy);
    }
    return moved;
  }

  /* ---------------- 绘制 ---------------- */

  function draw() {
    const svg = $('graph-svg');
    if (!svg) return;
    const box = svg.parentElement;
    const w = (box && box.clientWidth) || 900;
    const h = (box && box.clientHeight) || 520;
    svg.setAttribute('viewBox', `0 0 ${w} ${h}`);
    svg.setAttribute('width', String(w));
    svg.setAttribute('height', String(h));

    const cx = w / 2 + view.tx;
    const cy = h / 2 + view.ty;
    const k = view.k;

    const shown = edges.filter(e => !hiddenRels.has(e.rel));
    const visible = new Set();
    shown.forEach(e => { visible.add(e.src); visible.add(e.dst); });

    const lines = shown.map(e => {
      const a = nodes.find(x => x.word === e.src);
      const b = nodes.find(x => x.word === e.dst);
      if (!a || !b) return '';
      const dashed = e.source === 'llm' ? ' stroke-dasharray="5 4"' : '';
      const hi = selected && (e.src === selected || e.dst === selected);
      return `<line x1="${cx + a.x * k}" y1="${cy + a.y * k}" x2="${cx + b.x * k}" y2="${cy + b.y * k}"
        stroke="${relColor(e.rel)}" stroke-width="${hi ? 2.4 : 1.4}" opacity="${hi ? 0.95 : 0.45}"${dashed} />`;
    }).join('');

    const circles = nodes.map(p => {
      if (onlyDict && !p.in_dict) return '';
      const r = radius(p);
      const x = cx + p.x * k;
      const y = cy + p.y * k;
      // 词库里没有完整词条的节点画成空心：点它是「查词」而不是「看详情」
      const fill = p.in_dict ? (p.mastery != null && p.mastery >= 0.8 ? '#30a46c' : '#3e63dd') : 'transparent';
      const stroke = p.in_dict ? 'none' : '#8e8c99';
      const dim = !visible.has(p.word);
      const label = p.word.length > 16 ? p.word.slice(0, 15) + '…' : p.word;
      const isSel = p.word === selected;
      return `<g class="g-node" data-word="${U().esc(p.word)}" opacity="${dim ? 0.35 : 1}">
        ${isSel ? `<circle cx="${x}" cy="${y}" r="${r + 6}" fill="none" stroke="#3e63dd" stroke-width="2" />` : ''}
        <circle cx="${x}" cy="${y}" r="${r}" fill="${fill}" stroke="${stroke}" stroke-width="1.6" />
        <text x="${x}" y="${y + r + 13}" text-anchor="middle" font-size="${isSel ? 13 : 11.5}"
              fill="${isSel ? '#1c2024' : '#4a4a55'}" font-weight="${isSel ? 700 : 500}">${U().esc(label)}</text>
      </g>`;
    }).join('');

    svg.innerHTML = `
      <rect x="0" y="0" width="${w}" height="${h}" fill="transparent" />
      <g id="graph-layer">${lines}${circles}</g>`;
  }

  function radius(p) {
    return 7 + Math.min(p.degree || 0, 12) * 1.5;
  }

  /* ---------------- 面板渲染 ---------------- */

  function renderRelFilter() {
    const box = $('graph-legend');
    if (!box) return;
    box.innerHTML = rels.map(r => `
      <button class="g-legend${hiddenRels.has(r.code) ? ' off' : ''}" data-rel="${r.code}">
        <span class="g-dot" style="background:${relColor(r.code)}"></span>${U().esc(r.name)}
      </button>`).join('');
    const cnt = $('graph-legend-toggle');
    if (cnt) {
      cnt.textContent = hiddenRels.size ? `已隐藏 ${hiddenRels.size} 类` : '';
    }
  }

  function renderStats() {
    const el = $('graph-summary');
    if (!el) return;
    if (!stats) {
      el.innerHTML = '<span class="muted">—</span>';
      return;
    }
    if (!stats.nodes) {
      el.innerHTML = '<span class="muted">还没有关系数据。点「构建图谱」从词库抽取关系。</span>';
      return;
    }
    el.innerHTML = `共 <b>${stats.nodes}</b> 个词 / <b>${stats.edges}</b> 条关系` +
      (data && data.total_edges > (data.edges || []).length
        ? ` <span class="muted">（当前显示 ${(data.edges || []).length} 条）</span>`
        : '');
  }

  function renderCenterBar() {
    const el = $('graph-center');
    if (!el) return;
    el.innerHTML = center
      ? `以 <b>${U().esc(center)}</b> 为中心 <button class="ghost-btn xs" id="graph-reset">回到全局</button>`
      : '<span class="muted">全局视图 · 点任意词以它为中心展开</span>';
    const btn = $('graph-reset');
    if (btn) btn.addEventListener('click', () => viewGraph(null, 1));
  }

  function showTip(p) {
    const tip = $('graph-tip');
    if (!tip) return;
    if (!p) {
      tip.classList.add('hidden');
      return;
    }
    const parts = [`<b>${U().esc(p.word)}</b>`];
    if (p.gloss) parts.push(U().esc(p.gloss));
    parts.push(p.in_dict
      ? `连接 ${p.degree} 条 · 点击查看`
      : `连接 ${p.degree} 条 · 词库外（模型发散）`);
    tip.innerHTML = parts.join('<br>');
    tip.classList.remove('hidden');
  }

  /* ---------------- 交互 ---------------- */

  function hitTest(svgX, svgY) {
    const box = $('graph-svg');
    if (!box) return null;
    const rect = box.getBoundingClientRect();
    const cx = rect.width / 2 + view.tx;
    const cy = rect.height / 2 + view.ty;
    for (const p of nodes) {
      const x = cx + p.x * view.k;
      const y = cy + p.y * view.k;
      const r = radius(p) + 6;
      if ((svgX - x) ** 2 + (svgY - y) ** 2 <= r * r) return p;
    }
    return null;
  }

  function bind() {
    const svg = $('graph-svg');
    if (!svg) return;

    svg.addEventListener('pointerdown', (e) => {
      const rect = svg.getBoundingClientRect();
      const x = e.clientX - rect.left;
      const y = e.clientY - rect.top;
      const p = hitTest(x, y);
      if (p) {
        drag = p;
        selected = p.word;
        showTip(p);
        svg.setPointerCapture && svg.setPointerCapture(e.pointerId);
      } else {
        pan = { x: e.clientX, y: e.clientY, tx: view.tx, ty: view.ty };
      }
    });

    svg.addEventListener('pointermove', (e) => {
      const rect = svg.getBoundingClientRect();
      if (drag) {
        const cx = rect.width / 2 + view.tx;
        const cy = rect.height / 2 + view.ty;
        drag.x = (e.clientX - rect.left - cx) / view.k;
        drag.y = (e.clientY - rect.top - cy) / view.k;
        draw();
        return;
      }
      if (pan) {
        view.tx = pan.tx + (e.clientX - pan.x);
        view.ty = pan.ty + (e.clientY - pan.y);
        draw();
        return;
      }
      const p = hitTest(e.clientX - rect.left, e.clientY - rect.top);
      showTip(p);
    });

    const endDrag = (e) => {
      if (drag) {
        drag = null;
        startLayout();   // 松手后让它重新收敛
      }
      pan = null;
      if (e && e.pointerId != null && svg.releasePointerCapture) {
        try { svg.releasePointerCapture(e.pointerId); } catch (err) {}
      }
    };
    svg.addEventListener('pointerup', endDrag);
    svg.addEventListener('pointerleave', endDrag);
    svg.addEventListener('pointercancel', endDrag);

    svg.addEventListener('wheel', (e) => {
      e.preventDefault();
      const factor = e.deltaY > 0 ? 0.9 : 1.1;
      view.k = Math.min(3, Math.max(0.3, view.k * factor));
      draw();
    }, { passive: false });

    // 点击节点：单点为「以它为中心展开」，双击为「打开详情卡」
    svg.addEventListener('dblclick', (e) => {
      const rect = svg.getBoundingClientRect();
      const p = hitTest(e.clientX - rect.left, e.clientY - rect.top);
      if (!p) return;
      if (p.in_dict && window.Detail && window.Detail.openWord) {
        window.Detail.openWord(p.word);
      } else {
        Pages && Pages.go('lookup');
        const input = document.getElementById('lk-input');
        if (input) {
          input.value = p.word;
          window.Lookup && window.Lookup.query(p.word);
        }
      }
    });

    svg.addEventListener('click', (e) => {
      const rect = svg.getBoundingClientRect();
      const p = hitTest(e.clientX - rect.left, e.clientY - rect.top);
      if (!p || p.word === lastClicked) return;
      lastClicked = p.word;
      // 与 dblclick 冲突：延后一点，双击时不会触发「重新展开」
      setTimeout(() => {
        if (lastClicked === p.word) {
          viewGraph(p.word, 2);
        }
      }, 260);
    });

    // 图例：点一下隐藏/显示某类关系
    $('graph-legend')?.addEventListener('click', (e) => {
      const btn = e.target.closest('[data-rel]');
      if (!btn) return;
      const rel = btn.dataset.rel;
      if (hiddenRels.has(rel)) hiddenRels.delete(rel);
      else hiddenRels.add(rel);
      renderRelFilter();
      draw();
    });

    // 候选词列表：点一下以该词为中心（渲染出来的按钮带 data-goto）
    $('graph-suggest-list')?.addEventListener('click', (e) => {
      const btn = e.target.closest('[data-goto]');
      if (btn) viewGraph(btn.dataset.goto, 2);
    });

    $('graph-search')?.addEventListener('input', (e) => {
      scheduleSearch(e.target.value);
    });
    $('graph-search')?.addEventListener('keydown', (e) => {
      if (e.key === 'Enter') {
        e.preventDefault();
        const v = e.target.value.trim();
        if (v) viewGraph(v, 2);
      }
    });

    $('graph-build')?.addEventListener('click', build);
    $('graph-clear')?.addEventListener('click', clearAll);
    $('graph-expand')?.addEventListener('click', expand);
    $('graph-suggest')?.addEventListener('click', showSuggestions);
    $('graph-zoom-in')?.addEventListener('click', () => { view.k = Math.min(3, view.k * 1.2); draw(); });
    $('graph-zoom-out')?.addEventListener('click', () => { view.k = Math.max(0.3, view.k / 1.2); draw(); });
    $('graph-fit')?.addEventListener('click', () => { view = { tx: 0, ty: 0, k: 1 }; draw(); });
  }

  let lastClicked = null;
  let searchTimer = null;

  function scheduleSearch(text) {
    clearTimeout(searchTimer);
    const box = $('graph-suggest-list');
    if (!box) return;
    searchTimer = setTimeout(async () => {
      try {
        const list = await API.graphSearch(text || '', null);
        if (!list.length) {
          box.innerHTML = '<span class="muted">没有匹配的词</span>';
          return;
        }
        box.innerHTML = list.slice(0, 12).map(w =>
          `<button class="g-suggest" data-goto="${U().esc(w)}">${U().esc(w)}</button>`).join('');
      } catch (e) {
        box.innerHTML = `<span class="muted">搜索失败：${U().esc(e.message)}</span>`;
      }
    }, 260);
  }

  function showSuggestions() {
    scheduleSearch('');
  }

  /* ---------------- 动作 ---------------- */

  async function build() {
    if (busy) return;
    busy = true;
    const btn = $('graph-build');
    if (btn) { btn.disabled = true; btn.textContent = '构建中…'; }
    try {
      const n = await API.graphBuild(null);
      U().toast(`已从词库抽取 ${n} 条新关系`, 'ok');
      await refreshStats();
      await viewGraph(center, 1);
    } catch (e) {
      U().toast(e.message, 'err');
    } finally {
      busy = false;
      if (btn) { btn.disabled = false; btn.textContent = '构建图谱'; }
    }
  }

  async function clearAll() {
    try {
      await API.graphClear(null);
      U().toast('已清空图谱关系', 'ok');
      await refreshStats();
      await viewGraph(null, 1);
    } catch (e) {
      U().toast(e.message, 'err');
    }
  }

  /** AI 发散：给当前中心词补关联词。 */
  async function expand() {
    const word = selected || center;
    if (!word) {
      U().toast('先在图上点一个词，或搜索一个词', 'err');
      return;
    }
    if (busy) return;
    busy = true;
    const btn = $('graph-expand');
    if (btn) { btn.disabled = true; btn.textContent = 'AI 发散中…'; }
    try {
      const n = await API.graphExpand(word, null);
      U().toast(`为「${word}」新增 ${n} 条关系`, 'ok');
      await refreshStats();
      await viewGraph(word, 2);
    } catch (e) {
      // 最常见的失败是没有配置本地大模型，提示要能直接指路
      U().toast(e.message + '（可在设置页「本地大模型」一键部署）', 'err');
    } finally {
      busy = false;
      if (btn) { btn.disabled = false; btn.textContent = 'AI 发散'; }
    }
  }

  return {
    bind, load, build, clearAll, expand, viewGraph,
    get center() { return center; },
    get nodeCount() { return nodes.length; },
    get edgeCount() { return edges.length; },
  };
})();

window.Graph = Graph;
