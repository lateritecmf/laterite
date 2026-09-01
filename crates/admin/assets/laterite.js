// Laterite admin: shared behaviour and the widget (island) lifecycle.

function latModeGlyph(m) {
  return m === 'light' ? '☀' : m === 'dark' ? '☾' : '◐';
}
function latCycleMode() {
  var o = localStorage.getItem('lat-mode') || 'auto';
  var n = o === 'light' ? 'dark' : o === 'dark' ? 'auto' : 'light';
  localStorage.setItem('lat-mode', n);
  var dark = n === 'dark' || (n === 'auto' && matchMedia('(prefers-color-scheme:dark)').matches);
  document.documentElement.setAttribute('data-theme', dark ? 'dark' : 'light');
  var e = document.getElementById('lat-mode-ico');
  if (e) e.textContent = latModeGlyph(n);
}
function latToggleMenu() {
  var m = document.getElementById('lat-menu');
  if (m) m.classList.toggle('is-open');
}
function latDismissFlash(btn) {
  var t = btn.closest('.lat-flash');
  if (!t) return;
  t.classList.add('is-leaving');
  setTimeout(function () { t.remove(); }, 180);
}

// Widget (island) registry: register an initialiser by name; every element with
// a matching data-lat-widget is initialised exactly once, on first load and
// after an htmx swap (swapped fragments carry their own widgets).
(function () {
  var registry = {};
  var lat = (window.lat = window.lat || {});
  lat.widget = function (name, init) {
    registry[name] = init;
    scan(document);
  };
  lat.assets = {
    // Idempotently load a stylesheet or script (for fragments whose assets are
    // not already on the page).
    ensure: function (url) {
      if (document.querySelector('[data-lat-asset="' + url + '"]')) return;
      var el;
      if (/\.css(\?|$)/.test(url)) {
        el = document.createElement('link');
        el.rel = 'stylesheet';
        el.href = url;
      } else {
        el = document.createElement('script');
        el.src = url;
        el.defer = true;
      }
      el.setAttribute('data-lat-asset', url);
      document.head.appendChild(el);
    }
  };
  function scan(root) {
    var scope = root && root.querySelectorAll ? root : document;
    scope.querySelectorAll('[data-lat-widget]:not([data-lat-ready])').forEach(function (el) {
      var init = registry[el.getAttribute('data-lat-widget')];
      if (init) {
        el.setAttribute('data-lat-ready', '1');
        init(el);
      }
    });
  }
  document.addEventListener('DOMContentLoaded', function () {
    scan(document);
    var e = document.getElementById('lat-mode-ico');
    if (e) e.textContent = latModeGlyph(localStorage.getItem('lat-mode') || 'auto');
  });
  document.addEventListener('htmx:load', function (ev) { scan(ev.target); });
})();

// Flash toasts: auto-dismiss non-error messages after a few seconds.
window.lat.widget('flash', function (el) {
  if (el.classList.contains('is-error')) return;
  setTimeout(function () { latDismissFlash(el); }, 5000);
});
