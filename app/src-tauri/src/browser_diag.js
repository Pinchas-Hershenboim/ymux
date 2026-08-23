/* Phase 82.E: document-start diagnostic probe for the workspace Browser
 * child webview. Embedded into workspace_browser.rs with include_str!.
 *
 * WHY THIS EXISTS: on macOS a page renders in the in-app Browser but the
 * site's own JavaScript does not run, while the identical code path works
 * on Windows. Rule #17 means there is no local mac build to attach a
 * debugger to, so the diagnosis has to be baked into the binary and come
 * back through the unified logger.
 *
 * REPORT CHANNEL: `document.title = "ymux-diag:<base64 json>"`, read by
 * `on_document_title_changed` in workspace_browser.rs. It is the only
 * channel that behaves identically on both platforms -- wry hooks
 * add_DocumentTitleChanged on Windows and a KVO observer on the `title`
 * keypath on macOS. `fetch`/<img> cannot reach on_navigation (that is the
 * decidePolicyForNavigationAction delegate, frame navigations only), an
 * iframe beacon is silently dead on Windows (only NavigationStarting is
 * hooked, never FrameNavigationStarting), and `location.href` is a
 * main-frame navigation fired in the exact window we are trying to
 * measure. The original title is restored on the next task.
 *
 * STRICT ES5 ONLY. Modern syntax in this file is a parse error on old
 * WebKit -- which is one of the hypotheses under test, so a probe that
 * cannot parse would silently manufacture the evidence for its own bug.
 * Feature detection goes through `new Function` inside try/catch.
 *
 * PRIVACY (Rule #1): lengths, counts, scheme + hostname. Never a path, a
 * query string, or page markup. Strings are stripped to printable ASCII
 * and truncated. The Rust side caps how many beacons it will log.
 */
