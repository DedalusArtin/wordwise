/* ============================================================
   speak.js —— 单词 / 句子发音
   - 优先播放词典返回的音频 URL（英/美音分开发音）
   - 无音频时用 WebView 内置语音合成（离线、零延迟）
   - 音色挑选：优先「神经网络 / 在线」音色，其次同语言本地音色
   - 全局单例，避免多个音频叠加

   ★ 导出契约（调用方依赖，改名会直接让按钮变哑巴）：
     speak(text, opts) / speakText(text, opts) / stop()
     playUrl(url, key) / playTts(text, lang, rate, key)
     resolve(entry) / btnHtml(entry, accent, label) / bindDelegate(root)
     bcp47(lang, accent) / voices() / availableLang(lang)
   历史上 `playUrl` / `playTts` 只写在闭包里没导出，导致
   「翻译页朗读」「词级译文朗读」两个按钮点了毫无反应。
   ============================================================ */

const Speak = (() => {
  let audioEl = null;      // 复用同一个 <audio>，避免并发
  let currentKey = '';     // 当前正在播放的 key，用于重复点击切换
  let speaking = false;
  let voicesCache = [];
  let lastKey = '';        // 最近一次发声的 key，供音频失败时降级复用

  /** 语言代码 → BCP-47 语音标签 */
  function bcp47(lang, accent) {
    const l = String(lang || 'en').toLowerCase().replace('_', '-');
    if (l.split('-')[0] === 'en') return accent === 'uk' ? 'en-GB' : 'en-US';
    if (l.includes('-')) return l;
    const map = {
      zh: 'zh-CN', ja: 'ja-JP', ko: 'ko-KR', fr: 'fr-FR',
      de: 'de-DE', es: 'es-ES', ru: 'ru-RU', it: 'it-IT',
      pt: 'pt-PT', ar: 'ar-SA', hi: 'hi-IN', th: 'th-TH',
      el: 'el-GR', vi: 'vi-VN', tr: 'tr-TR', nl: 'nl-NL',
    };
    return map[l] || 'en-US';
  }

  /* ---------------- 用户偏好（音色 / 语速） ---------------- */

  const LS_VOICE = 'ww.speak.voice';   // 形如 "en-US|Microsoft Aria Online (Natural)"
  const LS_RATE = 'ww.speak.rate';
  const DEFAULT_RATE = 0.95;

  function lsGet(k) { try { return window.localStorage.getItem(k) || ''; } catch (e) { return ''; } }
  function lsSet(k, v) { try { window.localStorage.setItem(k, v); } catch (e) { /* 忽略 */ } }

  /** 用户手工指定的音色（`语言|名称`）。 */
  function voicePref() { return lsGet(LS_VOICE); }
  function setVoicePref(tag, name) { lsSet(LS_VOICE, tag ? `${tag}|${name}` : ''); }

  /** 语速偏好；未设置时用 0.95（背单词场景稍慢更清楚）。 */
  function ratePref() {
    const r = parseFloat(lsGet(LS_RATE));
    return Number.isFinite(r) && r > 0 ? r : DEFAULT_RATE;
  }
  function setRatePref(r) { lsSet(LS_RATE, String(r)); }

  function resetPrefs() { lsSet(LS_VOICE, ''); lsSet(LS_RATE, ''); }

  /* ---------------- 音色挑选 ---------------- */

  function refreshVoices() {
    try {
      const v = window.speechSynthesis && window.speechSynthesis.getVoices();
      if (v && v.length) voicesCache = Array.from(v);
    } catch (e) { /* 忽略 */ }
    return voicesCache;
  }

  /**
   * 给一个 BCP-47 标签挑最合适的语音。
   * 打分排序：语言完全匹配 > 同语族；「神经网络音色」额外加分 —— 这正是
   * 参考方案里 edge-tts 那套 Neural 音色的本机等价物（WebView2 会暴露
   * 系统里已安装的 Natural/Online 语音）。
   */
  function pickVoice(tag) {
    const voices = refreshVoices();
    if (!voices.length) return null;
    const want = String(tag || 'en-US').toLowerCase().replace('_', '-');
    const base = want.split('-')[0];

    // ① 用户手工选定的音色优先（只要语种对得上就用它）
    const pref = voicePref();
    if (pref) {
      const i = pref.indexOf('|');
      if (i > 0) {
        const ptBase = pref.slice(0, i).toLowerCase().replace('_', '-').split('-')[0];
        const pn = pref.slice(i + 1);
        const hit = voices.find(v => v.name === pn
          && String(v.lang || '').toLowerCase().replace('_', '-').split('-')[0] === ptBase);
        if (hit && ptBase === base) return hit;
      }
    }

    // ② 否则按语言匹配度 + 音色质量打分
    let best = null;
    let bestScore = -1;
    for (const v of voices) {
      const vl = String(v.lang || '').toLowerCase().replace('_', '-');
      if (!vl) continue;
      let s;
      if (vl === want) s = 100;
      else if (vl.split('-')[0] === base) s = 60;
      else continue;

      const n = String(v.name || '');
      if (/natural|neural|online/i.test(n)) s += 40;
      if (/aria|ava|emma|andrew|brian|guy|jenny|michelle|xiaoxiao|xiaoyi|yunxi|nanami|keita/i.test(n)) s += 12;
      if (/google/i.test(n)) s += 10;
      if (/david|zira|mark|hazel|huihui|kangkang/i.test(n)) s += 3;   // 老式 SAPI 兜底
      if (v.localService === false) s += 6;
      if (v.default) s += 2;

      if (s > bestScore) { bestScore = s; best = v; }
    }
    return best;
  }

  /** 某语言当前是否真的有可发声的音色（供界面提示，不阻塞调用）。 */
  function availableLang(lang) {
    const tag = bcp47(lang);
    return !!pickVoice(tag);
  }

  /** 暴露音色清单，设置页用来做下拉。 */
  function voices() {
    return refreshVoices().map(v => ({ name: v.name, lang: v.lang, local: !!v.localService }));
  }

  /* ---------------- 朗读 ---------------- */

  /**
   * 朗读一段文字（单词或整句）。
   * @param {string} text
   * @param {object} opts { audio, lang, accent, rate, pitch, key, force }
   */
  function speak(word, opts = {}) {
    const text = (word || '').trim();
    if (!text) return;
    const lang = opts.lang || 'en';
    const accent = opts.accent || 'us';
    const rate = typeof opts.rate === 'number' && opts.rate > 0 ? opts.rate : ratePref();
    const audioUrl = opts.audio || '';
    const key = opts.key || (text + '|' + accent + '|' + lang);

    // 重复点击同一段 → 停止
    if (currentKey === key && (speaking || (audioEl && !audioEl.paused))) {
      stop();
      currentKey = '';
      return;
    }
    stop();
    currentKey = key;
    lastKey = key;

    if (audioUrl) {
      playUrl(audioUrl, key, { lang, accent, rate });
    } else {
      playTts(text, lang, rate, key, { accent, pitch: opts.pitch });
    }
  }

  /**
   * 播放音频 URL；失败自动降级到 TTS。
   * @param {string} url
   * @param {string} key
   * @param {object} fallback { lang, accent, rate }
   */
  function playUrl(url, key, fallback = {}) {
    if (!url) return;
    const fb = () => playTts(
      (key || '').split('|')[0] || '',
      fallback.lang || 'en',
      typeof fallback.rate === 'number' && fallback.rate > 0 ? fallback.rate : 0,
      key,
      { accent: fallback.accent }
    );
    try {
      if (!audioEl) audioEl = new Audio();
      audioEl.src = url;
      speaking = true;
      audioEl.onended = () => { speaking = false; };
      audioEl.onerror = () => { speaking = false; fb(); };
      const p = audioEl.play();
      if (p && p.catch) p.catch(() => { speaking = false; fb(); });
    } catch (e) {
      speaking = false;
      fb();
    }
  }

  /**
   * 用 WebView 内置语音合成朗读。
   * @param {string} text
   * @param {string} lang   语言代码（en / ja / zh…）或 BCP-47
   * @param {number} rate   语速，1 = 正常
   * @param {string} key    去重用 key
   * @param {object} extra  { accent, pitch }
   */
  function playTts(text, lang, rate, key, extra = {}) {
    const synth = window.speechSynthesis;
    const content = (text || '').trim();
    if (!synth || !content) return;
    try {
      synth.cancel();
      const tag = /^[a-z]{2}-[a-z]{2}$/i.test(String(lang || ''))
        ? String(lang)
        : bcp47(lang, extra.accent);

      // 长句切段：部分引擎对超长文本会静默失败
      const chunks = splitLong(content, 180);
      speaking = true;
      currentKey = key || currentKey;

      chunks.forEach((chunk, i) => {
        const u = new SpeechSynthesisUtterance(chunk);
        u.lang = tag;
        // rate 传 0 / 未传 → 用用户偏好（设置页的「语速」）
        u.rate = typeof rate === 'number' && rate > 0 ? rate : ratePref();
        if (typeof extra.pitch === 'number') u.pitch = extra.pitch;
        const v = pickVoice(tag);
        if (v) u.voice = v;
        const isLast = i === chunks.length - 1;
        u.onend = () => { if (isLast) speaking = false; };
        u.onerror = () => { if (isLast) speaking = false; };
        synth.speak(u);
      });
    } catch (e) {
      speaking = false;
    }
  }

  /** 按标点/长度把长文本切成引擎友好的小段。 */
  function splitLong(text, max) {
    if (text.length <= max) return [text];
    const parts = text.split(/(?<=[.!?;。！？；\n])/);
    const out = [];
    let buf = '';
    for (const p of parts) {
      if ((buf + p).length > max && buf) { out.push(buf.trim()); buf = p; }
      else buf += p;
    }
    if (buf.trim()) out.push(buf.trim());
    return out.length ? out : [text];
  }

  /** 朗读整句/整段（与 speak 相同，语义更直白，供句子场景调用）。 */
  function speakText(text, opts = {}) {
    return speak(text, opts);
  }

  /** 试听：用指定音色/语速念一小段，供设置页按钮调用。 */
  function preview(text, lang) {
    return speak(text || 'Hello, this is how I read.', { lang: lang || 'en', key: 'preview', force: true });
  }

  function stop() {
    speaking = false;
    try {
      if (audioEl) { audioEl.pause(); audioEl.currentTime = 0; }
    } catch (e) { /* 忽略 */ }
    try { window.speechSynthesis && window.speechSynthesis.cancel(); } catch (e) { /* 忽略 */ }
  }

  /**
   * 为一个词条提供「怎么读」的解析：返回可用的音频与口音。
   * 音标字段可能是 {uk, us, audio} 结构，audio 可能是单串或数组。
   */
  function resolve(entry) {
    const ph = (entry && entry.phonetic) || {};
    let audio = ph.audio || '';
    // 后端可能返回 "url1|url2" 或数组
    if (Array.isArray(audio)) audio = audio.filter(Boolean)[0] || '';
    if (typeof audio === 'string' && audio.includes('|')) {
      audio = audio.split('|').filter(Boolean)[0] || '';
    }
    if (typeof audio === 'string' && audio.includes(',')) {
      audio = audio.split(',').filter(Boolean)[0] || '';
    }
    return {
      audio: audio || '',
      lang: (entry && entry.lang) || 'en',
      uk: ph.uk || '',
      us: ph.us || '',
    };
  }

  /** 生成音标旁的小喇叭按钮 HTML。 */
  function btnHtml(entry, accent, label) {
    const acc = accent || 'us';
    return `<button class="speak-btn accent-${acc}" title="朗读${label || ''}" data-speak-accent="${acc}">🔊</button>`;
  }

  /**
   * 事件委托：任何带 .speak-btn 的元素点击都会朗读。
   *
   * 取词优先级：按钮自身 data-speak-word → 最近的 [data-entry-word] →
   * 最近的 [data-speak-text]（整句朗读用）。
   */
  function bindDelegate(root) {
    const el = root || document;
    if (!el || el.__wwSpeakBound) return;
    el.__wwSpeakBound = true;
    el.addEventListener('click', (e) => {
      const t = e && e.target;
      if (!t || typeof t.closest !== 'function') return;
      const b = t.closest('.speak-btn');
      if (!b) return;
      e.stopPropagation();
      e.preventDefault();

      const host = b.closest('[data-entry-word]');
      const sentenceHost = b.closest('[data-speak-text]');
      const word = b.dataset.speakWord
        || (host && host.dataset.entryWord)
        || (sentenceHost && sentenceHost.dataset.speakText)
        || '';
      if (!word) return;
      const accent = b.dataset.speakAccent || 'us';
      const audio = b.dataset.speakAudio || '';
      const lang = b.dataset.speakLang || (sentenceHost && sentenceHost.dataset.speakLang) || 'en';
      const rate = b.dataset.speakRate ? parseFloat(b.dataset.speakRate) : undefined;
      speak(word, { accent, audio, lang, rate, force: true });
    });
  }

  // 预加载语音列表（部分引擎首次 getVoices 为空，需要等 voiceschanged）
  if (window.speechSynthesis) {
    try { refreshVoices(); } catch (e) { /* 忽略 */ }
    try {
      window.speechSynthesis.onvoiceschanged = () => { refreshVoices(); };
    } catch (e) { /* 忽略 */ }
  }

  return {
    speak, speakText, preview, stop, resolve, btnHtml, bindDelegate, bcp47,
    playUrl, playTts, voices, availableLang,
    voicePref, setVoicePref, ratePref, setRatePref, resetPrefs,
  };
})();

window.WW = window.WW || {};
window.WW.speak = Speak.speak;
window.WW.speakText = Speak.speakText;
window.WW.speakStop = Speak.stop;
window.WW.speakBtn = Speak.btnHtml;
window.WW.speakBind = Speak.bindDelegate;
window.WW.speakVoices = Speak.voices;
window.Speak = Speak;
