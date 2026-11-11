// Sensitive: a sealed secret, always shown masked. The eye shows and hides
// what the box holds: a stored value is fetched from `reveal/<field>` beside
// the form's own URL the first time; a value just typed is shown as it is.
// Typing into a stored value replaces it whole, and sending the mask back
// leaves it as it was. The value hides again when the tab is left.
window.lat.widget('sensitive', function (root) {
  var input = root.querySelector('input');
  var btn = root.querySelector('.lat-sensitive__btn');
  if (!input || !btn) return;
  // Untouched: the box still holds the mask standing in for the stored value.
  var untouched = root.hasAttribute('data-has-value');
  var shown = false;
  function show(on) {
    shown = on;
    input.type = on ? 'text' : 'password';
    btn.setAttribute('aria-pressed', on ? 'true' : 'false');
  }
  function revealUrl() {
    var path = location.pathname.replace(/\/edit\/?$/, '').replace(/\/$/, '');
    return path + '/reveal/' + encodeURIComponent(root.getAttribute('data-field'));
  }
  btn.addEventListener('click', function () {
    if (shown) { show(false); return; }
    if (!untouched) { show(true); return; }
    btn.classList.add('is-busy');
    fetch(revealUrl(), { credentials: 'same-origin', headers: { 'X-Requested-With': 'fetch' } })
      .then(function (r) { return r.ok ? r.text() : Promise.reject(r.status); })
      .then(function (text) {
        input.value = text;
        untouched = false;
        show(true);
        window.lat.emit(root, 'sensitive:revealed', { field: root.getAttribute('data-field') });
      })
      .catch(function () { window.lat.flash(root.getAttribute('data-failed') || 'Could not reveal the value.', 'error'); })
      .then(function () { btn.classList.remove('is-busy'); });
  });
  // The first edit of a stored value starts from empty, so the mask is never
  // mixed into what is typed or pasted.
  input.addEventListener('beforeinput', function () {
    if (untouched) { untouched = false; input.value = ''; }
  });
  document.addEventListener('visibilitychange', function () { if (document.hidden && shown) show(false); });
  if (input.form) input.form.addEventListener('submit', function () { if (shown) show(false); });
  return { hide: function () { show(false); } };
});