(function () {
  'use strict';
  if (window.__ymuxDiag) { return; }
  window.__ymuxDiag = { v: 1 };

  var PREFIX = 'ymux-diag:';
  var MAX_STR = 160;
  var MAX_ERRS = 3;
  var MAX_SCRIPTS = 5;

  var queue = [];
  var firing = false;
  var errs = [];
  var t0 = (new Date()).getTime();
  var reported = false;

  var g0 = -1;
  try { g0 = Object.getOwnPropertyNames(window).length; } catch (e) {}

  function ascii(v, n) {
    var s;
    try { s = String(v === null || v === undefined ? '' : v); } catch (e) { s = '?'; }
    s = s.replace(/[^\x20-\x7e]/g, '?');
    if (s.length > n) { s = s.slice(0, n) + '~'; }
    return s;
  }

  /* One beacon per task turn. WebKit delivers title changes to the UI
   * process asynchronously, so two assignments inside one task can
   * coalesce into a single notification carrying only the last value. */
  function pump() {
    if (firing || !queue.length) { return; }
    /* At AtDocumentStart the document exists but <head> may not, and the
     * document.title setter is a no-op with no head element. Wait for it
     * rather than losing the boot beacon. */
    if (!document.head) {
      firing = true;
      setTimeout(function () { firing = false; pump(); }, 20);
      return;
    }
    firing = true;
    var payload = queue.shift();
    var orig;
    try { orig = document.title; } catch (e) { orig = ''; }
    try { document.title = PREFIX + payload; } catch (e) {}
    setTimeout(function () {
      try {
        if (String(document.title).indexOf(PREFIX) === 0) { document.title = orig; }
      } catch (e) {}
      firing = false;
      pump();
    }, 60);
  }

  function send(obj) {
    var s;
    try {
      obj.ms = (new Date()).getTime() - t0;
      s = btoa(JSON.stringify(obj));
    } catch (e) { return; }
    queue.push(s);
    pump();
  }

  /* Passive listeners in the capture phase -- never `window.onerror = `,
   * which would clobber a handler the page installed. */
  try {
    window.addEventListener('error', function (e) {
      if (errs.length >= MAX_ERRS) { return; }
      errs.push({
        m: ascii(e && e.message, MAX_STR),
        s: ascii(e && e.filename, 120),
        l: (e && e.lineno) | 0,
        c: (e && e.colno) | 0
      });
    }, true);
    window.addEventListener('unhandledrejection', function (e) {
      if (errs.length >= MAX_ERRS) { return; }
      errs.push({ m: 'rej: ' + ascii(e && e.reason, MAX_STR), s: '', l: 0, c: 0 });
    });
  } catch (e) {}

  function loopback(h) {
    return (h === '127.0.0.1' || h === 'localhost' || h === '::1' || h === '[::1]') ? 1 : 0;
  }

  /* ---- phase 1: boot ------------------------------------------------ */
  var here = { sc: '?', h: '?', lb: 0 };
  try {
    here.sc = ascii(location.protocol, 12);
    here.h = ascii(location.hostname, 60);
    here.lb = loopback(location.hostname);
  } catch (e) {}
  send({
    p: 'boot',
    ua: ascii(navigator.userAgent, 180),
    rs: ascii(document.readyState, 20),
    g0: g0,
    loc: here
  });

  /* ---- phase 2: engine capability ----------------------------------- */
  function synOK(src) { try { new Function(src); return 1; } catch (e) { return 0; } }
  function typ(get) { try { return ascii(get(), 12); } catch (e) { return 'throw'; } }
  send({
    p: 'feat',
    withResolvers: typ(function () { return typeof Promise.withResolvers; }),
    structuredClone: typ(function () { return typeof structuredClone; }),
    objectHasOwn: typ(function () { return typeof Object.hasOwn; }),
    arrayAt: typ(function () { return typeof [].at; }),
    fetchFn: typ(function () { return typeof fetch; }),
    synOptChain: synOK('var a; a?.b'),
    synLogAssign: synOK('var a = 1; a ||= 2'),
    synStaticBlock: synOK('class C { static {} }'),
    synPrivIn: synOK('class C { #x; static t(o) { return #x in o } }'),
    synAsync: synOK('return (async function () { await 0 })()')
  });

  /* ---- phase 3: the page's scripts ---------------------------------- */
  function probeFetch(u, host, sameOrigin) {
    if (typeof fetch !== 'function') { return; }
    var opts = sameOrigin ? undefined : { mode: 'no-cors' };
    try {
      fetch(u, opts).then(function (r) {
        send({
          p: 'fetch', h: host, so: sameOrigin ? 1 : 0,
          st: r.status | 0, ty: ascii(r.type, 20),
          ct: ascii(r.headers && r.headers.get ? r.headers.get('content-type') : '', 60)
        });
        try { if (r.body && r.body.cancel) { r.body.cancel(); } } catch (e) {}
      }, function (err) {
        /* An ATS block, a refused connection, or a TLS failure all land
         * here -- including for a no-cors request, which is what makes
         * this usable for cross-origin script sources. */
        send({
          p: 'fetch', h: host, so: sameOrigin ? 1 : 0, st: -1,
          err: ascii(err && (err.name + ': ' + err.message), 90)
        });
      });
    } catch (e) {}
  }

  function scanScripts() {
    var all, i, el, u, same, ext = [], inline = 0;
    try { all = [].slice.call(document.scripts); } catch (e) { all = []; }
    for (i = 0; i < all.length; i++) {
      el = all[i];
      if (!el.src) { inline++; continue; }
      if (ext.length >= MAX_SCRIPTS) { continue; }
      u = null;
      try { u = new URL(el.src, location.href); } catch (e) {}
      if (!u) { continue; }
      same = false;
      try {
        same = (u.protocol + '//' + u.host) === (location.protocol + '//' + location.host);
      } catch (e) {}
      ext.push({
        sc: ascii(u.protocol, 12),
        h: ascii(u.hostname, 60),
        lb: loopback(u.hostname),
        so: same ? 1 : 0,
        t: ascii(el.type || '', 30),
        df: el.defer ? 1 : 0,
        as: el.async ? 1 : 0
      });
      probeFetch(el.src, ascii(u.hostname, 60), same);
    }
    send({ p: 'scripts', n: all.length, inline: inline, ext: ext });
  }

  /* ---- phase 4: did anything actually execute? ---------------------- */
  function report(why) {
    if (reported) { return; }
    reported = true;
    var g1 = -1;
    try { g1 = Object.getOwnPropertyNames(window).length; } catch (e) {}
    var bodyLen = -1;
    try { bodyLen = document.body ? document.body.innerHTML.length : -1; } catch (e) {}
    var res = { n: 0, empty: 0 };
    try {
      var entries = performance.getEntriesByType('resource');
      for (var i = 0; i < entries.length; i++) {
        if (entries[i].initiatorType !== 'script') { continue; }
        res.n++;
        if (!entries[i].encodedBodySize) { res.empty++; }
      }
    } catch (e) {}
    send({
      p: 'report', why: why,
      /* gd is the decisive number: if the page's own scripts ran at all,
       * they defined globals. gd ~ 0 with n > 0 scripts means the site's
       * JS never executed. */
      g1: g1, gd: (g0 >= 0 && g1 >= 0) ? (g1 - g0) : -1,
      bodyLen: bodyLen, rs: ascii(document.readyState, 20),
      errN: errs.length, errs: errs, scriptRes: res
    });
  }

  try {
    if (document.readyState === 'loading') {
      document.addEventListener('DOMContentLoaded', scanScripts, false);
    } else {
      scanScripts();
    }
    window.addEventListener('load', function () {
      setTimeout(function () { report('load'); }, 1500);
    }, false);
  } catch (e) {}
  /* Fallback: `load` never fires if a subresource hangs, and the whole
   * point is getting a verdict off a machine we cannot attach to. */
  setTimeout(function () { report('timeout'); }, 9000);
})();
