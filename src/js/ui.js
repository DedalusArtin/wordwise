/* ============================================================
   ui.js —— 通用界面工具
   词条渲染、提示、标签页、Markdown 轻解析等
   ============================================================ */

/* ---------------- 内联 SVG 图标（全站唯一来源） ----------------

   ★ 为什么界面图标一律用内联 SVG 而不是 emoji / 实体字符：
     emoji 的字形宽度由**字体**决定，而字体由**平台**决定。同一个 🔊
     在 Windows（Segoe UI Emoji）与 Android（Noto Color Emoji）上
     advance width 不同；同一个 &#9881;（⚙）在有的机器上被渲染成彩色
     emoji、有的机器上是纯文本符号。一列图标混着这两种行为，在 Windows
     上就是「底部图标对不齐」。
     内联 SVG 的尺寸与基线完全由 CSS 决定（见 app.css 的 `svg.ico`），
     与平台字体无关：写一次，到处一致。

   用法：`icon('sound')` 返回 `<svg class="ico" …>…</svg>`。
   颜色靠 `currentColor` 继承，所以悬停 / 选中态的换色自动跟随，
   不需要为每种状态各画一套。

   加图标只需在这里补一条 —— 别在别处再内联一份 SVG，
   否则「同一个图标两个尺寸」的问题会立刻回来。 */
const ICON_PATHS = {
  /* 发音 / 朗读 */
  sound: '<path d="M4.2 9.6h3L11.6 6v12l-4.4-3.6h-3Z"/><path d="M15.3 9.4a4 4 0 0 1 0 5.2"/><path d="M17.9 6.9a7.6 7.6 0 0 1 0 10.2"/>',
  /* 主题：浅色（太阳）/ 深色（月亮）/ 跟随系统 */
  sun: '<circle cx="12" cy="12" r="4.2"/><path d="M12 2.8v2.4M12 18.8v2.4M2.8 12h2.4M18.8 12h2.4M5.4 5.4l1.7 1.7M16.9 16.9l1.7 1.7M5.4 18.6l1.7-1.7M16.9 7.1l1.7-1.7"/>',
  moon: '<path d="M20.4 14.6A8.6 8.6 0 0 1 9.4 3.6a8.6 8.6 0 1 0 11 11Z"/>',
  system: '<rect x="3" y="4.6" width="18" height="12.4" rx="1.8"/><path d="M8.4 20.4h7.2"/><path d="M12 17v3.4"/>',
  /* 空状态 */
  book: '<path d="M12 6.7C10.4 5.3 8.1 4.7 4.4 4.7v12.5c3.7 0 6 .6 7.6 2 1.6-1.4 3.9-2 7.6-2V4.7c-3.7 0-6 .6-7.6 2Z"/><path d="M12 6.7v12.5"/>',
  search: '<circle cx="10.8" cy="10.8" r="6.3"/><path d="M15.5 15.5 20.5 20.5"/>',
  /* 通用符号 */
  check: '<path d="M5 12.8 9.6 17.4 19 8"/>',
  cross: '<path d="M6.4 6.4l11.2 11.2M17.6 6.4 6.4 17.6"/>',
  warn: '<path d="M12 4.4 3.3 19.6h17.4L12 4.4Z"/><path d="M12 9.9v4.4"/><path d="M12 17.2h.01"/>',
  back: '<path d="M19 12H5"/><path d="M11 6 5 12l6 6"/>',
  chevron: '<path d="M9.4 5.6 15.8 12l-6.4 6.4"/>',
  swap: '<path d="M4.6 8.4h13.2"/><path d="M14.6 5.2l3.2 3.2-3.2 3.2"/><path d="M19.4 15.6H6.2"/><path d="M9.4 12.4 6.2 15.6l3.2 3.2"/>',
  minus: '<path d="M5.4 12h13.2"/>',
  fit: '<path d="M4.6 9V4.8h4.2"/><path d="M19.4 15v4.2h-4.2"/><path d="M19.4 9V4.8h-4.2"/><path d="M4.6 15v4.2h4.2"/>',
  plus: '<path d="M12 5.4v13.2"/><path d="M5.4 12h13.2"/>',
};

/**
 * 生成一个内联 SVG 图标。
 *
 * @param {string} name  ICON_PATHS 里的键
 * @param {string} [cls] 追加的 class（如 'fill' 走实心填充）
 * @returns {string} SVG 标记；名字不认识时返回空串（**不抛异常**：
 *   图标缺失不该让整个词条渲染挂掉）
 */
function icon(name, cls) {
  const d = ICON_PATHS[name];
  if (!d) return '';
  return `<svg class="ico${cls ? ' ' + cls : ''}" viewBox="0 0 24 24" aria-hidden="true" focusable="false">${d}</svg>`;
}

/**
 * 发音按钮 HTML。
 *
 * ★ 必须是**顶层函数声明**，不能只挂 `window.WW.speakBtn`：
 *   `renderEntry()` 里用的是裸名字 `speakBtn(...)`，而裸名字只会往
 *   全局作用域找（`window.speakBtn`），找不到 `window.WW.speakBtn` 这个属性。
 *   只挂 WW 的话，只要 `showPhonetic !== false`（默认开启）就必然
 *   `ReferenceError: speakBtn is not defined`，结果就是查词永远转圈。
 *   speak.js 尚未加载时降级为空字符串。
 */
function speakBtn(entry, accent, label) {
  return window.Speak ? window.Speak.btnHtml(entry, accent, label) : '';
}

/**
 * 音标 HTML。全项目**所有**显示音标的位置都走它。
 *
 * 抽出来的三个理由：
 *   1. 标注必须跟着**词条语言**走 —— 中文词的拼音、日语词的假名读音、
 *      韩语词的罗马音，都不该被标成「英 / 美」；
 *   2. 只有拉丁字母的读音才用斜杠括起来，拼音加斜杠反而像乱码；
 *   3. IPA 有自己的一套字符（ˈ ɡ ʃ ʊ θ），必须用覆盖得住的字体栈，
 *      否则在部分机器上会掉成方框。字体在 CSS 的 --font-ipa 里统一配。
 *
 * @param {object} entry 词条（读 entry.phonetic 与 entry.lang）
 * @param {object} [opts] { size: 'sm'，speak: false 可去掉每条读音旁的小喇叭 }
 */
/**
 * 音标可信度清洗。
 *
 * 词典源会把**领域标签和杂数**塞进音标字段 —— 实测 `fault` 的 uk/us 都是
 * `"[地质]"`，旁边还跟着裸数字 `"41"`。这类值一旦入库就在所有展示位置现形
 * （词头、列表、详情卡全都走 phoneticHtml）。规则按词条语言判断：
 *   - 含 CJK / 假名 / 谚文 → 拒（领域标签、注释混进来了）；
 *   - 纯数字、长度 > 64 → 拒；
 *   - 不含 ≥2 个拉丁字母、也没有 IPA 修饰符（ˈ ˌ ː）→ 拒。
 * 宁可不显示音标（音标行会退回一个纯发音按钮），也不给用户看乱码。
 */
function isUsableIpa(raw) {
  const v = String(raw == null ? '' : raw).trim();
  if (!v || v.length > 64) return false;
  if (/[\u3400-\u4dbf\u4e00-\u9fff\u3040-\u30ff\uac00-\ud7af]/.test(v)) return false;
  if (/^\d+$/.test(v)) return false;
  // 字母按**计数**不按连续对：拼音带声调会打断连续（"nǐ hǎo" 没有任何
  // 连续两个 ASCII 字母，用 /[a-zA-Z]{2}/ 会把合法拼音整批误杀 —— 冒烟
  // 守卫「中文词条标拼音」抓过这个回归）。
  const latinCount = (v.match(/[a-zA-Z]/g) || []).length;
  if (latinCount < 2 && !/[ˈˌː]/.test(v)) return false;
  return true;
}

