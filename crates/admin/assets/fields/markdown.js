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
      insert: function () {
        var b = root.querySelector('[data-md-cmd="insert"]');
        if (b) openInsert(b);
        return true;
      },
      image: function (v) {
        if (!picker) return true;
        picker.open(root.getAttribute('data-pick') + '?mode=image');
        return true;
      }
    };
    // The Insert menu: commands for this surface, then snippets, which go in
    // as the figure the site expands at render.
    var menuBox = null;
    var menuData = null;
    function propsDialog(def, done) {
      var dialog = document.createElement('dialog');
      dialog.className = 'lat-dialog';
      var form = document.createElement('form');
      form.method = 'dialog';
      form.innerHTML =
        '<div class="lat-dialog__head"><h2></h2><button type="button" class="lat-dialog__close" data-lat-dialog-close aria-label="' + word('cancel') + '">&times;</button></div>' +
        '<div class="lat-dialog__body" data-props></div>' +
        '<div class="lat-dialog__actions"><button type="button" class="lat-btn lat-btn--ghost" data-lat-dialog-close></button><button type="submit" class="lat-btn lat-btn--primary"></button></div>';
      form.querySelector('h2').textContent = def.name;
      form.querySelector('.lat-dialog__actions [data-lat-dialog-close]').textContent = word('cancel');
      form.querySelector('[type=submit]').textContent = word('insert-title');
      var body = form.querySelector('[data-props]');
      def.properties.forEach(function (prop, i) {
        var id = 'lat-md-prop-' + i;
        var field = document.createElement('div');
        field.className = 'lat-field';
        if (prop.kind === 'checkbox') {
          field.innerHTML = '<label class="lat-check"><input type="checkbox"> <span></span></label>';
          field.querySelector('input').name = prop.code;
          field.querySelector('input').checked = !!prop.default && prop.default !== '0';
          field.querySelector('span').textContent = prop.title;
        } else if (prop.kind === 'dropdown') {
          field.innerHTML = '<label class="lat-field__label" for="' + id + '"></label><select class="lat-input" id="' + id + '"></select>';
          field.querySelector('label').textContent = prop.title;
          var select = field.querySelector('select'); select.name = prop.code;
          (prop.options || []).forEach(function (pair) {
            var opt = document.createElement('option'); opt.value = pair[0]; opt.textContent = pair[1];
            if (String(prop.default) === pair[0]) opt.selected = true;
            select.appendChild(opt);
          });
        } else {
          field.innerHTML = '<label class="lat-field__label" for="' + id + '"></label><input class="lat-input" type="text" id="' + id + '">';
          field.querySelector('label').textContent = prop.title;
          field.querySelector('input').name = prop.code;
          field.querySelector('input').value = prop.default === undefined || prop.default === null ? '' : String(prop.default);
        }
        body.appendChild(field);
      });
      dialog.appendChild(form);
      document.body.appendChild(dialog);
      form.addEventListener('submit', function (e) {
        e.preventDefault();
        var values = {};
        def.properties.forEach(function (prop) {
          var el = form.elements[prop.code];
          if (el) values[prop.code] = prop.kind === 'checkbox' ? el.checked : el.value;
        });
        dialog.close(); dialog.remove();
        done(values);
      });
      dialog.addEventListener('close', function () { dialog.remove(); });
      dialog.showModal();
      var first = form.querySelector('input:not([type=checkbox]), select, input');
      if (first) first.focus();
    }
    function insertFigure(def, values) {
      var attrs = JSON.stringify(values).replace(/&/g, '&amp;').replace(/"/g, '&quot;');
      insertBlock(view, '<figure data-snippet="' + def.code + '" data-properties="' + attrs + '"></figure>');
    }
    function openInsert(anchor) {
      if (!menuBox) {
        menuBox = document.createElement('div');
        menuBox.className = 'lat-markdown__menu';
        menuBox.setAttribute('role', 'menu');
        root.appendChild(menuBox);
        document.addEventListener('click', function (e) {
          if (!menuBox.hidden && !menuBox.contains(e.target) && !anchor.contains(e.target)) menuBox.hidden = true;
        });
        menuBox.addEventListener('keydown', function (e) { if (e.key === 'Escape') { menuBox.hidden = true; anchor.focus(); } });
      }
      if (!menuBox.hidden) { menuBox.hidden = true; return; }
      var paint = function () {
        menuBox.innerHTML = '';
        var any = false;
        (menuData.commands || []).forEach(function (cmd) {
          any = true;
          var b = document.createElement('button');
          b.type = 'button'; b.className = 'lat-markdown__menuitem'; b.setAttribute('role', 'menuitem');
          b.textContent = cmd.label;
          b.addEventListener('click', function () { menuBox.hidden = true; if (cmd.text) insertBlock(view, cmd.text); });
          menuBox.appendChild(b);
        });
        if ((menuData.snippets || []).length) {
          any = true;
          var h = document.createElement('div'); h.className = 'lat-markdown__menuhead'; h.textContent = word('snippets');
          menuBox.appendChild(h);
          menuData.snippets.forEach(function (def) {
            var b = document.createElement('button');
            b.type = 'button'; b.className = 'lat-markdown__menuitem'; b.setAttribute('role', 'menuitem');
            b.textContent = def.name;
            if (def.description) b.title = def.description;
            b.addEventListener('click', function () {
              menuBox.hidden = true;
              if (def.properties.length) propsDialog(def, function (values) { insertFigure(def, values); });
              else insertFigure(def, {});
            });
            menuBox.appendChild(b);
          });
        }
        if (!any) {
          var none = document.createElement('div'); none.className = 'lat-markdown__menuhead'; none.textContent = word('nothing-to-insert');
          menuBox.appendChild(none);
        }
        menuBox.hidden = false;
        var first = menuBox.querySelector('button');
        if (first) first.focus();
      };
      if (menuData) { paint(); return; }
      fetch(root.getAttribute('data-menu'), { credentials: 'same-origin', headers: { 'X-Requested-With': 'fetch' } })
        .then(function (r) { return r.ok ? r.json() : { commands: [], snippets: [] }; })
        .catch(function () { return { commands: [], snippets: [] }; })
        .then(function (data) { menuData = data; paint(); });
    }

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
