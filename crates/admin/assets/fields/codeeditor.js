// Code editor: the CodeMirror engine (vendor/codeeditor.js) mounted over a
// textarea. The textarea stays the control the form submits and keeps every
// keystroke, so a page with no script, or one whose engine failed to load, is a
// plain editable textarea with the same name and value.
//
// Markup: `<div data-lat-widget="codeeditor" data-language="jinja">` around a
// `<textarea>`. `data-language` is one of text, html, css, javascript, json,
// yaml, markdown, jinja; `data-line-numbers="off"` drops the gutter;
// `data-wrap="on"` soft-wraps; `--rows` on the div sets the height in lines.
// A readonly or disabled textarea gives a read-only editor.
//
// Keyboard: Tab indents (Escape, then Tab, leaves the editor), Mod+Enter
// submits the form like any textarea, Mod+F searches, Mod+Z undoes.
//
// Controller: `lat.get(el)` gives `{ get(), set(text), focus(), view }`.
// Events: `lat:codeeditor:ready` once mounted.
(function () {
  var C = window.LateriteCode;
  if (!C || !window.lat) return;
  var LANGS = {
    html: C.html, css: C.css, javascript: C.javascript, json: C.json,
    yaml: C.yaml, markdown: C.markdown, jinja: C.jinja
  };
  // Token colours come from the stylesheet's variables, so light and dark are
  // one theme and a site restyles them like everything else.
  var t = C.tags;
  var colours = C.HighlightStyle.define([
    { tag: t.keyword, color: 'var(--code-keyword)' },
    { tag: [t.name, t.deleted, t.character, t.propertyName, t.macroName], color: 'var(--code-name)' },
    { tag: [t.function(t.variableName), t.labelName], color: 'var(--code-function)' },
    { tag: [t.color, t.constant(t.name), t.standard(t.name)], color: 'var(--code-constant)' },
    { tag: [t.definition(t.name), t.separator], color: 'var(--code-definition)' },
    { tag: [t.typeName, t.className, t.number, t.changed, t.annotation, t.modifier, t.self, t.namespace], color: 'var(--code-type)' },
    { tag: [t.operator, t.operatorKeyword, t.url, t.escape, t.regexp, t.special(t.string)], color: 'var(--code-operator)' },
    { tag: [t.meta, t.comment], color: 'var(--code-comment)', fontStyle: 'italic' },
    { tag: t.strong, fontWeight: 'bold' },
    { tag: t.emphasis, fontStyle: 'italic' },
    { tag: t.strikethrough, textDecoration: 'line-through' },
    { tag: t.link, color: 'var(--code-operator)', textDecoration: 'underline' },
    { tag: t.heading, fontWeight: 'bold', color: 'var(--code-heading)' },
    { tag: [t.atom, t.bool, t.special(t.variableName)], color: 'var(--code-atom)' },
    { tag: [t.processingInstruction, t.string, t.inserted, t.attributeValue], color: 'var(--code-string)' },
    { tag: t.invalid, color: 'var(--code-invalid)' },
    { tag: t.tagName, color: 'var(--code-tag)' },
    { tag: t.attributeName, color: 'var(--code-attribute)' }
  ]);

  window.lat.widget('codeeditor', function (el) {
    var area = el.querySelector('textarea');
    if (!area) return;
    var form = area.form;
    var readOnly = area.readOnly || area.disabled;
    var name = el.getAttribute('data-language') || 'text';
    var ext = [
      C.highlightSpecialChars(), C.history(), C.drawSelection(), C.dropCursor(),
      C.indentOnInput(), C.bracketMatching(), C.highlightActiveLine(),
      C.highlightSelectionMatches(), C.indentUnit.of('  '),
      C.syntaxHighlighting(colours),
      C.keymap.of([{ key: 'Mod-Enter', run: function () { if (form) form.requestSubmit(); return true; } }]),
      C.keymap.of([].concat(C.defaultKeymap, C.historyKeymap, C.searchKeymap, C.foldKeymap, [C.indentWithTab])),
      C.EditorState.readOnly.of(readOnly),
      C.EditorView.editable.of(!readOnly),
      C.EditorView.updateListener.of(function (u) {
        if (!u.docChanged) return;
        area.value = u.state.doc.toString();
        area.dispatchEvent(new Event('input', { bubbles: true }));
      })
    ];
    if (LANGS[name]) ext.push(LANGS[name]());
    if (el.getAttribute('data-line-numbers') !== 'off') ext.push(C.lineNumbers(), C.highlightActiveLineGutter(), C.foldGutter());
    if (el.getAttribute('data-wrap') === 'on') ext.push(C.EditorView.lineWrapping);
    if (area.placeholder) ext.push(C.placeholder(area.placeholder));
    var view = new C.EditorView({
      state: C.EditorState.create({ doc: area.value, extensions: ext }),
      parent: el
    });
    area.hidden = true;
    // The textarea's label names the editor, so a screen reader still reads it.
    var label = area.id && document.querySelector('label[for="' + area.id + '"]');
    if (label) {
      label.id = label.id || area.id + '-label';
      view.contentDOM.setAttribute('aria-labelledby', label.id);
      label.addEventListener('click', function () { view.focus(); });
    } else if (area.getAttribute('aria-label')) {
      view.contentDOM.setAttribute('aria-label', area.getAttribute('aria-label'));
    }
    function set(text) {
      view.dispatch({ changes: { from: 0, to: view.state.doc.length, insert: text } });
    }
    if (form) form.addEventListener('reset', function () { setTimeout(function () { set(area.defaultValue); }, 0); });
    el.classList.add('is-ready');
    window.lat.emit(el, 'codeeditor:ready', { language: name });
    return {
      root: el,
      view: view,
      get: function () { return view.state.doc.toString(); },
      set: set,
      focus: function () { view.focus(); }
    };
  });
})();
