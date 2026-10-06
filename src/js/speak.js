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

  /* ---------------- 朗读引擎策略 ---------------- */

  /*
    三条通道并存，按偏好决定用哪条：
      ① 词典真人音频 URL —— 永远最高优先，有录音就没理由去合成
      ② 本地神经语音（后端 Piper）—— 离线，音质接近微软 Neural
      ③ 系统语音（WebView speechSynthesis）—— 兜底，永远可用

    偏好存在 localStorage 而不是每次去问后端：发音是高频操作，而且
    点击必须**同步**开始处理，不能在 await 里错过用户手势带来的播放许可。
    设置页加载时会用后端值覆盖它（见 Refresh.pullFromBackend）。
  */
  const LS_ENGINE = 'ww.tts.engine';
  const ENGINES = ['auto', 'local', 'online', 'system'];

  function enginePref() {
    const v = lsGet(LS_ENGINE);
    return ENGINES.includes(v) ? v : 'auto';
  }
  function setEnginePref(v) {
    lsSet(LS_ENGINE, ENGINES.includes(v) ? v : 'auto');
  }

  /** 本地语音当前是否可用（后端有引擎且至少装了一条语音包）。 */
  let localReady = false;
  function setLocalReady(v) { localReady = !!v; }
  function isLocalReady() { return localReady; }

  /* ---------------- 当前语音（「刚才这句是用什么读的」） ----------------

     需求原文：「朗读时在界面旁显示当前用的是哪种语音（如 AI 或本地语音）」。

     ★ 数据早就有了，只是没人接：后端 `SynthOut` 里就带着实际使用的
       `voice` 字段（`tts/mod.rs` 的返回值），而 `playLocalTts` 原来只取
       `res.audio`，把 `res.voice` 丢了 —— 界面上于是永远说不清
       「这句是 Piper 合成的，还是 WebView 系统语音念的」。
       两者音质差别明显，用户无从判断到底是哪条通道在生效。

     三条通道各自上报：
       dict   词典真人录音（音频 URL）
       local  本地神经语音（后端 Piper）—— 就是用户说的「AI 语音」
       system 系统语音（WebView speechSynthesis）—— 兜底
  */
  let lastVoice = null;             // { engine, voice, text, at }
  const voiceSubs = [];

  /** 通道 + 音色名 → 给用户看的一句话。 */
  const VOICE_KIND = {
    dict: '词典真人录音',
    local: '本地 AI 语音',   // 后端 Piper 合成的神经语音
    system: '系统语音',      // WebView speechSynthesis 兜底
  };
  function voiceKindLabel(engine) {
    return VOICE_KIND[engine] || VOICE_KIND.system;
  }
  function voiceLabel(engine, voice) {
    const k = voiceKindLabel(engine);
    return voice ? `${k} · ${voice}` : k;
  }

  /**
   * 上报「本次用了哪条通道、哪个音色」。
   * @param {'dict'|'local'|'system'} engine
   * @param {string} voice  音色名/模型 id（没有就传空串）
   * @param {string} text   被朗读的文字
   * @param {Element} [anchor] 触发朗读的按钮 —— 提示条会贴在它旁边
   */
  function notifyVoice(engine, voice, text, anchor) {
    lastVoice = { engine: engine || 'system', voice: voice || '', text: text || '', at: Date.now() };
    const label = voiceLabel(lastVoice.engine, lastVoice.voice);
    // 界面提示：ui.js 提供实现。ui.js 在本文件之后加载，所以运行时才取。
    try {
      if (window.WW && window.WW.voiceTip) window.WW.voiceTip(lastVoice, label, anchor);
    } catch (e) { /* 提示条画不出来不该影响发声 */ }
    // 其它订阅者（设置页的「最近一次合成」用它）
    for (const fn of voiceSubs) {
      try { fn(lastVoice, label); } catch (e) { /* 单个订阅者出错不影响其它 */ }
    }
  }

  /** 订阅「当前语音」变化；返回退订函数。 */
  function onVoice(fn) {
    if (typeof fn !== 'function') return () => {};
    voiceSubs.push(fn);
    // 立刻回灌一次当前值，订阅者不用等下一次朗读才知道现状
    if (lastVoice) { try { fn(lastVoice, voiceLabel(lastVoice.engine, lastVoice.voice)); } catch (e) {} }
    return () => {
      const i = voiceSubs.indexOf(fn);
      if (i >= 0) voiceSubs.splice(i, 1);
    };
  }
  function currentVoice() { return lastVoice; }

  /**
   * 用后端本地引擎合成并播放。
   *
   * 返回 Promise<boolean>：true = 已经接手播放（或已确定不需要回退），
   * false = 需要调用方回退到系统语音。**不吞异常**是刻意的 —— 任何一步
   * 出问题都回退系统语音，绝不让用户点了没声音。
   */
  function playLocalTts(text, lang, accent, rate, key, anchor) {
    const API = (window.WordWiseAPI && window.WordWiseAPI.API) || null;
    if (!API || typeof API.ttsSpeak !== 'function') return Promise.resolve(false);
    if (!localReady) return Promise.resolve(false);

    return API.ttsSpeak(text, lang, accent)
      .then((res) => {
        if (!res || !res.audio) return false;
        // 合成期间用户可能已经点了别的词 —— 那就别出声了
        if (currentKey !== key) return true;
        if (!audioEl) audioEl = new Audio();
        audioEl.src = res.audio;
        speaking = true;
        // ★ res.voice 是后端**实际用到的那条语音**（`SynthOut.voice`）。
        //   原来这里把它丢掉了，界面于是没法回答「这句到底是谁读的」。
        notifyVoice('local', res.voice || '', text, anchor);
        audioEl.onended = () => { speaking = false; };
        audioEl.onerror = () => { speaking = false; };
        const p = audioEl.play();
        if (p && p.catch) {
          return p.then(() => true).catch(() => { speaking = false; return false; });
        }
        return true;
      })
      .catch(() => false);
  }

  /* ---------------- 朗读 ---------------- */

  /**
   * 朗读一段文字（单词或整句）。
   * @param {string} text
   * @param {object} opts { audio, lang, accent, rate, pitch, key, force, anchor }
   *   `anchor` 是触发朗读的元素，用于把「当前语音」提示条贴在它旁边。
   */
  function speak(word, opts = {}) {
    const text = (word || '').trim();
    if (!text) return;
    const lang = opts.lang || 'en';
    const accent = opts.accent || 'us';
    const rate = typeof opts.rate === 'number' && opts.rate > 0 ? opts.rate : ratePref();
    const audioUrl = opts.audio || '';
    const key = opts.key || (text + '|' + accent + '|' + lang);
    const anchor = opts.anchor || null;

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
      playUrl(audioUrl, key, { lang, accent, rate, anchor });
      return;
    }

    // 词典没给真人音频 → 按偏好选合成通道。
    // 「系统语音」是显式选择，直接走 WebView；其余（auto / local / online）
    // 都先试本地引擎，失败再回退系统语音 —— 回退是无声的，用户不需要知道，
    // 但「当前语音」提示条会如实显示最后**真正**发声道的那一条。
    if (enginePref() === 'system') {
      playTts(text, lang, rate, key, { accent, pitch: opts.pitch, anchor });
      return;
    }
    playLocalTts(text, lang, accent, rate, key, anchor).then((handled) => {
      if (!handled) playTts(text, lang, rate, key, { accent, pitch: opts.pitch, anchor });
    });
  }

  /**
   * 播放音频 URL；失败自动降级到 TTS。
   * @param {string} url
   * @param {string} key
   * @param {object} fallback { lang, accent, rate, anchor }
   */
  function playUrl(url, key, fallback = {}) {
    if (!url) return;
    const fb = () => playTts(
      (key || '').split('|')[0] || '',
      fallback.lang || 'en',
      typeof fallback.rate === 'number' && fallback.rate > 0 ? fallback.rate : 0,
      key,
      { accent: fallback.accent, anchor: fallback.anchor }
    );
    try {
      if (!audioEl) audioEl = new Audio();
      audioEl.src = url;
      speaking = true;
      notifyVoice('dict', '', (key || '').split('|')[0] || '', fallback.anchor);
      audioEl.onended = () => { speaking = false; };
      // 真人录音挂了（外链失效、被墙）→ 降级合成，提示条也会跟着改成合成通道
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
   * @param {object} extra  { accent, pitch, anchor }
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

      // 系统语音这一路的音色是**按 tag + 打分**挑出来的，取一次就够，
      // 顺便把它报给界面（用户能看出「原来是系统语音在念」）。
      const v = pickVoice(tag);
      notifyVoice('system', v ? v.name : '', content, extra.anchor);

      chunks.forEach((chunk, i) => {
        const u = new SpeechSynthesisUtterance(chunk);
        u.lang = tag;
        // rate 传 0 / 未传 → 用用户偏好（设置页的「语速」）
        u.rate = typeof rate === 'number' && rate > 0 ? rate : ratePref();
        if (typeof extra.pitch === 'number') u.pitch = extra.pitch;
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
    try { if (window.WW && window.WW.voiceTipHide) window.WW.voiceTipHide(); } catch (e) { /* 忽略 */ }
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

  /**
   * 生成音标旁的小喇叭按钮 HTML。
   *
   * 图标是内联 SVG 而不是 emoji：emoji 的字形宽度由平台字体决定，
   * 同一个 🔊 在 Windows / Android 上的宽度不一样，一排发音按钮就会参差。
   * SVG 的尺寸由 CSS（`svg.ico`）决定，与字体无关。
   */
  function btnHtml(entry, accent, label) {
    const acc = accent || 'us';
    const ico = (window.WW && window.WW.icon) ? window.WW.icon('sound') : '';
    return `<button class="speak-btn accent-${acc}" title="朗读${label || ''}" data-speak-accent="${acc}">${ico}</button>`;
  }

  /**
   * 事件委托：任何带 .speak-btn 的元素点击都会朗读。
   *
   * ★ 取词与取音频都必须走**同一套三级回退**，这是「上方读不了、下方能读」
   *   那类不对称 bug 的根因：
   *
   *   原来取词有三段回退（按钮自身 → 最近 [data-entry-word] → 最近
   *   [data-speak-text]），取音频却**只读按钮自身的 `data-speak-audio`**，
   *   从不看容器。而真人录音 URL 是挂在**容器**上的（背诵页的音标行、
   *   反馈条、上一个词、已背列表的容器都带 `data-speak-audio`），按钮自己
   *   身上没有 —— 于是同样一个词，从容器里渲染出来的按钮能放真人录音，
   *   从别处渲染出来的按钮只能干走 TTS，听起来就是「有的地方能读、有的
   *   地方读不出来」。
   *
   *   现在 audio / lang 也照 word 的三级回退读，两边口径完全一致。
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
      // 只掐掉「点按钮顺带触发外层可点区域」这类行为（如折叠面板的 summary、
      // 整条可点的「上一个单词」）。这与 ui.js 里可点词的统一入口不冲突：
      // 那边是另一种元素，且它在冒泡更外层，标记不同。
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
      // 音频 / 语言：与 word 完全相同的三级回退（按钮 → 词条容器 → 整句容器）
      const audio = b.dataset.speakAudio
        || (host && host.dataset.speakAudio)
        || (sentenceHost && sentenceHost.dataset.speakAudio)
        || '';
      const lang = b.dataset.speakLang
        || (host && host.dataset.speakLang)
        || (sentenceHost && sentenceHost.dataset.speakLang)
        || 'en';
      const rate = b.dataset.speakRate ? parseFloat(b.dataset.speakRate) : undefined;
      speak(word, { accent, audio, lang, rate, force: true, anchor: b });
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
    enginePref, setEnginePref, setLocalReady, isLocalReady, playLocalTts,
    // 「当前语音」：onVoice 订阅变化，currentVoice 直接读最新值
    onVoice, currentVoice, voiceLabel, voiceKindLabel,
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
