/* ============================================================
   speak.js —— 单词发音（需求 1、17）
   - 优先播放词典返回的音频 URL（英/美音分开发音）
   - 无音频时降级为浏览器 Web Speech API 朗读
   - 全局单例，避免多个音频叠加
   ============================================================ */

const Speak = (() => {
  let audioEl = null;      // 复用同一个 <audio>，避免并发
  let currentKey = '';     // 当前正在播放的 key，用于重复点击切换
  let speaking = false;

  /** 语言代码 → BCP-47 语音标签 */
  function bcp47(lang, accent) {
    const l = (lang || 'en').toLowerCase();
    if (l === 'en') return accent === 'uk' ? 'en-GB' : 'en-US';
    const map = {
      zh: 'zh-CN', ja: 'ja-JP', ko: 'ko-KR', fr: 'fr-FR',
      de: 'de-DE', es: 'es-ES', ru: 'ru-RU', it: 'it-IT',
      pt: 'pt-PT', ar: 'ar-SA', hi: 'hi-IN',
    };
    return map[l] || l;
  }

  /**
   * 朗读一个单词。
   * @param {string} word   要朗读的词
   * @param {object} opts   { audio, lang, accent, rate, force }
   *   - audio: 音频 URL（优先）
   *   - lang:  语言代码，默认 'en'
   *   - accent:'uk' | 'us'，决定 Web Speech 用英音还是美音
   *   - rate:  语速，默认 0.9（背单词场景稍慢更清晰）
   */
  function speak(word, opts = {}) {
    const text = (word || '').trim();
    if (!text) return;
    const lang = opts.lang || 'en';
    const accent = opts.accent || 'us';
    const rate = typeof opts.rate === 'number' ? opts.rate : 0.9;
    const audioUrl = opts.audio || '';
    const key = text + '|' + accent;

    // 重复点击同一个词 → 停止
    if (currentKey === key && (speaking || (audioEl && !audioEl.paused))) {
      stop();
      currentKey = '';
      return;
    }
    stop();
    currentKey = key;

    if (audioUrl) {
      playUrl(audioUrl, key);
    } else {
      playTts(text, bcp47(lang, accent), rate, key);
    }
  }

  function playUrl(url, key) {
    try {
      if (!audioEl) audioEl = new Audio();
      audioEl.src = url;
      speaking = true;
      audioEl.onended = () => { speaking = false; };
      audioEl.onerror = () => {
        // 音频地址失效 → 降级 TTS
        speaking = false;
        playTts(key.split('|')[0], 'en-US', 0.9, key);
      };
      const p = audioEl.play();
      if (p && p.catch) p.catch(() => {
        speaking = false;
        playTts(key.split('|')[0], 'en-US', 0.9, key);
      });
    } catch (e) {
      speaking = false;
    }
  }

  function playTts(text, voiceLang, rate, key) {
    const synth = window.speechSynthesis;
    if (!synth) return;
    try {
      synth.cancel();
      const u = new SpeechSynthesisUtterance(text);
      u.lang = voiceLang;
      u.rate = rate;
      // 尽量挑一个匹配语言的本地语音
      const voices = synth.getVoices() || [];
      const match = voices.find(v => v.lang && v.lang.replace('_', '-').toLowerCase() === voiceLang.toLowerCase())
        || voices.find(v => v.lang && v.lang.toLowerCase().startsWith(voiceLang.slice(0, 2).toLowerCase()));
      if (match) u.voice = match;
      u.onend = () => { speaking = false; };
      u.onerror = () => { speaking = false; };
      speaking = true;
      synth.speak(u);
    } catch (e) {
      speaking = false;
    }
  }

  function stop() {
    speaking = false;
    try {
      if (audioEl) { audioEl.pause(); audioEl.currentTime = 0; }
    } catch (e) {}
    try { window.speechSynthesis && window.speechSynthesis.cancel(); } catch (e) {}
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
    const r = resolve(entry);
    const acc = accent || 'us';
    const cls = 'speak-btn' + (accent ? ' accent-' + accent : '');
    return `<button class="speak-btn accent-${acc}" title="朗读${label || ''}" data-speak-accent="${acc}">🔊</button>`;
  }

  /**
   * 事件委托：任何带 .speak-btn 的元素点击都会朗读。
   * 按钮需带 data-speak-word，或从最近的 [data-word] 元素取词。
   */
  function bindDelegate(root) {
    const el = root || document;
    el.addEventListener('click', (e) => {
      const b = e.target.closest('.speak-btn');
      if (!b) return;
      e.stopPropagation();
      e.preventDefault();
      const host = b.closest('[data-entry-word]');
      const word = b.dataset.speakWord || (host && host.dataset.entryWord) || '';
      const accent = b.dataset.speakAccent || 'us';
      const audio = b.dataset.speakAudio || '';
      const lang = b.dataset.speakLang || 'en';
      if (word) speak(word, { accent, audio, lang, force: true });
    });
  }

  // 预加载语音列表（部分浏览器首次 getVoices 为空）
  if (window.speechSynthesis) {
    try { window.speechSynthesis.getVoices(); } catch (e) {}
    window.speechSynthesis.onvoiceschanged = () => {
      try { window.speechSynthesis.getVoices(); } catch (e) {}
    };
  }

  return { speak, stop, resolve, btnHtml, bindDelegate, bcp47 };
})();

window.WW = window.WW || {};
window.WW.speak = Speak.speak;
window.WW.speakBtn = Speak.btnHtml;
window.WW.speakBind = Speak.bindDelegate;
window.Speak = Speak;
