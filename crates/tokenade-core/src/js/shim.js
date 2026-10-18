// Portable DOM/BOM shim for running the Cloudflare challenge-platform VM
// inside a bare JS engine (QuickJS / boa / V8 — no browser).
//
// Constraints: PLAIN ES2020 only. No Node APIs, no host builtins beyond what
// QuickJS ships. Everything the challenge can touch must resolve here.
//
// The shim is *recording*: every identifier the VM looks up and every property
// it reads goes through a Proxy that logs misses, so the required API surface
// is discovered empirically instead of guessed.

function makeShim(cfg) {
  var LOG = { miss: Object.create(null), hit: Object.create(null), net: [], errors: [] };

  function note(bucket, key) {
    bucket[key] = (bucket[key] || 0) + 1;
  }

  // ---- recording proxy -----------------------------------------------------
  // In diagnose mode a missing property yields a chainable stub instead of
  // undefined, so the VM keeps running and reveals its *whole* API demand in
  // one pass rather than one TypeError at a time.
  function stub(path) {
    var f = function () { return stub(path + '()'); };
    return new Proxy(f, {
      get: function (t, p) {
        if (typeof p === 'symbol') {
          if (p === Symbol.toPrimitive) return function () { return 0; };
          return undefined;
        }
        if (p === 'toString' || p === 'valueOf') return function () { return ''; };
        note(LOG.miss, path + '.' + p);
        return stub(path + '.' + p);
      },
      set: function () { return true; },
      apply: function () { return stub(path + '()'); },
      construct: function () { return stub('new ' + path); },
    });
  }

  function rec(name, target) {
    return new Proxy(target, {
      get: function (t, p, r) {
        if (typeof p === 'symbol') return Reflect.get(t, p, r);
        var path = name + '.' + p;
        if (p in t) { note(LOG.hit, path); return Reflect.get(t, p, r); }
        note(LOG.miss, path);
        return cfg.diagnose ? stub(path) : undefined;
      },
      set: function (t, p, v) { return Reflect.set(t, p, v); },
      has: function () { return true; },
    });
  }

  // ---- primitives ----------------------------------------------------------
  var listeners = Object.create(null);

  function EventTargetMix(o) {
    o.addEventListener = function (type, fn) {
      (listeners[type] = listeners[type] || []).push(fn);
    };
    o.removeEventListener = function () {};
    o.dispatchEvent = function () { return true; };
    return o;
  }

  function mkClassList() {
    var set = [];
    return {
      add: function () { for (var i = 0; i < arguments.length; i++) if (set.indexOf(arguments[i]) < 0) set.push(arguments[i]); },
      remove: function () {
        for (var i = 0; i < arguments.length; i++) {
          var j = set.indexOf(arguments[i]);
          if (j >= 0) set.splice(j, 1);
        }
      },
      toggle: function (c) { var j = set.indexOf(c); if (j >= 0) { set.splice(j, 1); return false; } set.push(c); return true; },
      contains: function (c) { return set.indexOf(c) >= 0; },
      replace: function (a, b) { var j = set.indexOf(a); if (j >= 0) { set[j] = b; return true; } return false; },
      item: function (i) { return set[i] === undefined ? null : set[i]; },
      toString: function () { return set.join(' '); },
      get length() { return set.length; },
    };
  }

  function mkElement(tag) {
    var el = EventTargetMix({
      tagName: String(tag).toUpperCase(),
      nodeName: String(tag).toUpperCase(),
      nodeType: 1,
      // Missing entirely until measured: `classList.add` was the 3rd most
      // common reason an inline script died, and a script that dies takes its
      // slice of page state with it.
      classList: mkClassList(),
      style: {},
      children: [],
      childNodes: [],
      attributes: {},
      innerHTML: '',
      textContent: '',
      firstChild: null,
      parentNode: null,
      ownerDocument: null,
      setAttribute: function (k, v) { this.attributes[k] = String(v); this[k] = v; },
      getAttribute: function (k) { return k in this.attributes ? this.attributes[k] : null; },
      removeAttribute: function (k) { delete this.attributes[k]; },
      hasAttribute: function (k) { return k in this.attributes; },
      appendChild: function (c) { this.children.push(c); c.parentNode = this; return c; },
      removeChild: function (c) {
        var i = this.children.indexOf(c);
        if (i >= 0) this.children.splice(i, 1);
        return c;
      },
      insertBefore: function (c) { this.children.push(c); c.parentNode = this; return c; },
      getElementsByTagName: function () { return []; },
      querySelector: function () { return null; },
      querySelectorAll: function () { return []; },
      getBoundingClientRect: function () {
        return { x: 0, y: 0, width: 0, height: 0, top: 0, left: 0, right: 0, bottom: 0 };
      },
      cloneNode: function () { return mkElement(tag); },
      contains: function () { return false; },
      focus: function () {},
      blur: function () {},
      click: function () {},
      remove: function () {},
    });
    return el;
  }

  // ---- document ------------------------------------------------------------
  var html = mkElement('html');
  var head = mkElement('head');
  var body = mkElement('body');
  html.appendChild(head);
  html.appendChild(body);

  var document = EventTargetMix({
    nodeType: 9,
    documentElement: html,
    head: head,
    body: body,
    title: 'Just a moment...',
    readyState: 'loading',
    characterSet: 'UTF-8',
    charset: 'UTF-8',
    contentType: 'text/html',
    compatMode: 'CSS1Compat',
    visibilityState: 'visible',
    hidden: false,
    referrer: '',
    cookie: '',
    createElement: function (t) { var e = mkElement(t); e.ownerDocument = document; return e; },
    createElementNS: function (ns, t) { return document.createElement(t); },
    createTextNode: function (s) { return { nodeType: 3, textContent: String(s), data: String(s) }; },
    createDocumentFragment: function () { return mkElement('#fragment'); },
    getElementById: function () { return null; },
    getElementsByTagName: function (t) {
      t = String(t).toLowerCase();
      if (t === 'head') return [head];
      if (t === 'body') return [body];
      if (t === 'html') return [html];
      return [];
    },
    getElementsByClassName: function () { return []; },
    querySelector: function () { return null; },
    querySelectorAll: function () { return []; },
    addEventListener: function (type, fn) { (listeners[type] = listeners[type] || []).push(fn); },
    write: function () {},
    open: function () {},
    close: function () {},
  });
  document.location = null; // wired after location exists
  // the VM's own <script> element — challenge code reaches for it
  var selfScript = mkElement('script');
  selfScript.nonce = cfg.nonce || '';
  selfScript.readyState = 'complete';
  head.appendChild(selfScript);
  document.currentScript = selfScript;
  document.scripts = [selfScript];

  // ---- navigator -----------------------------------------------------------
  var navigator = {
    userAgent: cfg.userAgent,
    appVersion: cfg.userAgent.replace(/^Mozilla\//, ''),
    appName: 'Netscape',
    appCodeName: 'Mozilla',
    product: 'Gecko',
    productSub: '20030107',
    vendor: 'Google Inc.',
    vendorSub: '',
    platform: cfg.platform,
    oscpu: undefined,
    language: cfg.language,
    languages: cfg.languages,
    onLine: true,
    cookieEnabled: true,
    doNotTrack: null,
    hardwareConcurrency: cfg.hardwareConcurrency,
    deviceMemory: cfg.deviceMemory,
    maxTouchPoints: 0,
    pdfViewerEnabled: true,
    webdriver: false,
    plugins: { length: 0, item: function () { return null; }, namedItem: function () { return null; } },
    mimeTypes: { length: 0, item: function () { return null; }, namedItem: function () { return null; } },
    javaEnabled: function () { return false; },
    sendBeacon: function () { return true; },
    permissions: { query: function () { return Promise.resolve({ state: 'prompt' }); } },
  };

  // ---- location ------------------------------------------------------------
  var location = {
    href: cfg.href,
    protocol: 'https:',
    host: cfg.host,
    hostname: cfg.host,
    port: '',
    pathname: cfg.pathname,
    search: cfg.search || '',
    hash: '',
    origin: 'https://' + cfg.host,
    reload: function () { LOG.net.push({ kind: 'reload' }); },
    replace: function (u) { LOG.net.push({ kind: 'replace', url: String(u) }); },
    assign: function (u) { LOG.net.push({ kind: 'assign', url: String(u) }); },
    toString: function () { return cfg.href; },
  };
  document.location = location;

  // ---- screen / timing -----------------------------------------------------
  var screen = {
    width: cfg.screenW, height: cfg.screenH,
    availWidth: cfg.screenW, availHeight: cfg.screenH - 48,
    availLeft: 0, availTop: 0,
    colorDepth: 24, pixelDepth: 24,
    orientation: { angle: 0, type: 'landscape-primary' },
  };

  var t0 = cfg.now;
  var perf = {
    timeOrigin: t0,
    now: function () { return cfg.monotonic(); },
    timing: {
      navigationStart: t0, fetchStart: t0 + 1, domainLookupStart: t0 + 2,
      domainLookupEnd: t0 + 3, connectStart: t0 + 3, connectEnd: t0 + 20,
      secureConnectionStart: t0 + 8, requestStart: t0 + 21, responseStart: t0 + 60,
      responseEnd: t0 + 62, domLoading: t0 + 65, domInteractive: t0 + 90,
      domContentLoadedEventStart: t0 + 91, domContentLoadedEventEnd: t0 + 92,
      domComplete: t0 + 120, loadEventStart: t0 + 121, loadEventEnd: t0 + 122,
    },
    navigation: { type: 0, redirectCount: 0 },
    getEntriesByType: function () { return []; },
    getEntriesByName: function () { return []; },
    mark: function () {}, measure: function () {},
  };

  // ---- timers --------------------------------------------------------------
  // Binary min-heap ordered by (at, id). It used to be a plain array that
  // `pump` re-`sort`ed on every single tick and then `shift`ed — O(n log n) per
  // timer plus an O(n) memmove, i.e. quadratic in the number of timers. A page
  // that registers 20 000 of them took **31 seconds** to pump; in production
  // the 1.5 s budget cuts that off and the page returns NOTHING, so a merely
  // timer-heavy page became a total loss. Same defect as the cycle guard and
  // the containment scan, in a third place.
  //
  // The `id` tiebreak is load-bearing: the old `sort` was stable, so timers due
  // at the same instant fired in registration order, and page setup code
  // depends on that.
  var timers = [];
  var timerSeq = 1;
  var cancelled = null;

  function timerBefore(a, b) { return a.at !== b.at ? a.at < b.at : a.id < b.id; }

  function heapPush(t) {
    timers.push(t);
    var i = timers.length - 1;
    while (i > 0) {
      var p = (i - 1) >> 1;
      if (!timerBefore(timers[i], timers[p])) break;
      var tmp = timers[p]; timers[p] = timers[i]; timers[i] = tmp;
      i = p;
    }
  }

  function heapPop() {
    var top = timers[0];
    var last = timers.pop();
    if (timers.length) {
      timers[0] = last;
      var i = 0;
      for (;;) {
        var l = i * 2 + 1;
        var r = l + 1;
        var m = i;
        if (l < timers.length && timerBefore(timers[l], timers[m])) m = l;
        if (r < timers.length && timerBefore(timers[r], timers[m])) m = r;
        if (m === i) break;
        var sw = timers[m]; timers[m] = timers[i]; timers[i] = sw;
        i = m;
      }
    }
    return top;
  }

  function setTimeout_(fn, ms) {
    var id = timerSeq++;
    heapPush({ id: id, fn: fn, at: cfg.monotonic() + (ms || 0), args: [].slice.call(arguments, 2) });
    return id;
  }
  // Lazy cancellation: removing from the middle of a heap is fiddly and
  // clearTimeout is called far more often than the timer is reached.
  function clearTimeout_(id) {
    (cancelled || (cancelled = new Set())).add(id);
  }

  // Cooperative pump: runs due timers in time order. The challenge VM is
  // timer-driven, so this is what actually advances it to completion.
  function pump(maxTicks) {
    var ticks = 0;
    var cap = maxTicks || 5000;
    while (timers.length && ticks < cap) {
      var t = heapPop();
      // A cancelled timer never ran before, so it must not consume a tick now.
      if (cancelled && cancelled.has(t.id)) continue;
      ticks++;
      cfg.advance(Math.max(0, t.at - cfg.monotonic()));
      try { t.fn.apply(null, t.args); } catch (e) { LOG.errors.push('timer: ' + (e && e.stack || e)); }
    }
    return ticks;
  }

  function fire(type, ev) {
    var ls = listeners[type] || [];
    for (var i = 0; i < ls.length; i++) {
      try { ls[i].call(window, ev || { type: type }); } catch (e) { LOG.errors.push(type + ': ' + (e && e.stack || e)); }
    }
  }

  // ---- network (recorded, driven by the host) ------------------------------
  function XMLHttpRequest_() {
    this.readyState = 0;
    this.status = 0;
    this.responseText = '';
    this.response = '';
    this._headers = {};
  }
  XMLHttpRequest_.prototype = {
    open: function (m, u, async) { this._m = m; this._u = String(u); this.readyState = 1; },
    setRequestHeader: function (k, v) { this._headers[k] = String(v); },
    getResponseHeader: function () { return null; },
    getAllResponseHeaders: function () { return ''; },
    overrideMimeType: function () {},
    abort: function () {},
    send: function (b) {
      var self = this;
      var req = { kind: 'xhr', method: self._m, url: self._u, headers: self._headers, body: b == null ? null : String(b) };
      LOG.net.push(req);
      var res = cfg.request(req); // host-provided, synchronous
      self.status = res.status;
      self.responseText = res.body;
      self.response = res.body;
      self.readyState = 4;
      if (self.onreadystatechange) { try { self.onreadystatechange(); } catch (e) { LOG.errors.push('xhr rsc: ' + e); } }
      if (self.onload) { try { self.onload(); } catch (e) { LOG.errors.push('xhr load: ' + e); } }
    },
  };

  function fetch_(u, opts) {
    opts = opts || {};
    var req = { kind: 'fetch', method: opts.method || 'GET', url: String(u), headers: opts.headers || {}, body: opts.body == null ? null : String(opts.body) };
    LOG.net.push(req);
    var res = cfg.request(req);
    return Promise.resolve({
      ok: res.status >= 200 && res.status < 300,
      status: res.status,
      headers: { get: function () { return null; } },
      text: function () { return Promise.resolve(res.body); },
      json: function () { return Promise.resolve(JSON.parse(res.body)); },
      arrayBuffer: function () { return Promise.resolve(new ArrayBuffer(0)); },
    });
  }

  // ---- window --------------------------------------------------------------
  var window = EventTargetMix({
    document: document,
    navigator: navigator,
    location: location,
    screen: screen,
    performance: perf,
    history: {
      length: 1, scrollRestoration: 'auto', state: null,
      replaceState: function () {}, pushState: function () {},
      back: function () {}, forward: function () {}, go: function () {},
    },
    innerWidth: cfg.screenW, innerHeight: cfg.screenH - 139,
    outerWidth: cfg.screenW, outerHeight: cfg.screenH - 48,
    screenX: 0, screenY: 0, screenLeft: 0, screenTop: 0,
    pageXOffset: 0, pageYOffset: 0, scrollX: 0, scrollY: 0,
    devicePixelRatio: 1,
    isSecureContext: true,
    origin: 'https://' + cfg.host,
    name: '',
    closed: false,
    setTimeout: setTimeout_,
    clearTimeout: clearTimeout_,
    setInterval: setTimeout_,
    clearInterval: clearTimeout_,
    requestAnimationFrame: function (fn) { return setTimeout_(function () { fn(cfg.monotonic()); }, 16); },
    cancelAnimationFrame: clearTimeout_,
    queueMicrotask: function (fn) { Promise.resolve().then(fn); },
    XMLHttpRequest: XMLHttpRequest_,
    fetch: fetch_,
    matchMedia: function (q) {
      return { matches: false, media: String(q), addListener: function () {}, removeListener: function () {}, addEventListener: function () {}, removeEventListener: function () {} };
    },
    getComputedStyle: function () {
      return { getPropertyValue: function () { return ''; }, length: 0 };
    },
    btoa: cfg.btoa,
    atob: cfg.atob,
    crypto: cfg.crypto,
    localStorage: mkStorage(),
    sessionStorage: mkStorage(),
    postMessage: function (m) { LOG.net.push({ kind: 'postMessage', data: String(m).slice(0, 200) }); },
    alert: function () {}, confirm: function () { return false; }, prompt: function () { return null; },
    open: function () { return null; },
    close: function () {},
    focus: function () {}, blur: function () {},
    scrollTo: function () {}, scrollBy: function () {},
  });
  // Observer/stream/event constructors. A page that calls `new MutationObserver`
  // and gets a TypeError loses the whole script — and with it whatever state
  // that script was going to publish. None of these need to DO anything: the
  // harvest reads state, it does not react to mutations. They only need to
  // exist and not throw.
  function Noop() {}
  Noop.prototype = {
    observe: function () {}, unobserve: function () {}, disconnect: function () {},
    takeRecords: function () { return []; },
  };
  window.MutationObserver = Noop;
  window.IntersectionObserver = Noop;
  window.ResizeObserver = Noop;
  window.PerformanceObserver = Noop;

  function EventCtor(type, init) {
    init = init || {};
    this.type = String(type);
    this.detail = init.detail;
    this.bubbles = !!init.bubbles;
    this.cancelable = !!init.cancelable;
    this.defaultPrevented = false;
  }
  EventCtor.prototype = {
    preventDefault: function () { this.defaultPrevented = true; },
    stopPropagation: function () {},
    stopImmediatePropagation: function () {},
  };
  window.Event = EventCtor;
  window.CustomEvent = EventCtor;

  function AbortSignalCtor() { this.aborted = false; this.reason = undefined; }
  AbortSignalCtor.prototype = EventTargetMix({});
  function AbortControllerCtor() { this.signal = new AbortSignalCtor(); }
  AbortControllerCtor.prototype.abort = function (r) {
    this.signal.aborted = true;
    this.signal.reason = r;
  };
  window.AbortController = AbortControllerCtor;
  window.AbortSignal = AbortSignalCtor;

  function ReadableStreamCtor() {}
  ReadableStreamCtor.prototype = {
    getReader: function () {
      return {
        read: function () { return Promise.resolve({ done: true, value: undefined }); },
        cancel: function () { return Promise.resolve(); },
        releaseLock: function () {},
      };
    },
    cancel: function () { return Promise.resolve(); },
    pipeTo: function () { return Promise.resolve(); },
    pipeThrough: function (t) { return t && t.readable; },
  };
  window.ReadableStream = ReadableStreamCtor;
  window.WritableStream = function () {};
  window.TransformStream = function () { this.readable = new ReadableStreamCtor(); this.writable = {}; };

  function HeadersCtor(init) {
    var m = {};
    this.append = function (k, v) { m[String(k).toLowerCase()] = String(v); };
    this.set = this.append;
    this.get = function (k) { var k2 = String(k).toLowerCase(); return k2 in m ? m[k2] : null; };
    this.has = function (k) { return String(k).toLowerCase() in m; };
    this['delete'] = function (k) { delete m[String(k).toLowerCase()]; };
    this.forEach = function (fn) { for (var k in m) fn(m[k], k, this); };
    if (init && typeof init === 'object') for (var k in init) this.append(k, init[k]);
  }
  window.Headers = HeadersCtor;
  window.FormData = function () {
    var m = [];
    this.append = function (k, v) { m.push([k, v]); };
    this.get = function (k) { for (var i = 0; i < m.length; i++) if (m[i][0] === k) return m[i][1]; return null; };
    this.getAll = function () { return []; };
    this.has = function (k) { return this.get(k) !== null; };
    this.forEach = function (fn) { for (var i = 0; i < m.length; i++) fn(m[i][1], m[i][0], this); };
  };
  window.Blob = function (parts) { this.size = 0; this.type = ''; this.parts = parts || []; };
  window.WebSocket = function () {
    this.readyState = 0;
    this.send = function () {};
    this.close = function () {};
    this.addEventListener = function () {};
    this.removeEventListener = function () {};
  };
  window.EventSource = window.WebSocket;
  window.Worker = function () {
    this.postMessage = function () {};
    this.terminate = function () {};
    this.addEventListener = function () {};
  };
  window.SharedWorker = window.Worker;
  window.IdleDeadline = function () {};
  window.requestIdleCallback = function (fn) { return setTimeout_(function () { fn({ didTimeout: false, timeRemaining: function () { return 0; } }); }, 1); };
  window.cancelIdleCallback = clearTimeout_;

  // QuickJS has no URL/URLSearchParams (Node does, which is why this only
  // surfaced once the probe ran on the real engine). Minimal but honest:
  // enough for the `new URL(x, base)` and `?a=b` parsing pages actually do.
  function URLSearchParamsCtor(init) {
    var pairs = [];
    if (typeof init === 'string') {
      var q = init.charAt(0) === '?' ? init.slice(1) : init;
      if (q) {
        var parts = q.split('&');
        for (var i = 0; i < parts.length; i++) {
          if (!parts[i]) continue;
          var eq = parts[i].indexOf('=');
          var k = eq < 0 ? parts[i] : parts[i].slice(0, eq);
          var v = eq < 0 ? '' : parts[i].slice(eq + 1);
          try { k = decodeURIComponent(k.replace(/\+/g, ' ')); } catch (e) { /* keep raw */ }
          try { v = decodeURIComponent(v.replace(/\+/g, ' ')); } catch (e) { /* keep raw */ }
          pairs.push([k, v]);
        }
      }
    } else if (init && typeof init === 'object') {
      for (var kk in init) pairs.push([kk, String(init[kk])]);
    }
    this.get = function (k) { for (var i = 0; i < pairs.length; i++) if (pairs[i][0] === k) return pairs[i][1]; return null; };
    this.getAll = function (k) { var o = []; for (var i = 0; i < pairs.length; i++) if (pairs[i][0] === k) o.push(pairs[i][1]); return o; };
    this.has = function (k) { return this.get(k) !== null; };
    this.set = function (k, v) { this['delete'](k); pairs.push([k, String(v)]); };
    this.append = function (k, v) { pairs.push([k, String(v)]); };
    this['delete'] = function (k) { for (var i = pairs.length - 1; i >= 0; i--) if (pairs[i][0] === k) pairs.splice(i, 1); };
    this.forEach = function (fn) { for (var i = 0; i < pairs.length; i++) fn(pairs[i][1], pairs[i][0], this); };
    this.keys = function () { var o = []; for (var i = 0; i < pairs.length; i++) o.push(pairs[i][0]); return o; };
    this.toString = function () {
      var o = [];
      for (var i = 0; i < pairs.length; i++) {
        o.push(encodeURIComponent(pairs[i][0]) + '=' + encodeURIComponent(pairs[i][1]));
      }
      return o.join('&');
    };
  }

  function URLCtor(href, base) {
    href = String(href);
    // resolve against base for the relative forms pages actually use
    if (base !== undefined && !/^[a-zA-Z][a-zA-Z0-9+.\-]*:/.test(href)) {
      var b = String(base);
      var bm = /^([a-zA-Z][a-zA-Z0-9+.\-]*:)\/\/([^\/?#]*)([^?#]*)/.exec(b);
      if (bm) {
        if (href.charAt(0) === '/') {
          href = bm[1] + '//' + bm[2] + href;
        } else if (href.charAt(0) === '?' || href.charAt(0) === '#') {
          href = bm[1] + '//' + bm[2] + (bm[3] || '/') + href;
        } else {
          var dir = (bm[3] || '/').replace(/[^\/]*$/, '');
          href = bm[1] + '//' + bm[2] + dir + href;
        }
      }
    }
    var m = /^([a-zA-Z][a-zA-Z0-9+.\-]*:)\/\/([^\/?#]*)([^?#]*)(\?[^#]*)?(#.*)?$/.exec(href);
    if (!m) throw new TypeError('Invalid URL: ' + href);
    var hostport = m[2];
    var at = hostport.lastIndexOf('@');
    if (at >= 0) hostport = hostport.slice(at + 1);
    var colon = hostport.lastIndexOf(':');
    this.href = href;
    this.protocol = m[1];
    this.host = hostport;
    this.hostname = colon > 0 ? hostport.slice(0, colon) : hostport;
    this.port = colon > 0 ? hostport.slice(colon + 1) : '';
    this.pathname = m[3] || '/';
    this.search = m[4] || '';
    this.hash = m[5] || '';
    this.origin = m[1] + '//' + hostport;
    this.searchParams = new URLSearchParamsCtor(this.search);
    this.toString = function () { return this.href; };
  }
  window.URL = URLCtor;
  window.URLSearchParams = URLSearchParamsCtor;

  // Never provided at all until the failure tally named it. Pages log during
  // setup constantly, and `console.warn` on an undefined `console` kills the
  // script mid-way through publishing its state. Silent by design: the harvest
  // reads state, it does not want the page's log noise on our stdout.
  var noop = function () {};
  window.console = {
    log: noop, warn: noop, error: noop, info: noop, debug: noop, trace: noop,
    dir: noop, table: noop, group: noop, groupEnd: noop, groupCollapsed: noop,
    time: noop, timeEnd: noop, timeLog: noop, count: noop, assert: noop, clear: noop,
  };
  window.CSS = {
    supports: function () { return false; },
    escape: function (s) { return String(s).replace(/[^\w-]/g, '\\$&'); },
  };

  window.window = window;
  window.self = window;
  window.top = window;
  window.parent = window;
  window.frames = window;
  window.globalThis = window;
  window.length = 0;

  function mkStorage() {
    var m = Object.create(null);
    return {
      getItem: function (k) { return k in m ? m[k] : null; },
      setItem: function (k, v) { m[k] = String(v); },
      removeItem: function (k) { delete m[k]; },
      clear: function () { m = Object.create(null); },
      key: function () { return null; },
      get length() { return Object.keys(m).length; },
    };
  }

  if (cfg.diagnose) {
    window.document = rec('document', document);
    window.navigator = rec('navigator', navigator);
    window.screen = rec('screen', screen);
  }

  return {
    window: window,
    document: document,
    log: LOG,
    pump: pump,
    fire: fire,
    listeners: listeners,
    mkElement: mkElement,
  };
}

if (typeof module !== 'undefined' && module.exports) module.exports = { makeShim: makeShim };
