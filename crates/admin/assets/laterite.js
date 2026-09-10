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

// Confirm dialog: a control carrying data-lat-confirm asks before it acts. A
// modal rather than window.confirm, because a native dialog blocks the page and
// cannot be styled or localized with the rest of the admin.
(function () {
  var pending = null;

  function box() {
    var el = document.getElementById('lat-confirm');
    if (el) return el;
    el = document.createElement('div');
    el.id = 'lat-confirm';
    el.className = 'lat-modal';
    el.setAttribute('role', 'dialog');
    el.setAttribute('aria-modal', 'true');
    el.innerHTML =
      '<div class="lat-modal__backdrop" data-lat-close></div>' +
      '<div class="lat-modal__panel">' +
      '<p class="lat-modal__text"></p>' +
      '<div class="lat-modal__actions">' +
      '<button type="button" class="lat-btn lat-btn--ghost" data-lat-close></button>' +
      '<button type="button" class="lat-btn lat-btn--danger" data-lat-go></button>' +
      '</div></div>';
    document.body.appendChild(el);
    el.addEventListener('click', function (e) {
      if (e.target.hasAttribute('data-lat-close')) close();
      if (e.target.hasAttribute('data-lat-go')) go();
    });
    return el;
  }

  function close() {
    pending = null;
    var el = document.getElementById('lat-confirm');
    if (el) el.classList.remove('is-open');
  }

  function go() {
    var el = pending;
    close();
    if (!el) return;
    // Marked so the second pass through the handler lets it through.
    el.setAttribute('data-lat-confirmed', '1');
    el.click();
    el.removeAttribute('data-lat-confirmed');
  }

  document.addEventListener('keydown', function (e) {
    if (e.key === 'Escape') close();
  });

  // Capture phase, so the click never reaches htmx or the form until confirmed.
  document.addEventListener('click', function (e) {
    var el = e.target.closest ? e.target.closest('[data-lat-confirm]') : null;
    if (!el || el.hasAttribute('data-lat-confirmed')) return;
    e.preventDefault();
    e.stopPropagation();
    pending = el;
    var modal = box();
    modal.querySelector('.lat-modal__text').textContent = el.getAttribute('data-lat-confirm');
    modal.querySelector('[data-lat-go]').textContent = el.textContent.trim() || 'OK';
    modal.querySelector('[data-lat-close]:not(.lat-modal__backdrop)').textContent =
      document.body.getAttribute('data-lat-cancel') || 'Cancel';
    modal.classList.add('is-open');
    modal.querySelector('[data-lat-go]').focus();
  }, true);
})();

// Flash toasts: auto-dismiss non-error messages after a few seconds.
window.lat.widget('flash', function (el) {
  if (el.classList.contains('is-error')) return;
  setTimeout(function () { latDismissFlash(el); }, 5000);
});

// Repeater: add and remove rows. Rows are renumbered after every change so the
// submitted indices read 0,1,2; the server sorts the indices it finds rather
// than counting, so this is tidiness, not correctness.
window.lat.widget('repeater', function (root) {
  var rows = root.querySelector('.lat-repeater__rows');
  var blank = root.querySelector('.lat-repeater__blank');
  var add = root.querySelector('.lat-repeater__add');
  if (!rows || !blank || !add) return;

  function renumber() {
    rows.querySelectorAll('.lat-repeater__row').forEach(function (row, index) {
      row.querySelectorAll('[name]').forEach(function (control) {
        control.name = control.name.replace(/\[[^\]]*\]/, '[' + index + ']');
      });
    });
  }

  add.addEventListener('click', function () {
    rows.appendChild(blank.content.cloneNode(true));
    renumber();
  });

  root.addEventListener('click', function (e) {
    if (!e.target.classList.contains('lat-repeater__remove')) return;
    var row = e.target.closest('.lat-repeater__row');
    if (row) {
      row.remove();
      renumber();
    }
  });
});

// Select-all checkbox in a list header: ticks every row box in its table. Bound
// by structure, and re-bound after a swap because the header comes back with it.
window.lat.widget('pick-all', function (box) {
  var table = box.closest('table');
  if (!table) return;
  box.addEventListener('change', function () {
    table.querySelectorAll('tbody input[type="checkbox"][name="id"]').forEach(function (row) {
      row.checked = box.checked;
    });
  });
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
