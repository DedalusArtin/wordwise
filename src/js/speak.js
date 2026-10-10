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

  /* ---------------- 播放解锁与错误可见化 ----------------

     ★ 为什么需要 unlock：Piper 冷启动合成一条要 3~10 秒（模型加载），
     等音频回来时，点击按钮带来的「用户手势」早已超过 Chromium 约 5 秒
     的有效期 —— `audioEl.play()` 会被 NotAllowedError 拒绝，而且这个
     错误此前被静默吞掉。用户的体感就是「明明合成成功了却没声音」
     （合成缓存的 wav 都落盘了，喇叭却不响）。

     解法：在**手势调用栈内**（speak() 的同步开头）用同一个 <audio>
     元素播一次极短的静音 —— Chromium 对「播过一次的元素」不再套
     自动播放限制，之后无论合成多久都能出声。 */

  let audioUnlocked = false;
  /** 隐藏期间被吞掉的最后一次朗读请求（窗口回前台时补播）。 */
  let pendingSpeak = null;

  /** 10ms 静音 WAV 的 data URL（现生成，44 字节头 + 160 字节数据）。 */
  function silentWavUrl() {
    const sr = 8000, n = sr / 10;
    const buf = new ArrayBuffer(44 + n * 2);
    const v = new DataView(buf);
    const ws = (o, s) => { for (let i = 0; i < s.length; i++) v.setUint8(o + i, s.charCodeAt(i)); };
    ws(0, 'RIFF'); v.setUint32(4, 36 + n * 2, true); ws(8, 'WAVE');
    ws(12, 'fmt '); v.setUint32(16, 16, true);
    v.setUint16(20, 1, true); v.setUint16(22, 1, true);
    v.setUint32(24, sr, true); v.setUint32(28, sr * 2, true);
    v.setUint16(32, 2, true); v.setUint16(34, 16, true);
    ws(36, 'data'); v.setUint32(40, n * 2, true);
    const u8 = new Uint8Array(buf);
    let bin = '';
    for (let i = 0; i < u8.length; i += 8192) {
      bin += String.fromCharCode.apply(null, u8.subarray(i, i + 8192));
    }
    return 'data:audio/wav;base64,' + btoa(bin);
  }

  /**
   * 在用户手势内解锁本模块复用的那个 <audio>。必须**同步**调用
   * （speak() 入口处），放异步链里手势就过期了。失败不打扰 ——
   * 若解锁没成，播放失败时至少还有可见的错误提示兜底。
   */
  function unlockAudio() {
    if (audioUnlocked) return;
    try {
      if (!audioEl) audioEl = new Audio();
      audioEl.muted = true;                 // 静音解锁，听不见也无需听
      audioEl.src = silentWavUrl();
      const p = audioEl.play();
      if (p && p.catch) {
        p.then(() => { audioUnlocked = true; audioEl.muted = false; })
          .catch(() => { audioEl.muted = false; /* 没解开也别留静音状态 */ });
      } else {
        audioUnlocked = true;
        audioEl.muted = false;
      }
    } catch (e) { /* 忽略 */ }
  }

  /**
   * 播放失败**不再静默**：分类给出用户能行动的提示。
   * 之前这里 catch(() => false) 把 NotAllowedError 吞得干干净净，
   * 用户只看到「点了没反应」，排查无从下手。
   */
  function reportPlayError(err, anchor) {
    const name = (err && err.name) || '';
    let msg;
    if (name === 'NotAllowedError') {
      msg = '系统拦下了自动播放：请再点一次朗读按钮';
    } else if (name === 'NotSupportedError') {
      // 合成本身是 WAV（WebView2 必然支持），走到这里几乎都是
      // 合成产物损坏（半截 wav / 非法采样率）—— 重新下载语音包能修复；
      // 词典真人录音是外站 MP3，那种失败走的是另一条降级路，到不了这里。
      msg = '合成出的音频损坏（可能是语音包文件不完整），请到「设置 → 朗读」重新下载该语音包';
    } else {
      msg = '播放失败：' + String((err && (err.message || err)) || '未知原因');
    }
    notifyVoice('none', '', '', anchor);
    healthError(msg);
    try {
      const U = window.WW;
      if (U && typeof U.toast === 'function') U.toast(msg, 'err');
    } catch (e) { /* 忽略 */ }
  }


  /* ---------------- WebAudio 兜底播放 ----------------

     为什么需要它：本机实测（2026-10-07，WebView2 154）这台机器的
     HTMLMediaElement 播放管线整体失灵 —— **任何**来源（在线 MP3、合成
     WAV、甚至 188 字节的标准静音 PCM）play() 都抛 NotSupportedError 或
     **永久挂起**，而 WebAudio 的 decodeAudioData 能把同一段字节完整解码、
     AudioBufferSourceNode 能正常发声到自然结束（onended 触发）。
     元素播不通用这条路把声音真正送到耳朵里，而不是对着红字干瞪眼。 */

  let audioCtx = null;
  let webAudioSrc = null;
  // ★ 按原生采样率缓存上下文：16k 的合成文件用 16k 上下文播放，砍掉
  //   WebView2 的 16k→48k 重采样环节（破音排查中它是嫌疑之一），由系统
  //   音频引擎（WASAPI 共享模式）直接对接实际设备（如 Quantum LT 2 @48k）。
  const ctxByRate = new Map();

  /* ---- 朗读输出设备：每台机器的音频设备不同 ----
     实测有的机器上 WebView2 只看得到一个（可能是虚拟/失效的）输出设备，
     <audio> 管线因此整体失灵。允许把朗读指到指定的输出设备：
     <audio> 走 setSinkId，AudioContext 同样走 setSinkId（Chromium 110+）。 */
  const LS_OUTPUT = 'ww.speak.output';

  function outputPref() {
    try { return localStorage.getItem(LS_OUTPUT) || ''; } catch (e) { return ''; }
  }

  /**
   * 'name:设备名' 不是 deviceId，只是「注册表里看到的设备名」。
   * setSinkId 只认 deviceId，硬传会抛错 —— 这类值只用于界面提示与日志，
   * 播放仍走系统默认设备。
   */
  function isRealDeviceId(id) {
    return !!id && id.indexOf('name:') !== 0;
  }

  function setOutputPref(id) {
    try {
      if (id) localStorage.setItem(LS_OUTPUT, id);
      else localStorage.removeItem(LS_OUTPUT);
    } catch (e) { /* 忽略 */ }
    // 已经活着的 AudioContext 立即跟着换设备
    try {
      if (audioCtx && isRealDeviceId(id) && audioCtx.setSinkId) audioCtx.setSinkId(id).catch(() => {});
    } catch (e) { /* 忽略 */ }
  }

  function applySink(el) {
    const id = outputPref();
    if (!isRealDeviceId(id)) return Promise.resolve();
    if (!el || typeof el.setSinkId !== 'function') return Promise.resolve();
    return el.setSinkId(id).catch(() => { /* 设备可能已拔出，按默认走 */ });
  }

  /* ---------------- 页面可见性 ----------------

     ★ 为什么必须关心它：窗口最小化或收进托盘后，页面进入 hidden ——
       Chromium 会**挂起隐藏页面的 AudioContext**，并节流定时器。于是
       「合成成功却一点声音都没有」，而且因为 `src.start()` 不抛错，
       上层还以为播成功了（既不降级也不报错）。

       修改前全项目 0 处 `document.hidden` / `visibilitychange`，
       所以隐藏态下的失败既没人发现、也没人兜底。 */

  function isHidden() {
    try {
      if (typeof document.hidden === 'boolean') return document.hidden;
      if (document.visibilityState) return document.visibilityState !== 'visible';
    } catch (e) { /* 极简 DOM（冒烟）里没有这些字段 */ }
    return false;
  }

  /**
   * 把 AudioContext 真正拉到 running，**返回 Promise**。
   *
   * ★ 曾经的写法是 `if (ac.state === 'suspended') ac.resume();` ——
   *   resume() 返回 Promise 却被丢弃，紧接着就 decodeAudioData + start()，
   *   此时上下文多半还挂着：start() 不报错、不出声，上层 resolve(true)，
   *   「没声音」于是被当成成功。这里 await 结果并给一个 500ms 上限，
   *   起不来就如实返回 false，让调用方换通路。
   */
  function resumeCtx(ac) {
    if (!ac) return Promise.resolve(false);
    if (ac.state === 'running') return Promise.resolve(true);
    if (typeof ac.resume !== 'function') return Promise.resolve(ac.state === 'running');
    const done = Promise.resolve(ac.resume())
      .then(() => ac.state === 'running')
      .catch(() => false);
    const timer = new Promise((r) => setTimeout(() => r(ac.state === 'running'), 500));
    return Promise.race([done, timer]);
  }

  function ensureAudioCtx() {
    if (!audioCtx) {
      const AC = window.AudioContext || window.webkitAudioContext;
      if (!AC) return null;
      try { audioCtx = new AC(); } catch (e) { return null; }
    }
    try { if (audioCtx.state === 'suspended') audioCtx.resume(); } catch (e) { /* 忽略 */ }
    applySink(audioCtx);
    return audioCtx;
  }

  /**
   * 当前播放上下文的采样率（诊断展示用）。
   *
   * ★ 必须读**真实的** ac.sampleRate，不能拿「请求时传的那个值」充数 ——
   *   new AudioContext({ sampleRate: n }) 只是请求，浏览器完全可以不认。
   *   拿请求值当实测值，诊断面板就会显示一个假数字，把排查带偏。
   */
  function activeCtxRate() {
    let best = 0;
    ctxByRate.forEach((ac) => {
      const r = ac && ac.sampleRate;
      if (r) best = Math.max(best, r);
    });
    return best || (audioCtx ? audioCtx.sampleRate : 0);
  }

  function dataUriToBuffer(dataUri) {
    const b64 = String(dataUri || '').split(',')[1] || '';
    const bin = atob(b64);
    const u8 = new Uint8Array(bin.length);
    for (let i = 0; i < bin.length; i++) u8[i] = bin.charCodeAt(i);
    return u8.buffer;
  }

  /** 从 WAV 字节头读原生采样率（offset 24，小端 u32）。读不到返回 0。 */
  function wavNativeRate(bytes) {
    try {
      const v = new DataView(bytes);
      if (v.getUint32(0, true) !== 0x46464952) return 0;   // 'RIFF'
      if (v.getUint32(8, true) !== 0x45564157) return 0;   // 'WAVE'
      return v.getUint32(24, true);
    } catch (e) { return 0; }
  }

  /** 取（并缓存）指定采样率的 AudioContext；该采样率下 decodeAudioData 不再做重采样。 */
  function ctxFor(rate) {
    const r = rate > 8000 ? rate : 48000;
    let ac = ctxByRate.get(r);
    if (!ac) {
      const AC = window.AudioContext || window.webkitAudioContext;
      if (!AC) return null;
      try { ac = new AC({ sampleRate: r }); } catch (e) { return null; }
      ctxByRate.set(r, ac);
    }
    try { if (ac.state === 'suspended') ac.resume(); } catch (e) { /* 忽略 */ }
    applySink(ac);
    return ac;
  }

  /**
   * 用 WebAudio 播一段 data URI 音频。返回 Promise<boolean>（true = 已开始出声）。
   *
   * ★ 这是朗读的**主通路**，不是兜底。真实应用内对照实测（同一段 16 kHz 音频）：
   *     16 kHz 上下文（按文件原生率建） → 峰值 0.875，削波样本 0
   *     48 kHz 上下文（默认，会重采样） → 峰值 1.4589，削波样本 827
   *   重采样过冲到满刻度的 145.9%，听起来就是「喷麦很炸」。按文件原生率建
   *   上下文可以直接绕开这一步。
   */
  function playViaWebAudio(dataUri, anchor) {
    let bytes;
    try { bytes = dataUriToBuffer(dataUri); } catch (e) { return Promise.resolve(false); }
    // ★ 适配：按文件原生采样率建上下文（16k 文件 → 16k 上下文），解码零重采样
    const native = wavNativeRate(bytes);
    const ac = ctxFor(native);
    if (!ac || !ac.decodeAudioData) return Promise.resolve(false);
    // ★ 先确认上下文真的在跑，再解码开播。隐藏窗口下它多半是 suspended，
    //   不等这一步就会出现「start() 成功、耳朵里没声」。
    return resumeCtx(ac).then((running) => {
      if (!running) return false;
      return ac.decodeAudioData(bytes.slice(0)).then((audioBuf) => {
        return new Promise((resolve) => {
          let src;
          try {
            src = ac.createBufferSource();
            src.buffer = audioBuf;
            src.connect(ac.destination);
          } catch (e) { resolve(false); return; }
          webAudioSrc = src;
          let done = false;
          const finish = () => {
            if (done) return;
            done = true;
            speaking = false;
            if (webAudioSrc === src) webAudioSrc = null;
          };
          src.onended = finish;
          try {
            src.start();
          } catch (e) {
            finish();
            resolve(false);
            return;
          }
          // ★ 开播那一刻状态必须仍是 running。start() 本身不校验可见性，
          //   隐藏页面上它照样返回成功 —— 只有看 state 才分得清
          //   「真的在响」和「排了个队但没声」。
          if (ac.state !== 'running') {
            try { src.stop(); } catch (e) { /* 忽略 */ }
            finish();
            resolve(false);
            return;
          }
          // 真的开播了就算成功。不等 onended —— 那要等整段读完（好几秒），
          // 「要不要回退」这个决策没必要跟着等。
          resolve(true);
        // 兜底清状态：个别环境不派发 onended，speaking 会永久为真，之后所有
        // 朗读都被当成「正在播」而点不动。按时长 + 余量收尾。
          // 兜底清状态：个别环境不派发 onended，speaking 会永久为真，之后所有
          // 朗读都被当成「正在播」而点不动。按时长 + 余量收尾。
          // （隐藏页面的 setTimeout 会被节流，所以 speak() 入口另有一道
          //   「按活动源复位」的兜底，两条一起才不会把状态锁死。）
          const tail = ((audioBuf && audioBuf.duration) || 0) * 1000 + 1500;
          if (tail > 0) setTimeout(finish, tail);
        });
      }).catch(() => false);
    }).catch(() => false);
  }

  /**
   * 交给**系统原生**播放（Windows：Rust 侧 winmm `PlaySoundW`）。
   *
   * 窗口最小化 / 收进托盘时页面进入 hidden，WebView 里的两条管线都会失灵，
   * 而原生播放由后端直接把字节送给系统音频接口，与页面可见性、自动播放
   * 策略、后台节流统统无关 —— 是隐藏态下唯一稳的一条路。
   *
   * @returns {Promise<boolean>} true = 已交出去
   */
  function playViaNative(dataUri) {
    const API = (window.WordWiseAPI && window.WordWiseAPI.API) || null;
    if (!dataUri || !API || typeof API.ttsPlayNative !== 'function') {
      return Promise.resolve(false);
    }
    return API.ttsPlayNative(dataUri).then((ok) => !!ok).catch(() => false);
  }


  /* ---------------- 语音健康（常驻状态，不再一闪而过） ----------------

     用户原话：「点试听时下面有红色提示……太快了没看清，感觉这个要在语音模块提示」。
     toast 会消失，朗读链路里每一步失败（本地合成失败 → 回退系统语音 → 系统又
     没音色）的**最终原因**必须常驻在两处看得见的地方：设置页朗读面板顶部、
     侧边栏语音状态条。这里只维护状态与订阅，谁显示谁订阅。 */

  const healthSubs = [];
  let ttsHealth = { state: 'ok', message: '' };   // state: 'ok' | 'error'

  function notifyHealth(state, message) {
    if (ttsHealth.state === state && ttsHealth.message === message) return;
    ttsHealth = { state, message };
    for (const fn of healthSubs) {
      try { fn(ttsHealth); } catch (e) { /* 单个订阅者出错不影响其它 */ }
    }
  }

  /** 订阅语音健康变化；返回退订函数。 */
  function onHealth(fn) {
    if (typeof fn === 'function') healthSubs.push(fn);
    return () => {
      const i = healthSubs.indexOf(fn);
      if (i >= 0) healthSubs.splice(i, 1);
    };
  }

  /** 记录一次朗读链路失败（常驻，直到下次成功朗读覆盖）；同时落日志供诊断模块分析。 */
  function healthError(message) {
    const msg = String(message || '');
    notifyHealth('error', msg);
    try {
      const API = (window.WordWiseAPI && window.WordWiseAPI.API) || null;
      if (API && typeof API.logWrite === 'function') API.logWrite('error', '[tts] ' + msg);
    } catch (e) { /* 日志失败不能拖累发声 */ }
  }

  /** 朗读成功 → 清除错误态（顺带清掉「隐藏期间待补播」的那一条）。 */
  function healthOk() {
    pendingSpeak = null;
    if (ttsHealth.state !== 'ok') notifyHealth('ok', '');
  }

  /**
   * 窗口可见性变化。
   *
   * 回到前台做两件事：
   *   ① 把所有缓存的 AudioContext 拉回 running —— 隐藏期间它们被浏览器
   *      挂起过，不恢复的话回前台第一次朗读照样没声（要等第二次 resume）；
   *   ② 补播隐藏期间被吞掉的那一次（超过 30 秒就算了 —— 隔半天突然响
   *      一声比不响更吓人）。
   */
  function onVisibilityChange() {
    if (isHidden()) return;
    try {
      ctxByRate.forEach((ac) => {
        if (ac && ac.state !== 'running' && typeof ac.resume === 'function') {
          Promise.resolve(ac.resume()).catch(() => {});
        }
      });
      if (audioCtx && audioCtx.state !== 'running' && typeof audioCtx.resume === 'function') {
        Promise.resolve(audioCtx.resume()).catch(() => {});
      }
    } catch (e) { /* 恢复失败不影响主流程 */ }
    if (pendingSpeak && Date.now() - pendingSpeak.at < 30000) {
      const p = pendingSpeak;
      pendingSpeak = null;
      setTimeout(() => speak(p.text, Object.assign({}, p.opts, { force: true })), 120);
    }
  }
  try {
    document.addEventListener('visibilitychange', onVisibilityChange);
  } catch (e) { /* 极简 DOM 没有 document */ }

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

  /* ---------------- 本地语音的全局门面 ----------------
     切换语音 / 可用性同步的**唯一入口**。此前这套逻辑散在设置页里，
     出过两类只有全局收敛才能根除的事故：
       · IPC 参数名手写错（`voice_local` vs `voiceLocal`）—— 后端收到
         null、配置根本没写，界面上却毫无异常，「点了使用却没切换」；
       · 可用性只在设置页刷新 —— 用户查词页直接朗读时它还是初始值，
         本地通道被整段跳过，回退系统语音又挑不出音色，就成了
         「没有可用语音」。 */

  /** 后端当前指定的本地语音包 id（空 = 按词条语言自动挑）。 */
  let currentLocalVoice = '';
  /** 是否已经从后端同步过本地语音可用性。 */
  let readySynced = false;
  /** 去重的同步 Promise（并发调用共用一次）。 */
  let syncingReady = null;

  /**
   * 从后端同步本地语音的可用性与当前选择。
   *
   * 应用启动后的**第一次朗读前**必须走到一次，否则 `localReady` 永远是
   * 初始 false。同步失败不锁死（清掉去重句柄，下次再试）。
   */
  function ensureReady() {
    if (readySynced) return Promise.resolve();
    if (!syncingReady) {
      const API = (window.WordWiseAPI && window.WordWiseAPI.API) || null;
      if (!API || typeof API.ttsStatus !== 'function') {
        readySynced = true;   // 无后端环境（测试 / 老壳）就别反复试
        return Promise.resolve();
      }
      syncingReady = API.ttsStatus().then((st) => {
        readySynced = true;
        localReady = !!st && !!st.engine_ready && (st.installed || []).length > 0;
        currentLocalVoice = (st && st.config && st.config.voice_local) || '';
      }).catch(() => { syncingReady = null; });
    }
    return syncingReady;
  }

  /**
   * 切换「当前使用的本地语音包」。设置页只准走这里，不许自己拼参数。
   * @param {string} id 语音包 id；空串 = 清除指定（回到按语言自动挑）
   */
  function selectVoice(id) {
    currentLocalVoice = id || '';   // 先改内存：高频读，不能等 IPC 回来
    const API = (window.WordWiseAPI && window.WordWiseAPI.API) || null;
    if (!API || typeof API.setTtsPrefs !== 'function') return Promise.resolve(false);
    return API.setTtsPrefs({ voiceLocal: currentLocalVoice })
      .then(() => true)
      .catch(() => false);
  }

  /** 后端把配置改了（设置页 loadTts 拉到新状态）→ 推给门面。 */
  function setLocalVoiceId(id) { currentLocalVoice = id || ''; }
  function localVoiceId() { return currentLocalVoice; }


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
    none: '没有可用语音',    // 两条通道都凑不出音色（如实说，别假装是系统语音）
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
    // 可用性还没同步过 → 先同步一次再决定（应用启动后的第一次朗读走这里）。
    // 没有这一步，用户装好了语音包也会因为 localReady 停在初始值
    // 而永远走不上本地通道。
    if (!localReady) {
      return ensureReady().then(() => {
        if (!localReady) return false;
        return playLocalTts(text, lang, accent, rate, key, anchor);
      });
    }

    return API.ttsSpeak(text, lang, accent)
      .then((res) => {
        if (!res || !res.audio) return false;
        // 合成期间用户可能已经点了别的词 —— 那就别出声了
        if (currentKey !== key) return true;
        // ★ res.voice 是后端**实际用到的那条语音**（`SynthOut.voice`）。
        //   原来这里把它丢掉了，界面于是没法回答「这句到底是谁读的」。
        notifyVoice('local', res.voice || '', text, anchor);
        // ★ 主通路 = WebAudio（按文件原生采样率建上下文，解码零重采样）。
        //   <audio> 元素做不到这一点：它只能按设备采样率（本机 48k）重采样，
        //   而 16k→48k 的重采样实测过冲到 1.4589（827 个样本被削）—— 就是
        //   用户说的「喷麦很炸」。所以顺序倒过来：<audio> 降级成兜底。
        // WebAudio 主通路 → <audio> 兜底 → 原生播放最后兜一道。
        const viaWebView = () => playViaWebAudio(res.audio, anchor).then((ok) => {
          if (ok) { healthOk(); return true; }
          // WebAudio 这条路走不通（极少数环境的 WebView2）才回到元素播放
          return playViaElement(res.audio, key).then((ok2) => {
            if (ok2) { healthOk(); return true; }
            // ★ 最后再试原生：窗口刚被藏起来的那一瞬间也会落到这里 ——
            //   AudioContext 挂起、元素被自动播放策略挡住，只有原生还能出声。
            return playViaNative(res.audio).then((ok3) => {
              if (ok3) { healthOk(); return true; }
              reportPlayError(
                { name: 'NotSupportedError', message: 'WebAudio、<audio> 与原生播放三条通路都没能出声' },
                anchor);
              return false;
            });
          });
        });

        // ★ 窗口不可见（最小化 / 收进托盘）时，页面里的两条管线大概率失灵：
        //   AudioContext 被挂起、<audio> 受限。这时**先**走原生，
        //   走不通再退回页面里的通路（万一只是「藏起来的瞬间」，页面还活着）。
        if (isHidden()) {
          return playViaNative(res.audio).then((ok) => {
            if (ok) { healthOk(); return true; }
            return viaWebView();
          });
        }
        return viaWebView();
      })
      .catch((err) => {
        // 后端合成失败：原因要留档（回退系统语音若也发不出声，
        // 用户在常驻状态里能看到本地失败的真正原因，而不是只有「没声音」）
        healthError('本地合成失败：' + String((err && (err.message || err)) || '未知原因')
          + '；已改用系统语音');
        return false;
      });
  }

  /**
   * 用 <audio> 元素播一段 data URI（WebAudio 走不通时的兜底）。
   *
   * 只在 WebAudio 失败时才走到这里 —— 元素播放会按设备采样率重采样，16k 的音
   * 频在 48k 设备上会过冲削波（实测峰值 1.4589），音质不如 WebAudio 通路。
   * 但它是最后一道保险：有的环境 WebAudio 起不来，能出声总比无声强。
   *
   * @returns {Promise<boolean>} true = 已开始出声
   */
  function playViaElement(dataUri, key) {
    if (!dataUri) return Promise.resolve(false);
    if (!audioEl) audioEl = new Audio();
    audioEl.src = dataUri;
    speaking = true;
    audioEl.onended = () => { speaking = false; };
    audioEl.onerror = () => { speaking = false; };
    // 先把输出指到选定设备（如有）再开播 —— setSinkId 完成后才 play
    const p = Promise.resolve(applySink(audioEl)).then(() => audioEl.play());
    if (!p || !p.catch) return Promise.resolve(true);
    // ★ 有些 Windows 环境的 WebView2 播放管线整体失灵：play() 对任何来源都抛
    //   NotSupportedError，甚至**永久挂起**（实测机器：能解码、不能开播，连
    //   188 字节标准 PCM 都失败）。等 3 秒不结算就按管线故障处理，不能再等。
    const stalled = new Promise((r) => setTimeout(() => r({ __stalled: true }), 3000));
    return Promise.race([p.then(() => ({ ok: true })).catch((err) => ({ err })), stalled])
      .then((got) => {
        if (got && got.ok) return true;
        if (got && got.__stalled) {
          try { audioEl.pause(); } catch (e) { /* 忽略 */ }
        }
        speaking = false;
        return false;
      });
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

    // ★ 状态自愈：隐藏页面的 setTimeout 会被 Chromium 节流（钳到 1 秒，
    //   长时间隐藏后甚至 1 分钟一次），收尾定时器可能迟迟不到，speaking
    //   就一直为真 —— 之后每次点朗读都被当成「正在播」而走停止分支，
    //   表现为「点了没声音」。这里按「有没有真的活动源」复位，不依赖定时器。
    if (speaking && !webAudioSrc && (!audioEl || audioEl.paused)) speaking = false;

    // 重复点击同一段 → 停止
    if (currentKey === key && (speaking || (audioEl && !audioEl.paused))) {
      stop();
      currentKey = '';
      return;
    }
    // 隐藏期间发起的朗读：先记下来，等窗口回到前台补一次（见 visibilitychange）。
    // 不记的话，最小化后点发音就等于石沉大海。
    if (isHidden()) pendingSpeak = { text, opts, at: Date.now() };
    // ★ 媒体解锁必须在**这个同步栈**里做（此刻还握着用户手势）：
    //   Piper 冷启动合成要 3~10 秒，等到能 play() 时手势早过期了，
    //   Chromium 会以 NotAllowedError 拒绝 —— 「合成成功却没声音」的主因。
    unlockAudio();
    ensureAudioCtx();   // WebAudio 兜底通道也要在手势内创建/恢复
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
      if (p && p.catch) {
        p.then(() => healthOk()).catch(() => { speaking = false; fb(); });
      } else {
        healthOk();
      }
    } catch (e) {
      speaking = false;
      fb();
    }
  }

  /**
   * 「机器里没有这门语言的任何音色」——如实告诉用户该去哪儿补。
   *
   * ★ 为什么需要它：朗读是高频操作，每次失败都弹提示会把人烦死；但一次都不
   *   说，用户就只能对着「点了喇叭没反应」干瞪眼（真实反馈：「切换 piper 却
   *   还是系统音色」有一半是被这个沉默坑出来的 —— 其实本机一条英语音色都没有）。
   *   所以折中：**每种语言一个会话只提醒一次**。
   */
  const missingVoiceWarned = new Set();
  function hintMissingVoice(lang) {
    const key = String(lang || 'en');
    if (missingVoiceWarned.has(key)) return;
    missingVoiceWarned.add(key);
    // ★ 常驻到语音健康状态（侧边栏 + 设置页朗读面板），toast 只配当配角 ——
    //   5 秒的红条没人来得及看清（用户原话「太快了我没看清」）。
    const nm = (window.WW && window.WW.langLabel && window.WW.langLabel(key)) || key;
    healthError('本机没有' + nm + '的系统语音，朗读发不出声。两个办法：'
      + '在 Windows「设置 → 时间和语言 → 语音」里添加' + nm + '语音；'
      + '或在「设置 → 朗读」下载离线语音包（不依赖系统，装完即可用）');
    try {
      const U = window.WW;
      if (U && typeof U.toast === 'function') {
        U.toast('朗读失败：本机没有' + nm + '语音，详细办法见侧边栏语音状态或设置页朗读面板', 'err');
      }
    } catch (e) { /* 提示本身失败不能拖累发声 */ }
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
      if (!v) {
        // ★ 挑不出音色时**不能**照旧报「系统语音」：那样界面写着「系统语音」、
        //   耳朵里却一点声音都没有 —— 用户会以为朗读功能坏了（真实反馈：
        //   「为什么切换 piper 但还是系统音色」的一半原因就在这里）。
        //   分两种情况：清单已经加载过却没有这门语言 → 确实缺，如实告知；
        //   清单还一次都没加载出来（引擎异步） → 不能冤枉它，照旧尝试播放。
        if (voices().length) {
          speaking = false;
          notifyVoice('none', '', content, extra.anchor);
          hintMissingVoice(lang);
          return;
        }
      }
      notifyVoice('system', v ? v.name : '', content, extra.anchor);

      chunks.forEach((chunk, i) => {
        const u = new SpeechSynthesisUtterance(chunk);
        u.lang = tag;
        // rate 传 0 / 未传 → 用用户偏好（设置页的「语速」）
        u.rate = typeof rate === 'number' && rate > 0 ? rate : ratePref();
        if (typeof extra.pitch === 'number') u.pitch = extra.pitch;
        if (v) u.voice = v;
        const isLast = i === chunks.length - 1;
        u.onend = () => { if (isLast) { speaking = false; healthOk(); } };
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
    try { if (webAudioSrc) { webAudioSrc.stop(); webAudioSrc = null; } } catch (e) { /* 忽略 */ }
    try { window.speechSynthesis && window.speechSynthesis.cancel(); } catch (e) { /* 忽略 */ }
    // 窗口藏着时可能是原生通路在响 —— 点停止也得让它停
    try {
      const API = (window.WordWiseAPI && window.WordWiseAPI.API) || null;
      if (API && typeof API.ttsStopNative === 'function') Promise.resolve(API.ttsStopNative()).catch(() => {});
    } catch (e) { /* 忽略 */ }
    pendingSpeak = null;
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
  //
  // ★ 用 addEventListener 而不是给 `onvoiceschanged` 赋值：后者是**单槽位**
  //   属性，设置页也用同一招刷新下拉，两边会互相覆盖 —— 谁后绑定谁生效，
  //   另一个就永远收不到通知（设置页先绑、speak.js 后绑的话，下拉就再也没
  //   机会刷新，表现就是「系统音色列表一直是空的 / 选了没反应」）。
  if (window.speechSynthesis) {
    try { refreshVoices(); } catch (e) { /* 忽略 */ }
    try {
      window.speechSynthesis.addEventListener('voiceschanged', refreshVoices);
    } catch (e) {
      // 老 WebView 没有 addEventListener（非 EventTarget）→ 退回单槽位赋值
      try { window.speechSynthesis.onvoiceschanged = refreshVoices; } catch (e2) { /* 忽略 */ }
    }
  }

  return {
    speak, speakText, preview, stop, resolve, btnHtml, bindDelegate, bcp47,
    playUrl, playTts, voices, availableLang,
    playViaWebAudio,   // 导出：冒烟测试直接断言 WebAudio 播放
    playViaElement,    // 导出：冒烟测试直接断言 <audio> 兜底
    playViaNative,     // 导出：冒烟测试直接断言「隐藏态走原生通路」
    resumeCtx, isHidden,   // 导出：窗口收起时的播放守卫（冒烟断言用）
    outputPref, setOutputPref,   // 导出：朗读输出设备偏好（设置页读写）
    activeCtxRate,               // 导出：当前 WebAudio 上下文采样率（诊断用）
    voicePref, setVoicePref, ratePref, setRatePref, resetPrefs,
    enginePref, setEnginePref, setLocalReady, isLocalReady, playLocalTts,
    // 本地语音门面：切换 / 同步 / 读取当前生效的语音包
    ensureReady, selectVoice, setLocalVoiceId, localVoiceId,
    // 「当前语音」：onVoice 订阅变化，currentVoice 直接读最新值
    onVoice, currentVoice, voiceLabel, voiceKindLabel,
    onHealth,
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
