// Sealed file: a secret file (a JSON or P12 key) read in the browser and sent
// inside the form as `{name, data}`, the data base64, to be sealed on save.
// The stored file is never sent back: the field holds the mask until a new
// file is chosen.
window.lat.widget('sealed-file', function (root) {
  var value = root.querySelector('[data-sealedfile-value]');
  var state = root.querySelector('[data-sealedfile-state]');
  var label = root.querySelector('[data-sealedfile-label]');
  var input = root.querySelector('input[type="file"]');
  if (!value || !state || !input) return;
  var max = parseInt(root.getAttribute('data-max'), 10) || 0;
  var accept = (root.getAttribute('data-accept') || '').split(',').filter(Boolean);
  function size(n) {
    if (n < 1024) return n + ' B';
    if (n < 1048576) return (n / 1024).toFixed(1) + ' KB';
    return (n / 1048576).toFixed(1) + ' MB';
  }
  function refuse(message) {
    input.value = '';
    window.lat.flash(message, 'error');
  }
  input.addEventListener('change', function () {
    var file = input.files && input.files[0];
    if (!file) return;
    var lower = file.name.toLowerCase();
    if (accept.length && !accept.some(function (ext) { return lower.slice(-ext.length) === ext; })) {
      refuse(root.getAttribute('data-wrong-type'));
      return;
    }
    if (max && file.size > max) { refuse(root.getAttribute('data-too-large')); return; }
    var reader = new FileReader();
    reader.onload = function () {
      var text = String(reader.result || '');
      value.value = JSON.stringify({ name: file.name, data: text.slice(text.indexOf(',') + 1) });
      state.textContent = '';
      var name = document.createElement('span');
      name.className = 'lat-sealedfile__name';
      name.textContent = file.name;
      var meta = document.createElement('span');
      meta.className = 'lat-sealedfile__meta';
      meta.textContent = size(file.size) + ' \u00b7 ' + root.getAttribute('data-pending');
      state.appendChild(name);
      state.appendChild(document.createTextNode(' '));
      state.appendChild(meta);
      if (label) label.textContent = root.getAttribute('data-replace');
      input.value = '';
      value.dispatchEvent(new Event('change', { bubbles: true }));
    };
    reader.readAsDataURL(file);
  });
});
