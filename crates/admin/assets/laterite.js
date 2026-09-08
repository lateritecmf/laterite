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

// htmx ignores a non-2xx response by default, so a form that failed validation
// would post and appear to do nothing. A 422 is our "here is the form again,
// with errors": let it swap, into the element that asked.
document.addEventListener('htmx:beforeSwap', function (e) {
  if (e.detail.xhr.status === 422) {
    e.detail.shouldSwap = true;
    e.detail.isError = false;
  }
});

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

// Raises a toast from script, matching the server-rendered flash markup so both
// look and dismiss the same.
window.lat.flash = function (text, level) {
  var box = document.querySelector('.lat-flashes');
  if (!box) {
    box = document.createElement('div');
    box.className = 'lat-flashes';
    box.setAttribute('role', 'status');
    box.setAttribute('aria-live', 'polite');
    document.body.insertBefore(box, document.body.firstChild);
  }
  var toast = document.createElement('div');
  toast.className = 'lat-flash is-' + (level || 'error');
  var label = document.createElement('span');
  label.className = 'lat-flash__text';
  label.textContent = text;
  var close = document.createElement('button');
  close.type = 'button';
  close.className = 'lat-flash__close';
  close.innerHTML = '&times;';
  close.addEventListener('click', function () { latDismissFlash(close); });
  toast.appendChild(label);
  toast.appendChild(close);
  box.appendChild(toast);
};

// A response htmx will not swap is otherwise swallowed, so a 500 or a dropped
// connection leaves the click with no outcome. The 422 above marks itself
// not-an-error, so a form's own validation errors never reach this.
function latRequestFailed() {
  window.lat.flash(document.body.getAttribute('data-lat-request-error') || 'Request failed.', 'error');
}
document.addEventListener('htmx:responseError', latRequestFailed);
document.addEventListener('htmx:sendError', latRequestFailed);

// Top progress bar: shown while any htmx request is in flight. The bar creeps
// toward the right while waiting, since the real duration is unknown.
(function () {
  var inflight = 0;
  var bar = null;
  var timer = null;
  function element() {
    if (!bar) {
      bar = document.createElement('div');
      bar.className = 'lat-progress';
      document.body.appendChild(bar);
    }
    return bar;
  }
  document.addEventListener('htmx:beforeRequest', function () {
    inflight++;
    clearTimeout(timer);
    var b = element();
    b.classList.remove('is-done');
    b.classList.add('is-active');
  });
  document.addEventListener('htmx:afterRequest', function () {
    inflight = Math.max(0, inflight - 1);
    if (inflight > 0) return;
    var b = element();
    b.classList.add('is-done');
    timer = setTimeout(function () { b.classList.remove('is-active', 'is-done'); }, 220);
  });
})();

// Flash toasts: auto-dismiss non-error messages after a few seconds.
window.lat.widget('flash', function (el) {
  if (el.classList.contains('is-error')) return;
  setTimeout(function () { latDismissFlash(el); }, 5000);
});

// Copy button: copies its input group's value, with brief confirmation. Binds by
// structure (closest group), so it survives repeater path-ids.
window.lat.widget('copy', function (btn) {
  var label = btn.textContent;
  btn.addEventListener('click', function () {
    var group = btn.closest('.lat-input-group');
    var input = group && group.querySelector('input');
    if (!input || !navigator.clipboard) return;
    navigator.clipboard.writeText(input.value).then(function () {
      btn.classList.add('is-copied');
      btn.textContent = 'Copied';
      setTimeout(function () {
        btn.classList.remove('is-copied');
        btn.textContent = label;
      }, 1200);
    });
  });
});
