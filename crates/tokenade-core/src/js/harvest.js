// Generic post-execution harvester.
//
// After the page's inline scripts have run inside the shim, the framework's
// preloaded state exists as live objects on `window`. This walks whatever
// appeared and pulls the prose out of it, without knowing the framework.
//
// It is deliberately shape-agnostic: Relay (Quora), Next.js RSC pushes,
// __NUXT__ factory calls, Apollo, Redux preloaded state and plain custom
// globals all end up as "some object graph hanging off window".
//
// Plain ES2020 — runs identically under QuickJS.

// Diagnostic only: when `__TRACE__` is set (rust-probe does, the product never
// does), remember which key path produced each emitted line. Round 7 needed it
// to ask "where does the junk come from" instead of guessing from the text.
var PATHS = null;
// Descent ceiling. The `seen` Set is what actually stops cycles; this is the
// backstop for a pathologically deep tree. It was 60 (walk) / 80 (flight),
// chosen without measurement: the deepest real state tree in the 63-page corpus
// nests **50** levels (vercel's flight stream), so 60 left 17 % headroom before
// silently truncating a page. Raised well clear of anything real — QuickJS's
// own 512 KB stack guard now stops runaway recursion with a catchable error
// long before this matters (see js_render::STACK_LIMIT).
var MAX_DEPTH = 400;
// Diagnostic counters, maintained only under __TRACE__ (PATHS non-null).
var DEPTH_CUT = 0;
var DEPTH_MAX = 0;
function depthStats() { return DEPTH_MAX + ':' + DEPTH_CUT; }
function emit(out, s, path) {
  out.push(s);
  if (PATHS) PATHS.push(path == null ? '?' : path);
}
function pathsTrace() { return PATHS; }
function sub(path, key) { return path === null ? null : path + '.' + key; }

function harvestWindow(win, baselineKeys) {
  if (typeof globalThis !== 'undefined' && globalThis.__TRACE__) PATHS = [];
  var out = [];
  var seenObj = new Set();
  var base = {};
  for (var i = 0; i < baselineKeys.length; i++) base[baselineKeys[i]] = 1;

  // Next.js App Router ships its payload as a React *flight* stream, not as
  // data: a blind walk of it yields className soup and <script> tags, which is
  // exactly the junk that made glassdoor look like a 17x win. Decode it as an
  // element tree and keep only text children.
  if (win.__next_f != null) decodeNextFlight(win.__next_f, out);

  // Named states first: when present they are the highest-signal payloads,
  // and visiting them early gives their text the better ordering.
  var NAMED = ['__NEXT_DATA__', '__NUXT__', '__APOLLO_STATE__', '__INITIAL_STATE__',
    '__PRELOADED_STATE__', '__remixContext', '__sveltekit'];
  var root = PATHS ? '' : null;
  for (var n = 0; n < NAMED.length; n++) {
    if (win[NAMED[n]] != null) walk(win[NAMED[n]], out, seenObj, 0, sub(root, NAMED[n]));
  }

  var keys;
  try { keys = Object.keys(win); } catch (e) { keys = []; }
  for (var k = 0; k < keys.length; k++) {
    var key = keys[k];
    if (base[key]) continue;
    if (key.charAt(0) === '_' && key.indexOf('__') !== 0 && key.length < 3) continue;
    var v;
    try { v = win[key]; } catch (e) { continue; }
    if (v == null || typeof v === 'function') continue;
    walk(v, out, seenObj, 0, sub(root, key));
  }
  return out;
}