function phoneticHtml(entry, opts) {
  const o = opts || {};
  const ph = (entry && entry.phonetic) || {};
  const lg = String((entry && entry.lang) || 'en').toLowerCase().split(/[-_]/)[0];
  const withSpeak = o.speak !== false;

  const pairs = [];
  if (lg === 'zh') {
    const py = ph.uk || ph.us;
    if (py && isUsableIpa(py)) pairs.push({ tag: '拼音', ipa: py, accent: 'us' });
  } else if (lg === 'ja') {
    const rb = ph.uk || ph.us;
    if (rb && isUsableIpa(rb)) pairs.push({ tag: '读音', ipa: rb, accent: 'us' });
  } else if (lg === 'ko') {
    const rb = ph.uk || ph.us;
    if (rb && isUsableIpa(rb)) pairs.push({ tag: '罗马音', ipa: rb, accent: 'us' });
  } else {
    if (ph.uk && isUsableIpa(ph.uk)) pairs.push({ tag: '英', ipa: ph.uk, accent: 'uk' });
    if (ph.us && isUsableIpa(ph.us)) pairs.push({ tag: '美', ipa: ph.us, accent: 'us' });
  }
  // 一个都没有时不留一个空壳，交给调用方决定要不要放个纯发音按钮
  if (!pairs.length) return '';

  const items = pairs.map((p) => {
    const raw = String(p.ipa == null ? '' : p.ipa).trim().replace(/^\/+|\/+$/g, '');
    if (!raw) return '';
    // 斜杠是「音标」的符号。中文的拼音、日语的假名读音、韩语的罗马音都不是音标，
    // 加斜杠反而像乱码 —— 所以按**词条语言**判断，而不是按字符集猜。
    const slashed = !['zh', 'ja', 'ko'].includes(lg);
    const ipa = slashed ? `/${esc(raw)}/` : esc(raw);
    return `<span class="phon-item">
      <i class="phon-tag">${esc(p.tag)}</i>
      <span class="phon-ipa">${ipa}</span>
      ${withSpeak ? speakBtn(entry, p.accent, p.tag + '发音') : ''}
    </span>`;
  }).join('');

  if (!items) return '';
  return `<span class="phon${o.size === 'sm' ? ' phon-sm' : ''}">${items}</span>`;
}

