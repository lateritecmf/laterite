// Laterite colour mode: light, dark, or auto.
//
// Inline this before any stylesheet. It runs blocking, on purpose: a deferred
// script runs after first paint, so a dark-mode reader would see a white flash.
//
// The contract it establishes, which is all a surface needs to know:
//
//   <html data-theme="light">  the resolved mode; style against this
//   localStorage["lat-mode"]   the reader's choice: light | dark | auto
//   window.latApplyMode(mode)  resolve and apply a mode now
//
// "auto" means follow the operating system, including when the system changes
// while the page is open: a page left open through dusk follows along instead of
// waiting for a reload. An explicit light or dark is the reader overriding the
// system and is left alone.
(function () {
  var mq = matchMedia('(prefers-color-scheme:dark)');
  window.latApplyMode = function (m) {
    var dark = m === 'dark' || (m === 'auto' && mq.matches);
    document.documentElement.setAttribute('data-theme', dark ? 'dark' : 'light');
  };
  var current = function () {
    try { return localStorage.getItem('lat-mode') || 'auto'; } catch (e) { return 'auto'; }
  };
  window.latSetMode = function (m) {
    try { localStorage.setItem('lat-mode', m); } catch (e) {}
    window.latApplyMode(m);
  };
  window.latMode = current;
  try { window.latApplyMode(current()); } catch (e) {}
  var onChange = function () {
    if (current() === 'auto') window.latApplyMode('auto');
  };
  if (mq.addEventListener) {
    mq.addEventListener('change', onChange);
  } else if (mq.addListener) {
    // Safari before 14 has only the deprecated form.
    mq.addListener(onChange);
  }
})();