function walk(node, out, seen, depth, path) {
  if (depth > MAX_DEPTH) { if (PATHS) DEPTH_CUT++; return; }
  if (node == null) return;
  if (PATHS && depth > DEPTH_MAX) DEPTH_MAX = depth;
  var t = typeof node;

  if (t === 'string') {
    var s = node;
    // a JSON blob hiding in a string: descend rather than emit
    var trimmed = s.replace(/^\s+|\s+$/g, '');
    if (trimmed.length > 2 && (trimmed.charAt(0) === '{' || trimmed.charAt(0) === '[')) {
      var parsed = null;
      try { parsed = JSON.parse(trimmed); } catch (e) { parsed = null; }
      if (parsed !== null && typeof parsed === 'object') { walk(parsed, out, seen, depth + 1, sub(path, '<json>')); return; }
    }
    s = decodeEntities(s);
    if (isProse(s)) emit(out, s, path);
    return;
  }

  if (t !== 'object') return;

  // Cycle guard. This was a linear scan of an array, which is O(n) per node and
  // so O(n²) over a state tree — on a 200 k-object payload that is tens of
  // millions of identity comparisons spent doing nothing. Set membership is
  // O(1) and QuickJS has had Set since forever.
  if (seen.has(node)) return;
  seen.add(node);

  // Classify the CONTAINER before descending into it. Arrays count: youtube
  // ships its UI labels as a list, not a dictionary, which is how 12 KB of
  // "Playlist deleted from downloads." survived the dictionary-only check.
  // A bundle contributes no prose — but it may still carry a few record values
  // among its labels, so descend into those rather than dropping the node.
  if (isStringBundle(node)) { walkRecordsOnly(node, out, seen, depth, path); return; }

  if (Object.prototype.toString.call(node) === '[object Array]') {
    for (var a = 0; a < node.length; a++) walk(node[a], out, seen, depth + 1, sub(path, '[]'));
    return;
  }

  // rich-text paragraph shape: {spans:[{text}]} (Quora, Notion-likes)
  if (Object.prototype.toString.call(node.spans) === '[object Array]') {
    var line = '';
    for (var j = 0; j < node.spans.length; j++) {
      var sp = node.spans[j];
      if (sp && typeof sp.text === 'string') line += sp.text;
    }
    line = decodeEntities(line);
    if (line.replace(/\s/g, '')) emit(out, line, sub(path, 'spans'));
    return;
  }

  var ks;
  try { ks = Object.keys(node); } catch (e) { return; }
  for (var q = 0; q < ks.length; q++) {
    var kk = ks[q];
    if (SKIP_KEY[kk]) continue;
    var vv;
    try { vv = node[kk]; } catch (e) { continue; }
    walk(vv, out, seen, depth + 1, sub(path, kk));
  }
}

// Keys whose values are never prose but are often huge — skipping them is
// what keeps the walk cheap on a 200 KB state tree.
var SKIP_KEY = {
  __typename: 1, id: 1, _id: 1, uuid: 1, hash: 1, sha: 1, etag: 1, css: 1, style: 1,
  styles: 1, className: 1, srcSet: 1, sizes: 1, viewBox: 1, d: 1, path: 1, svg: 1,
  token: 1, csrf: 1, jwt: 1, signature: 1, nonce: 1, key: 1, keys: 1, buildId: 1,
};

// The React flight format: numbered lines, each `<id>:<json>`. Elements are
// arrays ["$", tag, key, props]; the readable text lives in props.children as
// bare strings. Everything else (className, src, style) is layout, not content.
function decodeNextFlight(next_f, out) {
  var chunks = [];
  try {
    for (var i = 0; i < next_f.length; i++) {
      var e = next_f[i];
      if (typeof e === 'string') chunks.push(e);
      else if (e && typeof e[1] === 'string') chunks.push(e[1]);
    }
  } catch (err) { return; }

  var joined = chunks.join('');
  var re = /^([0-9a-f]+):/gm;
  var marks = [];
  var m;
  while ((m = re.exec(joined)) !== null) marks.push([m[1], m.index, m.index + m[0].length]);
  if (!marks.length) return;

  // Pass 1 — index every chunk by id. A chunk is either a JSON value or, when
  // it starts with `T<hexlen>,`, a raw TEXT payload. Text chunks are where
  // article bodies actually live; treating the stream as JSON-only drops them.
  var byId = {};
  var order = [];
  for (var s = 0; s < marks.length; s++) {
    var id = marks[s][0];
    var from = marks[s][2];
    var to = s + 1 < marks.length ? marks[s + 1][1] : joined.length;
    var body = joined.slice(from, to).replace(/\s+$/, '');
    if (!body) continue;

    if (body.charAt(0) === 'T') {
      var comma = body.indexOf(',');
      if (comma > 0 && /^[0-9a-f]+$/.test(body.slice(1, comma))) {
        byId[id] = { text: body.slice(comma + 1) };
        order.push(id);
        continue;
      }
    }
    var c0 = body.charAt(0);
    if (c0 !== '[' && c0 !== '{' && c0 !== '"') continue;
    try {
      byId[id] = { json: JSON.parse(body) };
      order.push(id);
    } catch (err) { /* partial/streamed chunk */ }
  }

  // Pass 2 — walk every chunk, resolving `$L<id>` / `$<id>` against the index.
  // `resolving` breaks the reference cycles the format allows.
  var resolving = {};
  var emitted = {};
  for (var o = 0; o < order.length; o++) {
    var entry = byId[order[o]];
    if (entry.text !== undefined) {
      var chunkText = decodeEntities(entry.text);
      if (!emitted[order[o]] && isProse(chunkText)) emit(out, chunkText, 'flight:' + order[o]);
      emitted[order[o]] = 1;
    } else {
      flightWalk(entry.json, out, 0, byId, resolving, PATHS ? 'flight:' + order[o] : null);
    }
  }
}

