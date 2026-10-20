// Markdown: the CodeMirror engine (vendor/codeeditor.js) in markdown mode over
// a textarea, a toolbar that writes markdown syntax around the selection, and
// a Preview tab rendered by the server, so what the author sees is what the
// site's own renderer makes of the text. The textarea stays the control the
// form submits and keeps every keystroke, so no script means a plain textarea.
//
// Markup: `<div data-lat-widget="markdown" data-preview="<url>" data-pick="<url>">`
// with `[data-md-tab]` tabs (write, preview), `[data-md-cmd]` buttons, a
// `[data-md-body]` the editor mounts into, a `[data-md-preview]` pane and the
// textarea. Words the island needs come as `data-w-<key>` attributes.
//
// Keyboard: Mod+B and Mod+I wrap the selection, Mod+K links it, Mod+Enter
// submits the form, Tab indents (Escape, then Tab, leaves the editor).
//
// Controller: `lat.get(el)` gives `{ get(), set(text), focus(), view }`.
// Events: `lat:markdown:ready` once mounted, `lat:markdown:previewed` after a
// preview renders.
(function () {
  var C = window.LateriteCode;
  if (!C || !window.lat) return;

  window.lat.widget('markdown', function (root) {
    var area = root.querySelector('textarea');
    var body = root.querySelector('[data-md-body]');
    var pane = root.querySelector('[data-md-preview]');
    if (!area || !body) return;
    var form = area.form;
    var readOnly = area.readOnly || area.disabled;
    var word = function (key) { return root.getAttribute('data-w-' + key) || key; };

    // Wraps the selection (or the word under the caret) in `before` and
    // `after`; with nothing to wrap, inserts a placeholder and selects it.
    function wrap(view, before, after, placeholder) {
      var state = view.state;
      var range = state.selection.main;
      var from = range.from, to = range.to;
      if (from === to) {
        var line = state.doc.lineAt(from);
        var text = line.text, at = from - line.from;
        var a = at, b = at;
        while (a > 0 && !/\s/.test(text[a - 1])) a--;
        while (b < text.length && !/\s/.test(text[b])) b++;
        from = line.from + a; to = line.from + b;
      }
      var inner = from === to ? placeholder : state.sliceDoc(from, to);
      view.dispatch({
        changes: { from: from, to: to, insert: before + inner + after },
        selection: { anchor: from + before.length, head: from + before.length + inner.length }
      });
      view.focus();
      return true;
    }
    // Prefixes every selected line (a heading, a quote, a list item).
    function prefixLines(view, prefix, numbered) {
      var state = view.state;
      var range = state.selection.main;
      var first = state.doc.lineAt(range.from), last = state.doc.lineAt(range.to);
      var changes = [];
      var n = 1;
      for (var pos = first.from; pos <= last.to;) {
        var line = state.doc.lineAt(pos);
        var p = numbered ? (n++) + '. ' : prefix;
        if (line.text.indexOf(p) === 0 && !numbered) changes.push({ from: line.from, to: line.from + p.length, insert: '' });
        else changes.push({ from: line.from, insert: p });
        if (line.to >= last.to) break;
        pos = line.to + 1;
      }
      view.dispatch({ changes: changes });
      view.focus();
      return true;
    }
    function insertBlock(view, text) {
      var range = view.state.selection.main;
      var line = view.state.doc.lineAt(range.from);
      var lead = line.text.length ? '\n\n' : '';
      view.dispatch({ changes: { from: line.to, insert: lead + text }, selection: { anchor: line.to + lead.length + text.length } });
      view.focus();
      return true;
    }
    var COMMANDS = {
      bold: function (v) { return wrap(v, '**', '**', word('bold-text')); },
      italic: function (v) { return wrap(v, '_', '_', word('italic-text')); },
      code: function (v) { return wrap(v, '`', '`', 'code'); },
      link: function (v) { return wrap(v, '[', '](https://)', word('link-text')); },
      heading: function (v) { return prefixLines(v, '## '); },
      quote: function (v) { return prefixLines(v, '> '); },
      bullet_list: function (v) { return prefixLines(v, '- '); },
      ordered_list: function (v) { return prefixLines(v, '', true); },
      table: function (v) { return insertBlock(v, '| ' + word('column') + ' | ' + word('column') + ' |\n| --- | --- |\n|  |  |'); },
      image: function (v) {
        if (!picker) return true;
        picker.open(root.getAttribute('data-pick') + '?mode=image');
        return true;
      }
    };
    var picker = window.lat.mediaPicker ? window.lat.mediaPicker({
      multiple: false,
      words: { title: word('library-title'), use: word('library-use'), cancel: word('cancel') },
      onDone: function (items) {
        var item = items[0];
        if (!item || !item.is_image) return;
        var range = view.state.selection.main;
        var text = '![' + item.name.replace(/[\[\]]/g, '') + '](' + item.url + ')';
        view.dispatch({ changes: { from: range.from, to: range.to, insert: text }, selection: { anchor: range.from + text.length } });
        view.focus();
      }
    }) : null;

    var ext = [
      C.highlightSpecialChars(), C.history(), C.drawSelection(), C.dropCursor(),
      C.highlightActiveLine(), C.highlightSelectionMatches(), C.indentUnit.of('  '),
      C.EditorView.lineWrapping,
      C.markdown({ base: C.markdownLanguage }),
      C.syntaxHighlighting(C.defaultHighlightStyle, { fallback: true }),
      C.keymap.of([
        { key: 'Mod-b', run: COMMANDS.bold },
        { key: 'Mod-i', run: COMMANDS.italic },
        { key: 'Mod-k', run: COMMANDS.link },
        { key: 'Mod-Enter', run: function () { if (form) form.requestSubmit(); return true; } }
      ]),
      C.keymap.of([].concat(C.markdownKeymap, C.defaultKeymap, C.historyKeymap, C.searchKeymap, [C.indentWithTab])),
      C.EditorState.readOnly.of(readOnly),
      C.EditorView.editable.of(!readOnly),
      C.EditorView.updateListener.of(function (u) {
        if (!u.docChanged) return;
        area.value = u.state.doc.toString();
        area.dispatchEvent(new Event('input', { bubbles: true }));
        stale = true;
      })
    ];
    if (area.placeholder) ext.push(C.placeholder(area.placeholder));
    var view = new C.EditorView({ state: C.EditorState.create({ doc: area.value, extensions: ext }), parent: body });
    area.hidden = true;

    // The toolbar colours come from the code editor's variables when its
    // stylesheet is on the page; markdown source reads fine without them.
    root.addEventListener('mousedown', function (e) {
      if (e.target.closest && e.target.closest('[data-md-cmd]')) e.preventDefault();
    });
    root.addEventListener('click', function (e) {
      if (!e.target.closest) return;
      var cmd = e.target.closest('[data-md-cmd]');
      if (cmd && COMMANDS[cmd.getAttribute('data-md-cmd')]) { COMMANDS[cmd.getAttribute('data-md-cmd')](view); return; }
      var tab = e.target.closest('[data-md-tab]');
      if (tab) showTab(tab.getAttribute('data-md-tab'));
    });

    // Preview: the server renders the text with the site's own renderer.
    var stale = true;
    function showTab(name) {
      root.querySelectorAll('[data-md-tab]').forEach(function (t) {
        var on = t.getAttribute('data-md-tab') === name;
        t.setAttribute('aria-selected', on ? 'true' : 'false');
        t.classList.toggle('is-on', on);
      });
      var previewing = name === 'preview';
      body.hidden = previewing;
      if (pane) pane.hidden = !previewing;
      root.classList.toggle('is-previewing', previewing);
      root.querySelectorAll('[data-md-cmd]').forEach(function (b) { b.disabled = previewing; });
      if (previewing) render(); else view.focus();
    }
    function render() {
      if (!pane) return;
      if (!stale) return;
      var url = root.getAttribute('data-preview');
      if (!url) return;
      var fd = new FormData();
      var token = form && form.querySelector('input[name="_csrf"]');
      fd.append('text', view.state.doc.toString());
      pane.setAttribute('aria-busy', 'true');
      fetch(url, {
        method: 'POST', body: fd, credentials: 'same-origin',
        headers: { 'X-Requested-With': 'fetch', 'X-CSRF-Token': token ? token.value : '' }
      })
        .then(function (r) { if (!r.ok) throw new Error(r.status); return r.text(); })
        .then(function (html) {
          pane.innerHTML = html || '<p class="lat-markdown__empty">' + word('nothing') + '</p>';
          stale = false;
          window.lat.emit(root, 'markdown:previewed', {});
        })
        .catch(function () { pane.textContent = word('preview-failed'); })
        .then(function () { pane.removeAttribute('aria-busy'); });
    }

    var label = area.id && document.querySelector('label[for="' + area.id + '"]');
    if (label) {
      label.id = label.id || area.id + '-label';
      view.contentDOM.setAttribute('aria-labelledby', label.id);
      label.addEventListener('click', function () { view.focus(); });
    }
    function set(text) { view.dispatch({ changes: { from: 0, to: view.state.doc.length, insert: text } }); }
    if (form) form.addEventListener('reset', function () { setTimeout(function () { set(area.defaultValue); }, 0); });
    root.classList.add('is-ready');
    window.lat.emit(root, 'markdown:ready', {});
    return {
      root: root,
      view: view,
      get: function () { return view.state.doc.toString(); },
      set: set,
      focus: function () { view.focus(); },
      preview: function () { showTab('preview'); }
    };
  });
})();
