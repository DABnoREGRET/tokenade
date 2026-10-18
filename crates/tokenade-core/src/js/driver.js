// Engine-agnostic driver: runs a page's inline scripts in the shim and
// harvests the rendered data. Loaded after shim.js. Requires only `__host`
// (btoa/atob/random/now) from the embedder — no Node, no browser.

function readPage(html, targetUrl) {
  var u = parseUrl(targetUrl);
  var mono = 120.5;
  // Windows, matching the UA header the fetcher sends: a page whose scripts
  // read navigator.userAgent as Linux while the request announced Windows has
  // told the site more than either value alone would.
  var UA = 'Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36';

  // never report our own fingerprint back as page text
  setHostEcho([UA, UA.replace(/^Mozilla\//, ''), 'Win32', 'en-US', targetUrl,
    'https://' + u.host, 'Just a moment...']);

  var shim = makeShim({
    userAgent: UA,
    platform: 'Win32',
    language: 'en-US',
    languages: ['en-US', 'en'],
    hardwareConcurrency: 8,
    deviceMemory: 8,
    screenW: 1920,
    screenH: 1080,
    href: targetUrl,
    host: u.host,
    pathname: u.pathname,
    search: u.search,
    now: __host.now(),
    monotonic: function () { return mono; },
    advance: function (ms) { mono += ms > 0 ? ms : 0.1; },
    btoa: __host.btoa,
    atob: __host.atob,
    crypto: {
      getRandomValues: function (a) {
        for (var i = 0; i < a.length; i++) a[i] = (__host.random() * 256) | 0;
        return a;
      },
    },
    request: function () { return { status: 0, body: '' }; },
  });

  var win = shim.window;
  var BUILTINS = ['Object', 'Array', 'Function', 'Boolean', 'Symbol', 'Error', 'TypeError', 'RangeError',
    'SyntaxError', 'ReferenceError', 'EvalError', 'URIError', 'Number', 'Math', 'Date', 'String', 'RegExp',
    'Map', 'Set', 'WeakMap', 'WeakSet', 'ArrayBuffer', 'DataView', 'Uint8Array', 'Int8Array', 'Int16Array',
    'Uint16Array', 'Int32Array', 'Uint32Array', 'Float32Array', 'Float64Array', 'JSON', 'Promise',
    'Reflect', 'Proxy', 'BigInt', 'parseInt', 'parseFloat', 'isNaN', 'isFinite', 'decodeURI',
    'decodeURIComponent', 'encodeURI', 'encodeURIComponent', 'escape', 'unescape', 'NaN', 'Infinity', 'undefined'];
  for (var i = 0; i < BUILTINS.length; i++) {
    var b = BUILTINS[i];
    if (!(b in win) && typeof globalThis[b] !== 'undefined') win[b] = globalThis[b];
  }

  var scope = new Proxy(win, {
    has: function () { return true; },
    get: function (t, p) {
      if (typeof p === 'symbol') return p === Symbol.unscopables ? undefined : Reflect.get(t, p);
      return p in t ? Reflect.get(t, p) : undefined;
    },
    set: function (t, p, v) { return Reflect.set(t, p, v); },
  });

  // Snapshot the pristine surface: anything the page adds is what we harvest.
  var baseline = Object.keys(win);

  // --- execute inline scripts ---------------------------------------------
  var ran = 0, failed = 0, errs = [];
  var re = /<script([^>]*)>([\s\S]*?)<\/script>/g;
  var m;
  while ((m = re.exec(html)) !== null) {
    var attrs = m[1];
    if (/\bsrc\s*=/.test(attrs)) continue;
    // JSON-typed script tags are data, not code — feed them straight in
    var typeM = /\btype\s*=\s*["']?([^"'\s>]+)/.exec(attrs);
    var type = typeM ? typeM[1].toLowerCase() : '';
    var src = m[2];
    if (!src || !src.replace(/\s/g, '')) continue;
    if (type && type.indexOf('json') >= 0) {
      try { win['__jsonScript' + ran] = JSON.parse(src); ran++; } catch (e) { failed++; }
      continue;
    }
    if (type && type !== 'text/javascript' && type !== 'application/javascript' && type !== 'module') continue;
    try {
      new Function('__scope__', 'with(__scope__){' + src + '\n}')(scope);
      ran++;
    } catch (e) {
      failed++;
      // Keep the distinct reasons: a script that dies takes its slice of page
      // state with it, and the message names the shim gap that caused it.
      var msg = String((e && e.message) || e).slice(0, 120);
      if (errs.indexOf(msg) < 0 && errs.length < 12) errs.push(msg);
    }
  }

  shim.fire('DOMContentLoaded');
  shim.document.readyState = 'complete';
  shim.fire('load');
  shim.pump(5000);

  // --- harvest -------------------------------------------------------------
  var raw = harvestWindow(win, baseline);
  var lines = dedupeProse(raw);

  // Diagnostic: under `__TRACE__` (rust-probe only) report which key path each
  // harvested line came from, BEFORE dedup reorders them.
  var tr = [];
  var paths = pathsTrace();
  if (paths) for (var t = 0; t < raw.length; t++) tr.push((paths[t] || '?') + '\t' + raw[t]);

  // `text` stays LAST: the Rust side finds it by scanning for `"text":"` and
  // takes everything after it, so anything appended below would be swallowed.
  return JSON.stringify({
    ran: ran,
    failed: failed,
    lines: lines.length,
    errors: errs,
    tr: tr,
    depth: depthStats(),
    text: lines.join('\n'),
  });
}

function parseUrl(url) {
  var m = /^https?:\/\/([^\/?#]+)([^?#]*)(\?[^#]*)?/.exec(url) || [];
  return { host: m[1] || '', pathname: m[2] || '/', search: m[3] || '' };
}