/// Resolve a flight scalar. Returns `undefined` for anything that is not a
/// reference we can follow.
function flightDeref(s, byId, resolving) {
  if (s.charAt(0) !== '$') return undefined;
  if (s.charAt(1) === '$') return undefined; // `$$` escapes a literal `$`
  var id = s.charAt(1) === 'L' ? s.slice(2) : s.slice(1);
  if (!/^[0-9a-f]+$/.test(id)) return undefined; // $undefined, $Sreact.suspense, $D…
  if (resolving[id]) return undefined;
  var entry = byId[id];
  if (!entry) return undefined;
  return entry;
}

function flightWalk(node, out, depth, byId, resolving, path) {
  if (depth > MAX_DEPTH) { if (PATHS) DEPTH_CUT++; return; }
  if (node == null) return;
  if (PATHS && depth > DEPTH_MAX) DEPTH_MAX = depth;

  if (typeof node === 'string') {
    if (node.charAt(0) === '$') {
      var entry = flightDeref(node, byId, resolving);
      if (!entry) return;
      var id = node.charAt(1) === 'L' ? node.slice(2) : node.slice(1);
      resolving[id] = 1;
      if (entry.text !== undefined) {
        var refText = decodeEntities(entry.text);
        if (isProse(refText)) emit(out, refText, sub(path, '$' + id));
      } else {
        flightWalk(entry.json, out, depth + 1, byId, resolving, sub(path, '$' + id));
      }
      resolving[id] = 0;
      return;
    }
    var plain = decodeEntities(node);
    if (isProse(plain)) emit(out, plain, path);
    return;
  }
  if (typeof node !== 'object') return;

  if (Object.prototype.toString.call(node) === '[object Array]') {
    // element: ["$", tag, key, props] — descend into props.children only
    if (node.length === 4 && node[0] === '$' && typeof node[1] === 'string') {
      var props = node[3];
      if (props && typeof props === 'object') {
        flightWalk(props.children, out, depth + 1, byId, resolving, sub(path, node[1]));
        if (typeof props.title === 'string') flightWalk(props.title, out, depth + 1, byId, resolving, sub(path, 'title'));
        if (typeof props.alt === 'string') flightWalk(props.alt, out, depth + 1, byId, resolving, sub(path, 'alt'));
        if (typeof props.value === 'string') flightWalk(props.value, out, depth + 1, byId, resolving, sub(path, 'value'));
      }
      return;
    }
    for (var i = 0; i < node.length; i++) flightWalk(node[i], out, depth + 1, byId, resolving, sub(path, '[]'));
    return;
  }

  // plain data object inside the stream — treat as data, keep its prose
  var bundle = isStringBundle(node);
  var ks = Object.keys(node);
  for (var q = 0; q < ks.length; q++) {
    if (SKIP_KEY[ks[q]]) continue;
    var v = node[ks[q]];
    // In a bundle the strings are labels; the records beside them are not.
    if (bundle && (v === null || typeof v !== 'object')) continue;
    flightWalk(v, out, depth + 1, byId, resolving, sub(path, ks[q]));
  }
}