/** HTML 转义，防止词条内容破坏结构。 */
function esc(s) {
  if (s === null || s === undefined) return '';
  return String(s)
    .replace(/&/g, '&amp;')
    .replace(/</g, '&lt;')
    .replace(/>/g, '&gt;')
    .replace(/"/g, '&quot;')
    .replace(/'/g, '&#39;');
}

/**
 * 把 related 拆成一个个真正可查的词。
 *
 * 后端 `split_related()` 已经拆过一遍，但**旧缓存里存的是拆之前的数据**
 * （dict_cache TTL 30 天），所以前端必须再兜一层 —— 否则老用户点相关词
 * 仍然是拿 "desert, abandon, leave" 整串去查，结果显示「查询失败」。
 */
function splitRelated(list) {
  const out = [];
  const seps = /[,;，；、/|\n\r\t·]+/;
  const edge = /^[.。,，()\[\]"']+|[.。,，()\[\]"']+$/g;
  const clean = (s) => String(s).trim().replace(edge, '');
  for (const raw of (list || [])) {
    if (typeof raw !== 'string') continue;
    for (const piece of raw.split(seps)) {
      const p = clean(piece);
      if (!p) continue;
      // CJK 串里的空格常是词组的一部分，保留原样；拉丁串按空格再拆
      const words = /[\u3040-\u30ff\u4e00-\u9fff\uac00-\ud7af]/.test(p) ? [p] : p.split(/\s+/);
      for (const raw2 of words) {
        const w = clean(raw2);
        if (!w || w.length > 30) continue;
        if (!out.includes(w)) out.push(w);
      }
    }
  }
  return out.slice(0, 20);
}

/** 全局轻提示。 */
let toastTimer = null;
function toast(msg, type = '') {
  const el = document.getElementById('toast');
  if (!el) return;
  el.textContent = msg;
  el.className = 'toast ' + type;
  el.classList.remove('hidden');
  clearTimeout(toastTimer);
  toastTimer = setTimeout(() => el.classList.add('hidden'), type === 'err' ? 5200 : 2600);
}

/** 加载中占位。 */
function loadingHtml(text = '加载中…') {
  return `<div class="loading"><div class="spinner"></div><span>${esc(text)}</span></div>`;
}

/**
 * 渲染词条。
 *
 * 两种排布，共用同一份头部与同一份义项渲染：
 *  - 默认（`tabs: false`）：头部 + 各块**纵向堆叠**。对应词卡片用它 ——
 *    那里的词头已经画在卡片标题上了，再套一排标签只是让用户多点一次。
 *  - `tabs: true`：头部 + **分块卡片 + 底部标签切换**（查词结果页用它）。
 *    参照有道词典：大词头，音标紧随其下，标签栏压在头部之下、内容之上，
 *    一次只呈现一块 —— 长词条就不会被撑成一条读不完的竖条。
 *
 * @param {object} entry  WordEntry
 * @param {object} opts   { showInflections, showExamples, showRelated, showMnemonic,
 *                          showPhonetic, compactHead, tabs }
 */
/* 部分导入词库把多个释义挤在一条里、用「<」分隔（实测 tame：
   「adj. 驯服的，温顺的 < 沉闷的，乏味的 vt. 驯服，制服」整坨塞进一条
   sense）。展开成独立义项，顺带把挤在中间的词性标记（vt. / n. …）
   拆出来——卡片渲染与 AI 补全都吃结构化的 senses。 */
function explodeLegacySenses(entry) {
  if (!entry || !entry.senses || !entry.senses.length) return entry;
  const hasSep = entry.senses.some((s) => /</.test(s.definition || ''));
  if (!hasSep) return entry;
  const POS = /^(n|v|vt|vi|adj|adv|prep|conj|pron|art|num|interj|int|aux)\.\s*/;
  const out = [];
  entry.senses.forEach((s) => {
    const parts = String(s.definition || '').split(/\s*<\s*/).map((x) => x.trim()).filter(Boolean);
    if (parts.length <= 1 && !POS.test(String(s.definition || ''))) { out.push(s); return; }
    const list = parts.length ? parts : [String(s.definition || '')];
    let lastPos = s.pos || '';
    list.forEach((p0) => {
      let p = p0;
      const m = p.match(POS);
      if (m) { lastPos = m[1] + '.'; p = p.slice(m[0].length).trim(); }
      if (p) out.push({ pos: lastPos, definition: p, examples: (s.examples || []).slice(0) });
    });
  });
  entry.senses = out;
  return entry;
}

/* ============================================================
   词条正文的固定专栏序列（有道式）—— 全项目唯一实现
   ============================================================

   用户反馈的两个问题都出在这里：
     1. 「这两个格式不同」—— 查词页用 renderEntry、详情卡另有
        renderDefs/renderInfl/renderEx/renderRel 四份手抄，排版必然漂移；
     2. 「每个词都一样」—— 旧版块是**条件生成**的（有变形才有变形标签、
        没例句就没有例句），于是 craftsman 有 [释义][变形] 两个标签、
        fault 可能只有 [释义]，栏目集合跟着数据走，每个词长得都不一样。

   现在的约定：
     - 顺序恒定：释义 → 例句 → 变形 → 记忆法 → 相关词；
     - **核心三栏（释义/例句/变形）永远在位**，无数据显示占位文案
       （占位里点名「AI 讲解」，把已有的一键生成能力接上）；
     - 记忆法 / 相关词是可选栏，有数据才追加（有道也不给没数据的词
       硬开一个空栏目）；
     - 例句从义项里抽出来**集中成专栏**（中英对照 + 逐句朗读），
       与详情卡的「例句」页共用同一段 HTML。
*/
function entryBlocks(entry, opts = {}) {
  const o = {
    showInflections: true,
    showExamples: true,
    showRelated: false,
    showMnemonic: true,
    // 无数据时是否保留占位（释义/例句/变形）。主查词页与详情卡恒为 true
    // —— 栏目集合必须每个词一致；「对应词汇」这类紧凑对照卡传 false，
    // 无数据整栏收起，不让虚线占位喧宾夺主（有数据照常显示）。
    placeholders: true,
    ...opts,
  };
  if (!entry) return [];
  const blocks = [];

  // ① 释义：按「释文写成的语言」分两组（中文组在前），组内有道式编号。
  //    例句不在这里内嵌 —— 统一进「例句」专栏，避免同一批例句出现两遍。
  const senses = entry.senses || [];
  if (senses.length) {
    const [local, native] = splitSensesByScript(senses);
    const inner = [];
    if (local.length && native.length) {
      inner.push(senseGroup(null, local, { showExamples: false }));
      // 原文组小标题跟着词条语言走（「乌鸦」的原文组本身就是中文）
      const title = langLabel(entry.lang || '') === '中文'
        ? '参考释义' : `${langLabel(entry.lang || '')}释义`;
      inner.push(senseGroup(title, native, { showExamples: false }));
    } else {
      inner.push(senseGroup(null, local.length ? local : native, { showExamples: false }));
    }
    blocks.push({ id: 'def', label: '释义', inner: inner.join('') });
  } else {
    blocks.push({
      id: 'def',
      label: '释义',
      inner: '<div class="blk-empty muted">暂无释义 —— 点下方「AI 讲解」可生成完整词条。</div>',
    });
  }

  // ② 例句：全量收集 + 中英对照 + 逐句朗读（有道「例句」栏的形态）
  if (o.showExamples !== false) {
    const has = collectExamples(entry).some((x) => x.text);
    if (has || o.placeholders !== false) {
      blocks.push({ id: 'ex', label: '例句', inner: examplesBlockHtml(entry) });
    }
  }

  // ③ 词形变化：**恒在** —— inflectionGridHtml 永远画得出「原形」锚点行，
  //    每个词的变形栏都不再为空（栏目集合一致的另一半保证）。
  if (o.showInflections !== false) {
    blocks.push({ id: 'infl', label: '变形', inner: inflectionGridHtml(entry) });
  }

  // ④ 记忆法（可选栏）
  if (o.showMnemonic !== false && entry.mnemonic) {
    blocks.push({ id: 'mnemonic', label: '记忆法', inner: `<div class="mnemonic-box">${esc(entry.mnemonic)}</div>` });
  }

  // ⑤ 相关词（可选栏）
  if (o.showRelated && entry.related && entry.related.length) {
    const rels = splitRelated(entry.related);
    if (rels.length) {
      blocks.push({
        id: 'related',
        label: '相关词',
        inner: '<div class="rel-list">' + rels
          .map(r => `<span class="rel-chip" data-word="${esc(r)}">${esc(r)}</span>`)
          .join('') + '</div>',
      });
    }
  }
  return blocks;
}

/** 例句专栏：全量（去重）中英对照，每句带朗读。无数据给占位。 */
function examplesBlockHtml(entry) {
  const exs = collectExamples(entry);
  const seen = new Set();
  const items = [];
  const lang = entry.lang || 'en';
  for (const ex of exs) {
    const k = String(ex.text || '').trim();
    if (!k || seen.has(k)) continue;
    seen.add(k);
    items.push(`<div class="ex-item">
      <div class="ex-line">
        <span class="ex-en">${esc(ex.text)}</span>
        <button class="speak-btn" title="朗读例句"
          data-speak-word="${esc(ex.text)}"
          data-speak-lang="${esc(lang)}"
          data-speak-audio=""
          data-speak-accent="us">${icon('sound')}</button>
      </div>
      ${ex.translation ? `<div class="ex-zh">${esc(ex.translation)}</div>` : ''}
    </div>`);
  }
  if (!items.length) {
    return '<div class="blk-empty muted">暂无例句 —— 点下方「AI 讲解」可生成例句与词形变化。</div>';
  }
  return `<div class="ex-list">${items.join('')}</div>`;
}

/**
 * 词典释义把**查询词**标注为另一个词的屈折/派生形式时，提取原形。
 *
 * 场景（用户反馈「如果是变形的话标记原型和现在的时态」）：查 `coincided`
 * 时词典只给一句 *simple past and past participle of coincide* —— 原形
 * 藏在释义里没人提，变形栏也没有任何参照。这里把原形挖出来：
 *   - 英文：past tense of X / past participle of X / plural of X / form of X…
 *   - 中文：coincide 的过去式 / …的复数形式（中文语序里原形在**前**）
 * 提不到返回 ''（不是变形，按普通词渲染）。
 */
function detectBaseForm(entry) {
  const defs = (entry.senses || []).map((s) => s.definition || '').join(' ');
  if (!defs) return '';
  // 英文：在小写副本上定位并截取（ASCII 小写不改变长度，index 对齐安全）
  const lower = defs.toLowerCase();
  const EN = [
    'past participle of ', 'past tense of ', 'simple past of ',
    'present participle of ', 'third person singular of ',
    'alternative spelling of ', 'misspelling of ',
    'form of ', 'variant of ', 'plural of ',
  ];
  for (const p of EN) {
    const i = lower.indexOf(p);
    if (i < 0) continue;
    const w = (lower.slice(i + p.length).match(/^[a-z][a-z'-]*/) || [''])[0];
    if (w.length >= 2) return w;
  }
  // 中文：原形在前，「X 的过去式 / 的复数形式 …」
  const zh = defs.match(/([A-Za-z][A-Za-z'-]*)['」)]?\s*的(过去式|过去分词|现在分词|复数形式|第三人称单数|复数)/);
  if (zh && zh[1].length >= 2) return zh[1];
  return '';
}

/** 查询词本身是屈折形式时，它是什么形态（coincided → 过去式/过去分词）。 */
function detectFormLabel(entry) {
  const defs = (entry.senses || []).map((s) => s.definition || '').join(' ');
  const lower = defs.toLowerCase();
  if (lower.includes('simple past and past participle')) return { label: '过去式/过去分词', pos: 'v.' };
  if (lower.includes('past tense')) return { label: '过去式', pos: 'v.' };
  if (lower.includes('past participle')) return { label: '过去分词', pos: 'v.' };
  if (lower.includes('present participle')) return { label: '现在分词', pos: 'v.' };
  if (lower.includes('third person')) return { label: '第三人称单数', pos: 'v.' };
  if (defs.includes('的复数形式') || lower.includes('plural of')) return { label: '复数', pos: 'n.' };
  if (defs.includes('的过去式')) return { label: '过去式', pos: 'v.' };
  if (defs.includes('的过去分词')) return { label: '过去分词', pos: 'v.' };
  if (defs.includes('的现在分词')) return { label: '现在分词', pos: 'v.' };
  if (lower.includes('plural')) return { label: '复数', pos: 'n.' };
  return { label: '变形', pos: '' };
}

/**
 * 变形类型归一 → `{ label: 中文标签, pos: 词性 }`。
 *
 * 数据源给的 label 中英混杂（freedictionary 是 `plural`/`past tense`，
 * AI 生成的是「过去式」），用户要求「动名形容时态单复数变形**要表明**」——
 * 两件事一起解决：label 统一成中文，每条再挂一个词性徽标（v. / n. / adj.），
 * 一眼看清这是谁的什么变形。注意匹配顺序：`past participle` 必须排在
 * `past` 前面，`过去分词` 必须排在 `过去式` 前面。
 */
function inflMeta(label) {
  const raw = String(label || '').trim();
  const l = raw.toLowerCase();
  const has = (...ks) => ks.some((k) => l.includes(k));
  if (has('past participle') || raw.includes('过去分词')) return { label: '过去分词', pos: 'v.' };
  if (has('past') || raw.includes('过去式')) return { label: '过去式', pos: 'v.' };
  if (has('present participle') || raw.includes('现在分词')) return { label: '现在分词', pos: 'v.' };
  if (has('3rd person', 'third person') || raw.includes('第三人称')) return { label: '第三人称单数', pos: 'v.' };
  if (has('gerund') || raw.includes('动名词')) return { label: '动名词', pos: 'v.' };
  if (has('imperative') || raw.includes('祈使')) return { label: '祈使形', pos: 'v.' };
  if (has('plural') || raw.includes('复数')) return { label: '复数', pos: 'n.' };
  if (has('singular') || raw.includes('单数')) return { label: '单数', pos: 'n.' };
  if (has('comparative') || raw.includes('比较级')) return { label: '比较级', pos: 'adj.' };
  if (has('superlative') || raw.includes('最高级')) return { label: '最高级', pos: 'adj.' };
  if (has('adverb', '副词')) return { label: raw, pos: 'adv.' };
  if (has('alternative', 'variant') || raw.includes('变体')) return { label: '变体', pos: '' };
  if (has('abbrev', 'short form') || raw.includes('缩写')) return { label: '缩写', pos: '' };
  // 中文 label（AI 生成）原样保留；认不出的一律原样显示、不挂词性
  return { label: raw || '形式', pos: '' };
}

/**
 * 词形变化网格（查词页与详情卡共用）。
 *
 * 结构（用户要求「标记原型和现在的时态」）：
 *   ① 恒定第一行 **原形（现在时）** —— 变形表的锚点，后面每条都相对它读；
 *   ② 查询词本身就是屈折形式时（coincided / grievaunces），
 *      第二行标出**它是什么形态**（过去式/过去分词、复数…）；
 *   ③ 数据源给的其余变形（中英 label 归一 + 词性徽标），
 *      与 ①② 重复的 form 跳过；
 *   ④ 数据变形一条都没有时补一句 AI 引导（原形行本身永远有内容，
 *      所以变形栏对**每个词**都不再是空的 —— 栏目集合因此完全一致）。
 *
 * 非英语词没有「现在时」概念，第一行 label 退回「原形」。
 */
function inflectionGridHtml(entry) {
  const inflections = (entry && entry.inflections) || [];
  const word = String((entry && entry.word) || '');
  const isEn = String((entry && entry.lang) || 'en').toLowerCase().startsWith('en');

  // ① 原形锚点：词典说「这是 X 的过去式」→ 原形是 X；否则原形就是词本身
  const base = detectBaseForm(entry) || word;
  const isInflected = !!base && base.toLowerCase() !== word.toLowerCase();
  const firstPos = ((entry && entry.senses) || [])
    .map((s) => String(s.pos || '').trim()).find(Boolean) || '';
  const items = [];
  if (base) {
    const badge = firstPos ? `<i class="infl-pos">${esc(firstPos)}</i>` : '';
    items.push(`<div class="infl-item infl-base">
      <span class="infl-label">${badge}${isEn ? '原形（现在时）' : '原形'}</span>
      <span class="infl-form clickable" data-word="${esc(base)}" title="点击查询 ${esc(base)}">${esc(base)}</span>
    </div>`);
  }

  // ② 查询词是变形形式 → 标明它的时态/形态
  if (isInflected) {
    const fl = detectFormLabel(entry);
    const badge = fl.pos ? `<i class="infl-pos">${fl.pos}</i>` : '';
    items.push(`<div class="infl-item">
      <span class="infl-label">${badge}${esc(fl.label)}</span>
      <span class="infl-form clickable" data-word="${esc(word)}" title="点击查询 ${esc(word)}">${esc(word)}</span>
    </div>`);
  }

  // ③ 数据里的其余变形（去重：与原形/本词相同的跳过）
  const seen = new Set([base.toLowerCase(), word.toLowerCase()]);
  let extra = 0;
  for (const i of inflections) {
    const form = String(i.form || '').trim();
    if (!form || seen.has(form.toLowerCase())) continue;
    seen.add(form.toLowerCase());
    const m = inflMeta(i.label);
    const badge = m.pos ? `<i class="infl-pos">${m.pos}</i>` : '';
    items.push(`<div class="infl-item">
      <span class="infl-label">${badge}${esc(m.label)}</span>
      <span class="infl-form clickable" data-word="${esc(form)}" title="点击查询 ${esc(form)}">${esc(form)}</span>
    </div>`);
    extra += 1;
  }

  if (!items.length) {
    return '<div class="blk-empty muted">暂无变形数据 —— 「AI 讲解」会一并生成。</div>';
  }
  const hint = extra
    ? ''
    : '<div class="infl-more muted">其余变形暂无数据 ——「AI 讲解」可生成。</div>';
  return `<div class="infl-grid">${items.join('')}</div>${hint}`;
}

function renderEntry(entry, opts = {}) {
  entry = explodeLegacySenses(entry);
  const o = {
    showInflections: true,
    showExamples: true,
    showRelated: false,
    showMnemonic: true,
    showPhonetic: true,
    // 头部只画音标与来源标签、不重复画大词头（双向词条的对应卡片用它，
    // 因为词头已经画在卡片标题上了，再来一遍就是加倍冗余）
    compactHead: false,
    // 分块 + 底部标签切换（见函数头说明）
    tabs: false,
    // 无数据核心栏保留占位（主查词页/详情卡要求栏目集合每词一致）
    placeholders: true,
    ...opts,
  };
  if (!entry) return '<div class="empty-state"><p>无内容</p></div>';

  /* ---------- 头部：大词头 + 音标成对 + 来源标签 ---------- */
  const head = [];
  head.push('<div class="we-head">');
  if (!o.compactHead) {
    head.push(`<div class="we-word">${esc(entry.word)}</div>`);
  }

  if (o.showPhonetic) {
    // 统一走 phoneticHtml：语言标注、斜杠规则、IPA 字体都只有一份实现。
    // 词条一个音标都没有时，至少留一个纯发音按钮，别整块消失。
    const html = phoneticHtml(entry, { speak: true });
    head.push(`<div class="we-phon">${html || speakBtn(entry, 'us', '发音')}</div>`);
  }

  const srcs = (entry.source || '').split('+').filter(Boolean);
  const tags = srcs.map(s => `<span class="tag">${esc(sourceLabel(s))}</span>`);
  if (entry.lang && entry.lang !== 'en') {
    tags.unshift(`<span class="tag blue">${esc(langLabel(entry.lang))}</span>`);
  }
  // ★ 查询词本身是屈折形式（coincided → coincide）时，词头旁直接标出原形，
  //   点击即查原形 ——「如果是变形的话标记原型」在词头这一层的兑现。
  const baseTag = detectBaseForm(entry);
  if (baseTag && baseTag.toLowerCase() !== String(entry.word || '').toLowerCase()) {
    tags.unshift(`<span class="tag orange clickable" data-word="${esc(baseTag)}"
      title="这是变形形式，点击查看原形">原形 ${esc(baseTag)}</span>`);
  }
  if (tags.length) head.push(`<div class="we-src">${tags.join('')}</div>`);
  head.push('</div>');

  /* ---------- 正文：固定专栏序列（有道式），排布由 o.tabs 决定 ----------
     专栏内容的唯一实现在 `entryBlocks` —— 查词页与详情卡调同一份，
     「同一个词在两处长得不一样」「不同词栏目集合乱跳」都是从这里治掉的。 */
  const blocks = entryBlocks(entry, {
    showInflections: o.showInflections,
    showExamples: o.showExamples,
    showRelated: o.showRelated,
    showMnemonic: o.showMnemonic,
    placeholders: o.placeholders,
  });

  /* ---------- 组装 ---------- */
  const body = o.tabs
    ? tabbedBlocks(blocks)
    : blocks.map(b => `<div class="we-section"><div class="we-section-title">${esc(b.label)}</div>${b.inner}</div>`).join('');

  return `<div class="word-entry${o.tabs ? ' tabbed' : ''}" data-entry-word="${esc(entry.word)}">`
    + head.join('') + body + '</div>';
}

/**
 * 分块卡片 + 底部标签切换。
 *
 * 标签栏夹在头部与内容之间（头部之下、正文之上）—— 用户要的是「先把词头
 * 看全，再按需切到想看的那一块」。面板用 `display:none` 切换而不是重渲染，
 * 这样切标签不会丢掉已经绑好的发音按钮委托，也不会闪。
 *
 * 第一块默认选中：查词的人十有八九是来看释义的，多一次点击就是白费。
 */
function tabbedBlocks(blocks) {
  if (!blocks.length) return '';
  const tabs = blocks.map((b, i) => `<button class="entry-tab${i === 0 ? ' active' : ''}"`
    + ` data-block="${esc(b.id)}" type="button" role="tab">${esc(b.label)}</button>`).join('');
  const panes = blocks.map((b, i) => `<div class="entry-pane${i === 0 ? ' active' : ''}"`
    + ` data-pane="${esc(b.id)}">${b.inner}</div>`).join('');
  return `<div class="entry-tabs" role="tablist">${tabs}</div>`
    + `<div class="entry-body">${panes}</div>`;
}

/** 一段释义里有没有汉字 —— 用来区分「母语释义」与「原文释义」。 */
function hasHan(s) {
  return /[\u3400-\u4dbf\u4e00-\u9fff\uf900-\ufaff]/.test(s || '');
}

/**
 * 把义项按「释文写成的语言」分成两组。
 *
 * @returns {[Array, Array]} `[母语写成的一组, 原文写成的一组]`，任一组可能为空
 */
function splitSensesByScript(senses) {
  const local = [];
  const native = [];
  for (const s of (senses || [])) {
    (hasHan(s.definition) ? local : native).push(s);
  }
  return [local, native];
}

/**
 * 画一组义项。
 *
 * 返回的是**块内容**，外层的卡片/区块由 `renderEntry` 决定 —— 堆叠排布与
 * 标签排布共用这一份实现，才不会出现「同一个词在两处长得不一样」。
 *
 * `title` 传空时不出小标题：只有一个来源的释义时，「释义」这个标题由块自己
 * 带着（标签模式下就是标签本身），再来一遍是重复。
 */
function senseGroup(title, senses, opts = {}) {
  /* 有道式排版：**词性只在组头标一次**，组内义项编号 1. 2. 3. …
     旧版每条义项都带一个 .sense-pos，数据里 pos 只写在第一条时就成了
     「n. 故障 / 断层 / 错误… v. 弄错」这种词性悬在半空、编号全无的样子
     （用户截图 fault 就是这样）。这里按 pos 变化切组：pos 相同的连续义项
     共用一个词性标题，编号在每个词性组内从 1 重排 —— 与有道一致。 */
  const parts = ['<div class="sense-group">'];
  if (title) parts.push(`<div class="we-section-title">${esc(title)}</div>`);
  let lastPos = null;
  let n = 0;
  for (const s of senses) {
    const pos = String(s.pos || '').trim();
    if (pos !== lastPos) {
      if (pos) parts.push(`<div class="sg-pos">${esc(pos)}</div>`);
      lastPos = pos;
      n = 0;
    }
    n += 1;
    parts.push('<div class="sense">');
    parts.push(`<div class="sense-no">${n}.</div>`);
    parts.push('<div class="sense-def">');
    parts.push(esc(s.definition));
    if (opts.showExamples !== false && s.examples && s.examples.length) {
      for (const ex of s.examples.slice(0, 3)) {
        parts.push('<div class="sense-ex">');
        parts.push(esc(ex.text));
        if (ex.translation) parts.push(`<div class="ex-zh">${esc(ex.translation)}</div>`);
        parts.push('</div>');
      }
    }
    parts.push('</div></div>');
  }
  parts.push('</div>');
  return parts.join('');
}

/**
 * 渲染「双向词条」的另一半：目标语言侧的对应词及其完整词条。
 *
 * 用户需求原文：「这里我是中文转英文，应该下面详细介绍的是 crow 或者
 * 其他能表示乌鸦的单词」「仿照有道词典两者都有，还有其他语言也要类似」。
 *
 * 所以「乌鸦」的主卡片下面是 crow / rook / raven 这几个候选译法各自的
 * 完整英文词条（英文音标、英文释义、词形变化），而不只是一串词。
 *
 * @param {Array} pairs [{ word, lang, entry, via }]
 */
function renderPairs(pairs, opts = {}) {
  const list = (pairs || []).filter(p => p && p.entry);
  if (!list.length) return '';

  const parts = ['<div class="pair-block">'];
  parts.push('<div class="pair-block-title">对应词汇</div>');
  parts.push(`<div class="pair-hint muted">下面是在${esc(langLabel(list[0].lang))}里表示这个词的说法，点击词头可单独查询</div>`);
  for (const p of list) {
    parts.push('<div class="pair-card">');
    parts.push('<div class="pair-card-head">');
    parts.push(`<span class="pair-word clickable" data-word="${esc(p.word)}">${esc(p.word)}</span>`);
    // ★ 词头旁**不再**放小喇叭：紧跟着的 renderEntry 音标行里已经有
    //   「英 / 美」两个发音按钮，这里再来一个是重复入口，视觉上也抢焦点
    //   （用户原话：「删除文字旁的小喇叭图标，因为下方已有英美发音」）。
    parts.push(`<span class="tag blue">${esc(langLabel(p.lang))}</span>`);
    parts.push(p.via === 'translation'
      ? '<span class="tag ok">主译</span>'
      : '<span class="tag">其他译法</span>');
    parts.push('</div>');
    parts.push(renderEntry(p.entry, {
      compactHead: true,
      showInflections: opts.showInflections !== false,
      showExamples: opts.showExamples !== false,
      showRelated: false,
      showMnemonic: false,
      showPhonetic: opts.showPhonetic !== false,
      // 对照卡是紧凑形态：没数据的栏整栏收起，不摆占位框
      placeholders: false,
    }));
    parts.push('</div>');
  }
  parts.push('</div>');
  return parts.join('');
}

/** 收集词条中所有例句，供「例句」标签页使用。 */
function collectExamples(entry) {
  const out = [];
  for (const s of (entry.senses || [])) {
    for (const ex of (s.examples || [])) {
      if (ex.text) out.push({ pos: s.pos, ...ex });
    }
  }
  return out;
}

/** 来源 id → 中文名 */
function sourceLabel(id) {
  const map = {
    'free-dictionary': 'Free Dictionary',
    'wiktionary': 'Wiktionary',
    'youdao-suggest': '有道',
    // 下面两个和有道的 suggest 接口同域名，但来源 id 不同。
    // 不补进映射表的话界面上会直接露出 `youdao-newhh` 这种 id ——
    // 用户看到的是「中文 youdao-newhh」，完全读不懂。
    'youdao-jsonapi': '有道释义',
    'youdao-newhh': '现代汉语规范词典',
    'libre-translate': 'LibreTranslate',
    'lmstudio': '本地大模型',
    'builtin-seed': '内置词库',
  };
  return map[id] || id;
}

/** 语言代码 → 中文名 */
function langLabel(code) {
  const map = {
    en: '英语', ja: '日语', ko: '韩语', fr: '法语', de: '德语',
    es: '西班牙语', ru: '俄语', it: '意大利语', pt: '葡萄牙语',
    zh: '中文', ar: '阿拉伯语', hi: '印地语',
  };
  return map[code] || code;
}

/**
 * 规范 Markdown 渲染（AI 讲解专用，仿有道词典讲解版式）。
 *
 * 相比旧版新增：
 *   - 表格（| a | b | 含表头分隔行）
 *   - 多级嵌套列表（按缩进判定层级）
 *   - 引用块 >、分割线 ---
 *   - 代码块 ```（保留原样，不转义内部）
 *   - 软换行：段内单行换行渲染为 <br>
 *   - 行内：**粗体**、*斜体*、`代码`、[文字](链接)、~~删除线~~
 *   - 中文标点规范化：把模型误输出的 Markdown 符号吃掉，避免「符号乱飘」
 */
function renderMarkdown(md) {
  if (!md) return '';
  let src = String(md).replace(/\r\n?/g, '\n');

  // 去掉模型偶发包裹的整段 ```markdown / ``` 外壳
  src = src.replace(/^\s*```[a-zA-Z]*\s*\n/, '').replace(/\n```\s*$/, '');

  const lines = src.split('\n');
  const out = [];

  // ---------- 行内解析 ----------
  const inline = (raw) => {
    let s = esc(raw);
    // 代码（先占位，避免内部被后续规则改写）
    const codes = [];
    s = s.replace(/`([^`]+)`/g, (m, c) => {
      codes.push(c);
      return '\u0000C' + (codes.length - 1) + '\u0000';
    });
    // 图片 ![alt](url) → 链接样式
    s = s.replace(/!\[([^\]]*)\]\(([^)\s]+)\)/g,
      '<a href="$2" target="_blank" rel="noreferrer">$1</a>');
    // 链接 [文字](url)
    s = s.replace(/\[([^\]]+)\]\(([^)\s]+)\)/g,
      '<a href="$2" target="_blank" rel="noreferrer">$1</a>');
    // 粗体 + 斜体
    s = s.replace(/\*\*\*(.+?)\*\*\*/g, '<strong><em>$1</em></strong>');
    s = s.replace(/\*\*(.+?)\*\*/g, '<strong>$1</strong>');
    s = s.replace(/(^|[^*])\*([^*\n]+?)\*(?!\*)/g, '$1<em>$2</em>');
    s = s.replace(/__(.+?)__/g, '<strong>$1</strong>');
    // 删除线
    s = s.replace(/~~(.+?)~~/g, '<del>$1</del>');
    // 还代码
    s = s.replace(/\u0000C(\d+)\u0000/g, (m, i) => '<code>' + codes[+i] + '</code>');
    return s;
  };

  // ---------- 表格检测 ----------
  const isTableRow = (l) => /^\s*\|.*\|\s*$/.test(l);
  const isTableSep = (l) => /^\s*\|?[\s:|-]+\|[\s:|-]*$/.test(l) && l.includes('-');

  const parseTableRow = (l) =>
    l.trim().replace(/^\|/, '').replace(/\|$/, '').split('|').map(c => c.trim());

  // ---------- 列表栈（支持嵌套） ----------
  // 栈元素：{ indent, tag }
  const listStack = [];
  const closeLists = (toIndent = -1) => {
    while (listStack.length && listStack[listStack.length - 1].indent > toIndent) {
      out.push('</li></' + listStack.pop().tag + '>');
    }
  };
  const closeAllLists = () => { closeLists(-1); };

  for (let i = 0; i < lines.length; i++) {
    const raw = lines[i];
    const line = raw.replace(/\s+$/, '');

    // 空行
    if (!line.trim()) { closeAllLists(); continue; }

    // 代码块 ```
    const fence = line.match(/^\s*```(.*)$/);
    if (fence) {
      closeAllLists();
      const buf = [];
      i++;
      while (i < lines.length && !/^\s*```/.test(lines[i])) { buf.push(lines[i]); i++; }
      out.push('<pre class="md-pre"><code>' + esc(buf.join('\n')) + '</code></pre>');
      continue;
    }

    // 分割线
    if (/^\s*([-*_])\s*\1\s*\1[\s\1]*$/.test(line)) {
      closeAllLists();
      out.push('<hr class="md-hr">');
      continue;
    }

    // 标题
    const h = line.match(/^\s*(#{1,6})\s+(.*)$/);
    if (h) {
      closeAllLists();
      const lvl = Math.min(h[1].length + 1, 5);
      out.push(`<h${lvl} class="md-h">${inline(h[2].replace(/#+\s*$/, ''))}</h${lvl}>`);
      continue;
    }

    // 引用块
    if (/^\s*>\s?/.test(line)) {
      closeAllLists();
      const buf = [];
      while (i < lines.length && /^\s*>\s?/.test(lines[i])) {
        buf.push(lines[i].replace(/^\s*>\s?/, ''));
        i++;
      }
      i--;
      out.push('<blockquote class="md-quote">' + inline(buf.join(' ')) + '</blockquote>');
      continue;
    }

    // 表格
    if (isTableRow(line) && i + 1 < lines.length && isTableSep(lines[i + 1])) {
      closeAllLists();
      const head = parseTableRow(line);
      i += 2; // 跳过表头与分隔行
      const rows = [];
      while (i < lines.length && isTableRow(lines[i])) {
        rows.push(parseTableRow(lines[i]));
        i++;
      }
      i--;
      let t = '<div class="md-table-wrap"><table class="md-table"><thead><tr>';
      t += head.map(c => '<th>' + inline(c) + '</th>').join('');
      t += '</tr></thead><tbody>';
      for (const r of rows) {
        t += '<tr>';
        for (let c = 0; c < head.length; c++) {
          t += '<td>' + inline(r[c] === undefined ? '' : r[c]) + '</td>';
        }
        t += '</tr>';
      }
      t += '</tbody></table></div>';
      out.push(t);
      continue;
    }

    // 列表（无序 / 有序，按缩进分层）
    const ul = raw.match(/^(\s*)[-*•]\s+(.*)$/);
    const ol = raw.match(/^(\s*)(\d+)[.)]\s+(.*)$/);
    if (ul || ol) {
      const indent = Math.floor((ul ? ul[1] : ol[1]).length / 2);
      const tag = ol ? 'ol' : 'ul';
      const text = ul ? ul[2] : ol[3];

      // 回退到同级或更浅层级
      closeLists(indent);
      const top = listStack[listStack.length - 1];
      if (!top || top.indent < indent) {
        // 开新层（含类型切换）
        if (top && top.indent === indent && top.tag !== tag) {
          out.push('</li></' + listStack.pop().tag + '>');
        }
        out.push('<' + tag + ' class="md-list">');
        listStack.push({ indent, tag });
      } else if (top.indent === indent && top.tag !== tag) {
        out.push('</li></' + listStack.pop().tag + '>');
        out.push('<' + tag + ' class="md-list">');
        listStack.push({ indent, tag });
      } else {
        out.push('</li>'); // 同级下一条
      }
      out.push('<li>' + inline(text));
      continue;
    }

    // 普通段落：连续行软换行 → <br>
    closeAllLists();
    const buf = [line];
    while (
      i + 1 < lines.length &&
      lines[i + 1].trim() &&
      !/^\s*(#{1,6}\s|[-*•]\s|\d+[.)]\s|>|\||```)/.test(lines[i + 1]) &&
      !isTableSep(lines[i + 1])
    ) {
      buf.push(lines[i + 1].replace(/\s+$/, ''));
      i++;
    }
    out.push('<p class="md-p">' + buf.map(inline).join('<br>') + '</p>');
  }

  closeAllLists();
  return out.join('');
}

/** 格式化日期为「今天/明天/月-日」。 */
function fmtDay(dateStr) {
  const today = new Date();
  const d = new Date(dateStr + 'T00:00:00');
  const diff = Math.round((d - new Date(today.toDateString())) / 86400000);
  if (diff === 0) return '今天';
  if (diff === 1) return '明天';
  if (diff === 2) return '后天';
  if (diff === -1) return '昨天';
  return `${d.getMonth() + 1}月${d.getDate()}日`;
}

/** 相对时间：刚刚 / 5 分钟前 / 3 天前。 */
function timeAgo(ts) {
  if (!ts) return '—';
  const diff = Math.floor(Date.now() / 1000) - ts;
  if (diff < 60) return '刚刚';
  if (diff < 3600) return `${Math.floor(diff / 60)} 分钟前`;
  if (diff < 86400) return `${Math.floor(diff / 3600)} 小时前`;
  return `${Math.floor(diff / 86400)} 天前`;
}

/** 熟练度颜色分级。 */
function masteryClass(v) {
  if (v >= 75) return 'high';
  if (v < 40) return 'low';
  return '';
}

/** 渲染柱状图（近 N 天答题量）。 */
function renderBarChart(container, history) {
  if (!container) return;
  if (!history || !history.length) {
    container.innerHTML = '<div class="muted" style="padding:40px;text-align:center">暂无数据</div>';
    return;
  }
  const max = Math.max(1, ...history.map(h => h.count));
  container.innerHTML = history.map(h => {
    const total = h.count || 0;
    const ok = Math.min(h.correct || 0, total);
    const bad = Math.max(0, total - ok);
    const hOk = total ? Math.round((ok / max) * 108) : 0;
    const hBad = total ? Math.round((bad / max) * 108) : 0;
    const label = h.date ? h.date.slice(5) : '';
    return `<div class="bar-col" title="${esc(h.date)}：${total} 题（对 ${ok} / 错 ${bad}）">
      <div class="bar-stack">
        ${hBad ? `<div class="bar-bad" style="height:${hBad}px"></div>` : ''}
        ${hOk ? `<div class="bar-ok" style="height:${hOk}px"></div>` : ''}
      </div>
      <div class="bar-lbl">${esc(label)}</div>
    </div>`;
  }).join('');
}

/** 侧边栏/主界面的词条详情标签页切换。 */
function switchDetailTab(name) {
  document.querySelectorAll('#dc-tabs .dc-tab').forEach(t => {
    t.classList.toggle('active', t.dataset.tab === name);
  });
  document.querySelectorAll('.dc-pane').forEach(p => {
    p.classList.toggle('active', p.dataset.pane === name);
  });
}

/**
 * 事件目标是否处于「正在打字」的控件里。
 *
 * 用途：全局快捷键（document 级的 keydown）必须先问一句这个，否则
 * 用户在搜索框里输入时按键会被快捷键吞掉——表现为「输入框打不了字、
 * 退格也没用」。这是之前背诵页 A~H / 1~8 快捷键把搜索框吃掉的原因。
 *
 * ★ 已禁用 / 只读的输入框**不算**打字目标。
 *   这一条是「答题后按回车进不了下一题」的根因之一：拼写模式答完会把
 *   `#spell-input` 置为 `disabled`，但焦点还在它身上；禁用控件根本收不到
 *   按键，「正在输入」这个判断于是永远为真 —— 回车被无条件吞掉。
 *   判定标准应该是「这个控件还能不能接收文字」，而不是「它是不是 INPUT」。
 */
function isTypingTarget(t) {
  if (!t || t.nodeType !== 1) return false;
  const tag = (t.tagName || '').toUpperCase();
  if (tag === 'TEXTAREA' || tag === 'SELECT') return !t.disabled;
  if (tag === 'INPUT') {
    // 禁用 / 只读的输入框不参与文本输入，键盘应当交还给全局快捷键
    if (t.disabled || t.readOnly) return false;
    const type = (t.getAttribute('type') || 'text').toLowerCase();
    // 按钮类 input 不参与文本输入
    return !['button', 'submit', 'reset', 'checkbox', 'radio', 'file', 'image', 'range', 'color'].includes(type);
  }
  if (t.isContentEditable || t.getAttribute('contenteditable') === 'true') {
    return t.getAttribute('contenteditable') !== 'false' && !t.hasAttribute('data-noneditable');
  }
  return false;
}

/** 防抖。 */
function debounce(fn, wait = 260) {
  let t = null;
  return (...args) => {
    clearTimeout(t);
    t = setTimeout(() => fn(...args), wait);
  };
}

/**
 * 给一个「已渲染的列表」接上关键词搜索。
 *
 * 采用纯前端过滤：只切换行的 display，不重新请求后端、不重建 DOM，
 * 因此行上已经绑定好的点击事件（开详情卡、移出错词本…）全部保留；
 * 列表被重新渲染后调用返回的 apply() 即可重新套用当前关键词。
 *
 * @param {object} o
 * @param {string|HTMLElement} o.input          搜索输入框
 * @param {string|HTMLElement} o.list           列表容器
 * @param {string} [o.row='.word-row']          参与过滤的行选择器
 * @param {string|HTMLElement} [o.counter]      显示「匹配 N 条」的元素
 * @param {object} [o.empty]                    空状态文案 { icon, title, hint }
 * @param {number} [o.wait=260]                 实时搜索防抖毫秒数
 * @param {Function} [o.onApply](matched, kw)   每次过滤完成后的回调
 * @returns {{apply:Function,set:Function,clear:Function,keyword:string}}
 */
function attachListSearch(o) {
  const input = typeof o.input === 'string' ? document.getElementById(o.input) : o.input;
  const list = typeof o.list === 'string' ? document.getElementById(o.list) : o.list;
  const noop = { apply() {}, set() {}, clear() {}, keyword: '' };
  if (!input || !list) return noop;

  const rowSel = o.row || '.word-row';
  const wait = o.wait || 260;
  const counter = typeof o.counter === 'string' ? document.getElementById(o.counter) : (o.counter || null);
  const cfg = Object.assign({
    icon: '&#128269;',
    title: '没有匹配的结果',
    hint: '换个关键词试试，或清空搜索框查看全部',
  }, o.empty || {});

  let kw = '';
  let timer = null;

  /** 一行的可搜索文本：优先用 data-word / data-name，再兜底整行文本（含释义、标签）。 */
  function haystack(row) {
    const d = row.dataset || {};
    return ((d.word || '') + ' ' + (d.name || '') + ' ' + (row.textContent || '')).toLowerCase();
  }

  function emptyEl() {
    let el = list.querySelector(':scope > .search-empty');
    if (!el) {
      el = document.createElement('div');
      el.className = 'empty-state search-empty hidden';
      el.innerHTML =
        `<div class="es-icon">${cfg.icon}</div>` +
        `<p>${esc(cfg.title)}</p>` +
        `<p class="muted">${esc(cfg.hint)}</p>`;
      list.appendChild(el);
    }
    return el;
  }

  function apply() {
    const rows = Array.prototype.slice.call(list.querySelectorAll(rowSel));
    // 列表本来就是空的（页面自己已渲染空状态），不要叠加「没有匹配的结果」
    if (!rows.length) return;

    if (!kw) {
      rows.forEach(r => { r.style.display = ''; });
      emptyEl().classList.add('hidden');
      if (counter) counter.textContent = '';
      if (o.onApply) o.onApply(rows.length, '');
      return;
    }

    let n = 0;
    rows.forEach(r => {
      const hit = haystack(r).indexOf(kw) >= 0;
      r.style.display = hit ? '' : 'none';
      if (hit) n++;
    });
    emptyEl().classList.toggle('hidden', n > 0);
    if (counter) counter.textContent = n ? `匹配 ${n} 条` : '';
    if (o.onApply) o.onApply(n, kw);
  }

  function run() {
    kw = (input.value || '').trim().toLowerCase();
    apply();
  }

  input.addEventListener('input', () => {
    clearTimeout(timer);
    timer = setTimeout(run, wait);
  });
  input.addEventListener('keydown', (e) => {
    if (e.key === 'Enter') {          // 回车立即生效，不等防抖
      e.preventDefault();
      clearTimeout(timer);
      run();
    } else if (e.key === 'Escape') {  // Esc 清空
      clearTimeout(timer);
      input.value = '';
      run();
    }
  });

  return {
    apply,
    set(v) { input.value = v || ''; clearTimeout(timer); run(); },
    clear() { this.set(''); },
    get keyword() { return kw; },
  };
}

/**
 * 把**纯文本**渲染成 HTML：转义 + 保留段落与换行。
 *
 * 译文不是 Markdown（模型/接口返回的就是平文），直接用 renderMarkdown 会把
 * 译文里的 `#`、`*`、`-` 当成语法，反而破坏内容。段落翻译还需要保留原文的
 * 分行结构，所以按空行切段、段内换行转 <br>。
 */
function renderPlainText(s) {
  const t = String(s == null ? '' : s).replace(/\r\n?/g, '\n').trim();
  if (!t) return '';
  return t
    .split(/\n{2,}/)
    .map(p => `<p>${esc(p).replace(/\n/g, '<br>')}</p>`)
    .join('');
}

/* ---------------- 「当前语音」提示条 ----------------

   需求原文：「朗读时在界面旁显示当前用的是哪种语音（如 AI 或本地语音）」。

   做成**一个**全局浮层 + 动态定位，不是为了省代码，而是因为发音按钮散布在
   十几个容器里（题面、音标行、反馈条、上一个词、已背列表、详情卡、查词结果、
   侧边栏、翻译页…）。给每处都插一个提示元素意味着每处都要维护显隐状态，
   必然出现「某处忘了隐藏，提示条一直挂着」。
   一处浮层则天然只有一份状态：谁触发朗读就贴到谁旁边，朗读结束/被抢占即消失。

   `position: fixed` 是为了不受任何容器的 `overflow: hidden` 裁剪 ——
   可滚动的结果面板里，absolute 定位的提示条会被裁掉一半。 */
let voiceTipTimer = null;

/** 把提示条摆到锚点元素的旁边（优先右下，贴边时自动翻转，永不越界）。 */
function positionVoiceTip(el, anchor) {
  const gap = 6;
  const r = (anchor && anchor.getBoundingClientRect) ? anchor.getBoundingClientRect() : null;
  // 拿不到锚点（程序化朗读）→ 退到右下角，至少不挡住内容
  if (!r || (!r.width && !r.height)) {
    el.style.left = 'auto';
    el.style.top = 'auto';
    el.style.right = '16px';
    el.style.bottom = '16px';
    return;
  }
  el.style.right = 'auto';
  el.style.bottom = 'auto';
  // 宽度要等浏览器量过一次才知道，所以先按当前尺寸算，再夹到视口内
  const w = el.offsetWidth || 160;
  const h = el.offsetHeight || 26;
  let left = r.right + gap;
  let top = r.top + (r.height - h) / 2;
  if (left + w > window.innerWidth - 8) left = r.left - w - gap;   // 贴右边 → 翻到左侧
  if (left < 8) left = Math.max(8, Math.min(r.left, window.innerWidth - w - 8));
  top = Math.max(8, Math.min(top, window.innerHeight - h - 8));
  el.style.left = `${Math.round(left)}px`;
  el.style.top = `${Math.round(top)}px`;
}

/**
 * 显示「当前语音」。由 `Speak.notifyVoice` 在每次真正发声时调用。
 * @param {{engine:string, voice:string}} info
 * @param {string} label 已拼好的整句（未用，保留给调试）
 * @param {Element} [anchor]
 */
function voiceTip(info, label, anchor) {
  const el = document.getElementById('voice-tip');
  if (!el || !info) return;
  // 分成两段渲染：**类型词**单独一个文本节点，才能被 i18n 词典命中
  // （「本地 AI 语音 · amy」这种拼接串是翻译不了的）。
  const kind = (window.Speak && window.Speak.voiceKindLabel)
    ? window.Speak.voiceKindLabel(info.engine) : '';
  const name = String(info.voice || '').trim();
  el.innerHTML = `<span class="vt-ico">${icon('sound')}</span>`
    + `<span class="vt-text">${esc(kind)}${name ? `<i>${esc(name)}</i>` : ''}</span>`;
  el.classList.remove('hidden');
  el.setAttribute('data-engine', String(info.engine || ''));
  positionVoiceTip(el, anchor);
  clearTimeout(voiceTipTimer);
  // 自动淡出：提示条是「状态说明」不是「通知」，不该要求用户手动关。
  // 真人录音/合成语音的时长通常在这个量级，宁可早消失也不要赖着不走。
  voiceTipTimer = setTimeout(voiceTipHide, 3000);
}

/** 收起提示条。 */
function voiceTipHide() {
  clearTimeout(voiceTipTimer);
  document.getElementById('voice-tip')?.classList.add('hidden');
}

/* ---------------- 可点词的统一入口 ---------------- */

/** 全站「点一下就查这个词」的元素选择器。加新形态时改这一处。 */
const WORD_CHIP_SEL =
  '.rel-chip[data-word], .infl-form.clickable[data-word], .pair-word.clickable[data-word]'
  + ', .tag.clickable[data-word]';

/**
 * 给一个容器的「可点词」（相关词 / 变形 / 对应词词头）接上统一入口。
 *
 * ★ 这里替代的是**原来那个全局捕获兜底**（`document.addEventListener('click',
 *   …, true)` 里做 `stopPropagation()`）。那个写法有两个致命后果：
 *
 *   1. **它排在捕获阶段**，而捕获是从 window 往下走 —— 它先于任何容器的
 *      冒泡委托执行。一 `stopPropagation()`，侧边栏（`#sb-body`）与详情卡
 *      （`#dc-pane-rel` / `#dc-pane-infl`）里那些**已经写好**的委托就永远
 *      收不到事件，全成了死代码。
 *   2. 它还硬编码只调 `Lookup.query()` —— 于是侧边栏里点相关词，结果被画进
 *      **主窗口**的 `#lk-result`（侧边栏窗口里那块是隐藏的），用户看到的就是
 *      「点了没反应」。
 *
 * 现在的分工：
 *   - 每个渲染可点词的容器自己调一次本函数，指定「查到之后画到哪里」；
 *   - 事件走**冒泡**，内层容器先处理并打标记（`e.__wwChip`），外层与全局兜底
 *     看到标记就跳过 —— 同一层里不会重复触发，也不需要 `stopPropagation`
 *     （那会把同一元素上的发音委托等无关监听一起掐掉）。
 *   - 全局只留一个**兜底**：万一将来新加了一处渲染忘了绑定，点击仍然有效，
 *     只是会画到主查词页 —— 比「点了没反应」好得多。
 *
 * @param {string|HTMLElement} root    容器
 * @param {Function} [onPick]          (word, chipEl) => void，缺省走主查词页
 */
function bindWordChips(root, onPick) {
  const el = typeof root === 'string' ? document.getElementById(root) : root;
  if (!el || el.__wwChipsBound) return;
  el.__wwChipsBound = true;
  el.addEventListener('click', (e) => {
    if (e.__wwChip) return;                       // 更内层的容器已处理
    const t = e.target;
    if (!t || typeof t.closest !== 'function') return;
    const chip = t.closest(WORD_CHIP_SEL);
    if (!chip || !el.contains(chip)) return;
    const w = chip.dataset.word;
    if (!w) return;
    e.__wwChip = true;
    const go = onPick || ((word) => (window.Lookup && window.Lookup.query
      ? window.Lookup.query(word) : undefined));
    go(w, chip);
  });
}

window.WW = window.WW || {};
Object.assign(window.WW, {
  esc, toast, loadingHtml, renderEntry, entryBlocks, examplesBlockHtml, inflMeta,
  detectBaseForm, detectFormLabel, collectExamples, sourceLabel, langLabel,
  splitRelated, phoneticHtml, renderPairs, senseGroup, splitSensesByScript, hasHan,
  renderMarkdown, renderPlainText, fmtDay, timeAgo, masteryClass, renderBarChart, switchDetailTab, debounce,
  attachListSearch, speakBtn, isTypingTarget, icon, ICON_PATHS, bindWordChips, WORD_CHIP_SEL,
  voiceTip, voiceTipHide,
  speak: (word, opts) => (window.Speak ? window.Speak.speak(word, opts) : null),
  speakBind: (root) => { if (window.Speak) window.Speak.bindDelegate(root); },
});

/* 全局兜底（冒泡阶段，不做 stopPropagation）：
   万一某条渲染路径忘了调 bindWordChips，点击仍然查得动。
   已处理过的事件带 e.__wwChip 标记，不会重复触发。 */
document.addEventListener('click', (e) => {
  if (e.__wwChip) return;
  const t = e.target;
  if (!t || typeof t.closest !== 'function') return;
  const chip = t.closest(WORD_CHIP_SEL);
  if (!chip) return;
  e.__wwChip = true;
  const w = chip.dataset.word;
  if (w && window.Lookup && window.Lookup.query) window.Lookup.query(w);
});