// A UI/i18n bundle is a wide collection of SHORT strings with no record
// structure. Page content, even when its lines are short (a listing of rentals,
// a feed of posts), is objects with fields — a title next to a price next to a
// rating. That structural difference is the only one that holds: output shape
// does NOT separate them (youtube's bundle is 82% short lines, airbnb's genuine
// listings are 96%), and neither does title relevance (youtube matched 9/9
// title tokens, airbnb 5/9). So classify the container, never the text.
function isStringBundle(node) {
  var isArray = Object.prototype.toString.call(node) === '[object Array]';
  var vals = [];
  if (isArray) {
    if (node.length < 25) return false;
    vals = node;
  } else {
    var ks;
    try { ks = Object.keys(node); } catch (e) { return false; }
    if (ks.length < 25) return false;
    for (var i = 0; i < ks.length; i++) {
      try { vals.push(node[ks[i]]); } catch (e) { /* getter threw */ }
    }
  }

  var strings = 0;
  var objects = 0;
  var total = 0;
  var chars = 0;
  for (var j = 0; j < vals.length; j++) {
    var v = vals[j];
    if (v === undefined) continue;
    total++;
    if (typeof v === 'string') {
      strings++;
      chars += v.length;
    } else if (v !== null && typeof v === 'object') {
      objects++;
    }
  }
  if (!total || strings * 5 < total * 4) return false; // <80% plain strings
  // Records are where content lives — but "contains a record" is not the same
  // as "is records". youtube's `ytcfg.msgs` is 249 UI labels next to SIX plural
  // templates, and treating a single object as a veto let all 249 through as
  // page text. Enough records to be content means ~10%, not one.
  if (objects * 10 > total) return false;
  return chars / strings < 140; // short strings ⇒ labels, not prose
}

// Walk only the non-string values of a bundle. Keeps the "skip the labels"
// decision from also throwing away whatever records sit beside them.
function walkRecordsOnly(node, out, seen, depth, path) {
  var ks;
  try { ks = Object.keys(node); } catch (e) { return; }
  for (var i = 0; i < ks.length; i++) {
    var v;
    try { v = node[ks[i]]; } catch (e) { continue; }
    if (v !== null && typeof v === 'object') walk(v, out, seen, depth + 1, sub(path, ks[i]));
  }
}

// Values the shim itself supplied — echoing them back would be reporting our
// own fingerprint as page content.
var HOST_ECHO = null;
function setHostEcho(values) {
  HOST_ECHO = [];
  for (var i = 0; i < values.length; i++) if (values[i]) HOST_ECHO.push(String(values[i]));
}

// Substring, not equality: youtube echoes the user-agent with `,gzip(gfe)`
// appended, which an exact match sails straight past.
function hostEchoed(s) {
  for (var i = 0; i < HOST_ECHO.length; i++) {
    var v = HOST_ECHO[i];
    if (v.length >= 12 && s.indexOf(v) >= 0) return true;
    if (s === v) return true;
  }
  return false;
}

// Scripts that do not separate words with spaces. Every space-based test below
// has to stand down for these, or the reader silently discards the entire
// Chinese, Japanese, Thai, Khmer, Lao and Burmese web.
var SPACELESS = /[぀-ヿ㐀-䶿一-鿿豈-﫿฀-๿຀-໿ក-៿က-႟]/;

// Letter counting must cover every alphabet, not the handful that happened to
// be in the first corpus. `\p{L}` is the correct answer where the engine has
// it; the fallback enumerates the major scripts explicitly.
var LETTER_RE;
function letterCount(s) {
  if (LETTER_RE === undefined) {
    try {
      // `\p{M}` matters: in Devanagari, Thai and Arabic the vowel marks are
      // combining MARKS, not letters. Counting only `\p{L}` made Hindi look
      // like it was half punctuation and dropped it.
      LETTER_RE = new RegExp('[\\p{L}\\p{M}]', 'gu');
      if (!'أ'.match(LETTER_RE)) throw new Error('no unicode classes');
    } catch (e) {
      LETTER_RE = /[A-Za-zÀ-ÿĀ-ſЀ-ӿͰ-Ͽ԰-֏֐-׿؀-ۿ܀-ݏऀ-ॿঀ-৿਀-ൿ฀-๿຀-໿က-႟ក-៿぀-ヿ㐀-䶿一-鿿가-힯]/g;
    }
  }
  var m = s.match(LETTER_RE);
  return m ? m.length : 0;
}

// State blobs carry HTML-ESCAPED text: a page that renders `a & b` stores
// `a &amp; b`, and stackoverflow's answers store whole code samples as
// `(data[c] - 128) &gt;&gt; 31`. The markup path decodes entities as its last
// step; this path had no equivalent and emitted them raw.
//
// Decoded BEFORE `isProse`, not after, so the checks below see the real text:
// `&lt;p&gt;` has to look like the markup it is in order to be rejected.
// One regex pass, so `&amp;lt;` correctly yields `&lt;` and not `<`.
var NAMED_ENTITY = {
  amp: '&', lt: '<', gt: '>', quot: '"', apos: "'", nbsp: ' ',
  hellip: '…', mdash: '—', ndash: '–', middot: '·',
  lsquo: '‘', rsquo: '’', ldquo: '“', rdquo: '”',
  laquo: '«', raquo: '»', deg: '°', euro: '€',
  pound: '£', yen: '¥', copy: '©', reg: '®', trade: '™',
};
function decodeEntities(s) {
  if (s.indexOf('&') < 0) return s;
  return s.replace(/&(#\d{1,7}|#[xX][0-9a-fA-F]{1,6}|[a-zA-Z]{2,8});/g, function (whole, body) {
    if (body.charAt(0) === '#') {
      var code = body.charAt(1) === 'x' || body.charAt(1) === 'X'
        ? parseInt(body.slice(2), 16)
        : parseInt(body.slice(1), 10);
      // Lone surrogates and out-of-range values would corrupt the output.
      if (!(code > 0) || code > 0x10ffff || (code >= 0xd800 && code <= 0xdfff)) return whole;
      try { return String.fromCodePoint(code); } catch (e) { return whole; }
    }
    var named = NAMED_ENTITY[body];
    return named === undefined ? whole : named;
  });
}

function isProse(s) {
  if (HOST_ECHO && hostEchoed(s)) return false;
  var spaceless = SPACELESS.test(s);
  // Prose puts a space every ~6 characters. Config blobs, query strings,
  // base64 and minified data do not — youtube inlines 47 KB of
  // `flag_name=true&other_flag=1.0&…` that is otherwise letter-rich enough to
  // pass every other check here. Meaningless for spaceless scripts.
  if (!spaceless) {
    var spaces = 0;
    for (var si = 0; si < s.length; si++) if (s.charCodeAt(si) === 32) spaces++;
    if (spaces * 25 < s.length) return false;
  }
  // css function soup and date/number format patterns
  if (/^\s*(invert|sepia|saturate|hue-rotate|brightness|contrast|rgba?|hsla?|blur|drop-shadow)\s*\(/.test(s)) return false;
  if (/(%\)\s*){2,}/.test(s)) return false;
  if (/^[MDYHhmsAaZz\s,:\[\]()GMTPST.\-\/]+$/.test(s)) return false;
  // Runtime error templates. Every bundle carries DOMException/TypeError
  // message strings, and they are perfectly sentence-shaped, so nothing else
  // here catches them — vk.com opened with "Failed to execute 'send' on
  // 'WebSocket'" before its actual wall posts.
  if (/^(Uncaught |Unhandled )?(Type|Range|Reference|Syntax|Eval|URI|DOM)Error\b/.test(s)) return false;
  if (/Failed to (execute|construct|set|read|fetch)\s+'/.test(s)) return false;
  if (/Cannot (read|assign to|set|delete) .{0,40}propert(y|ies)/.test(s)) return false;
  if (/\bis not a (function|constructor|valid|object)\b/.test(s)) return false;
  if (/\b(is not defined|of undefined|of null)\b/.test(s)) return false;
  // GraphQL operation documents. Apps ship the query text itself in the
  // payload, and on producthunt two of them were 91 % of everything the reader
  // produced. They arrive as flight TEXT chunks — the same container article
  // bodies use — so there is no container to classify here and the container
  // rule genuinely cannot help. Matching the SYNTAX is safe in a way that
  // matching "junk-shaped text" never was: `fragment X on Y {` and
  // `query Name{` are formal grammar, not a guess about tone or length.
  if (/\bfragment\s+\w+\s+on\s+\w+\s*\{/.test(s)) return false;
  if (/^\s*(query|mutation|subscription)\s+\w*\s*[({]/.test(s)) return false;
  // i18n format templates ("%s · Get Started", "Welcome {0}") are strings the
  // page never shows as-is — nuxt.com yields hundreds of them.
  if (/(^|[\s·])%[sdf@]([\s·]|$)/.test(s)) return false;
  if (/\{\d+\}|\{\{\s*\w+\s*\}\}/.test(s)) return false;
  // CJK is dense: 25 characters of Chinese is a long sentence, not a fragment.
  if (s.length < (spaceless ? 8 : 25) || s.length > 100000) return false;
  if (!spaceless && s.indexOf(' ') < 0) return false;
  // urls, data uris, css, code-ish and encoded blobs are not prose
  if (/^(https?:|data:|\/\/|blob:)/.test(s)) return false;
  if (/[{}<>]\s*$/.test(s) && /[:;]/.test(s)) return false;
  if (/^[A-Za-z0-9+/=]{40,}$/.test(s)) return false;
  // serialized markup / bundler output that survived as one long string
  if (/\["\$"|"className"|_next\/static|"src":|<\/?[a-z]+[\s>]/.test(s)) return false;
  // JSON that did not parse: quote-dense, so not a sentence
  var quotes = s.split('"').length - 1;
  if (quotes * 20 > s.length) return false;
  var letters = letterCount(s);
  if (letters * 2 < s.length) return false; // mostly punctuation/digits
  // Word counting is a latin-script idea. For spaceless scripts the equivalent
  // floor is "enough characters to be a sentence".
  if (spaceless) return letters >= 8;
  var words = s.replace(/^\s+|\s+$/g, '').split(/\s+/).length;
  return words >= 4;
}

// Rolling-hash window index behind the containment test. ANCHOR is 8 because
// that is `isProse`'s shortest accepted line (a spaceless-script sentence);
// anything shorter cannot be indexed and falls through to the exact scan.
var ANCHOR = 8;
var ANCHOR_BASE = 131;

function indexAnchors(s, set) {
  if (s.length < ANCHOR) return;
  var h = 0;
  var pow = 1;
  for (var i = 0; i < ANCHOR; i++) {
    h = (Math.imul(h, ANCHOR_BASE) + s.charCodeAt(i)) | 0;
    if (i) pow = Math.imul(pow, ANCHOR_BASE) | 0;
  }
  set.add(h);
  for (var k = ANCHOR; k < s.length; k++) {
    h = (h - Math.imul(s.charCodeAt(k - ANCHOR), pow)) | 0;
    h = (Math.imul(h, ANCHOR_BASE) + s.charCodeAt(k)) | 0;
    set.add(h);
  }
}

/// Necessary condition for containment. `true` means "maybe" — the caller still
/// verifies. Strings too short to index answer `true` so they take the scan.
///
/// Probes several windows, not just the first: pages repeat prefixes (route
/// tables, "Read more about …"), so a single leading window let most candidates
/// through and the scan stayed the bottleneck. Every window of a contained
/// string is necessarily indexed, so ANY miss is a definite no.
function anchorHit(s, set) {
  if (s.length < ANCHOR) return true;
  var last = s.length - ANCHOR;
  for (var p = 0; p < 4; p++) {
    var at = ((last * p) / 3) | 0;
    var h = 0;
    for (var i = at; i < at + ANCHOR; i++) h = (Math.imul(h, ANCHOR_BASE) + s.charCodeAt(i)) | 0;
    if (!set.has(h)) return false;
    if (last === 0) break;
  }
  return true;
}

// Order-preserving dedup, plus containment: a string already present inside a
// longer kept one adds nothing.
function dedupeProse(lines) {
  var seen = {};
  var kept = [];
  for (var i = 0; i < lines.length; i++) {
    var s = lines[i].replace(/\s+/g, ' ').replace(/^\s+|\s+$/g, '');
    if (!s || seen[s]) continue;
    seen[s] = 1;
    kept.push(s);
  }
  kept.sort(function (a, b) { return b.length - a.length; });
  var out = [];
  var blob = '';
  // The containment test used to be `blob.indexOf(candidate)` against a blob
  // that grows to the size of the whole output — O(n·L), i.e. 400 M character
  // comparisons on nuxt.com/blog (3 011 lines, 263 KB) and 62 % of that page's
  // total run time, to remove 1.5 % of its text.
  //
  // Same answer, near-linear: index every ANCHOR-length window of the kept text
  // by rolling hash. If a candidate is contained in some kept line, its own
  // first window is necessarily one of those windows, so a miss is a definite
  // no and the scan is skipped. A hit (or a hash collision) falls through to
  // the exact `indexOf`, which stays the source of truth.
  //
  // A candidate can never span two kept lines: `blob` joins them with `\n` and
  // no candidate contains one.
  var anchors = new Set();
  for (var j = 0; j < kept.length; j++) {
    if (anchorHit(kept[j], anchors) && blob.indexOf(kept[j]) >= 0) continue;
    out.push(kept[j]);
    blob += kept[j] + '\n';
    indexAnchors(kept[j], anchors);
  }
  // restore document order
  var pos = {};
  for (var p = 0; p < lines.length; p++) {
    var key = lines[p].replace(/\s+/g, ' ').replace(/^\s+|\s+$/g, '');
    if (pos[key] === undefined) pos[key] = p;
  }
  out.sort(function (a, b) { return (pos[a] === undefined ? 1e9 : pos[a]) - (pos[b] === undefined ? 1e9 : pos[b]); });
  return out;
}
