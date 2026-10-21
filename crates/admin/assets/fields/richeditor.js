// Rich text: the TipTap engine (vendor/richeditor.js) mounted over a textarea
// that holds the document as HTML. The textarea stays the control the form
// submits and is kept current on every change, so a page with no script, or
// one whose engine failed to load, shows the HTML in a plain textarea with the
// same name and value. The server cleans what comes back to the same schema.
//
// Markup: `<div data-lat-widget="richeditor" data-pick="<picker url>">` with
// a toolbar of `[data-rich-cmd]` buttons and an optional `[data-rich-format]`
// select, a `[data-rich-tablebar]` of `[data-rich-table]` buttons, a
// `[data-rich-body]` the editor mounts into, and the textarea. Words the island
// needs for its dialogs come as `data-w-<key>` attributes.
//
// Keyboard: the engine's own (Mod+B, Mod+I, Mod+U, Mod+Z, Tab in a table) plus
// Mod+Enter to submit the form like any textarea.
//
// Controller: `lat.get(el)` gives `{ get(), set(html), focus(), editor }`.
// Events: `lat:richeditor:ready` once mounted.
(function () {
  var R = window.LateriteRich;
  if (!R || !window.lat) return;

  // The library image carries its record id, which is how the server links
  // the document to the file.
  var LibraryImage = R.Image.extend({
    addAttributes: function () {
      var parent = this.parent ? this.parent() : {};
      parent['data-media'] = { default: null };
      return parent;
    }
  });

  // A snippet: a block the site expands at render. It carries a code and a
  // JSON object of properties, and shows in the editor as a card.
  var SnippetNode = R.Node.create({
    name: 'snippet',
    group: 'block',
    atom: true,
    draggable: true,
    selectable: true,
    addAttributes: function () {
      return {
        code: {
          default: '',
          parseHTML: function (el) { return el.getAttribute('data-snippet') || ''; },
          renderHTML: function (attrs) { return { 'data-snippet': attrs.code }; }
        },
        properties: {
          default: {},
          parseHTML: function (el) {
            try { return JSON.parse(el.getAttribute('data-properties') || '{}') || {}; } catch (e) { return {}; }
          },
          renderHTML: function (attrs) { return { 'data-properties': JSON.stringify(attrs.properties || {}) }; }
        }
      };
    },
    parseHTML: function () { return [{ tag: 'figure[data-snippet]' }]; },
    renderHTML: function (p) { return ['figure', R.mergeAttributes(p.HTMLAttributes)]; },
    addNodeView: function () {
      return function (p) {
        // The widget this editor belongs to lends its words and dialog.
        var widget = p.editor.options.element.closest('.lat-richeditor') || {};
        var ctx = widget.latRich || { word: function (k) { return k; }, askProperties: null };
        var dom = document.createElement('figure');
        dom.className = 'lat-snippet';
        dom.contentEditable = 'false';
        dom.setAttribute('data-snippet', p.node.attrs.code);
        var head = document.createElement('div');
        head.className = 'lat-snippet__head';
        var name = document.createElement('span');
        name.className = 'lat-snippet__name';
        var edit = document.createElement('button');
        edit.type = 'button';
        edit.className = 'lat-btn lat-btn--ghost lat-btn--sm';
        var props = document.createElement('dl');
        props.className = 'lat-snippet__props';
        head.appendChild(name); head.appendChild(edit);
        dom.appendChild(head); dom.appendChild(props);
        var paint = function (node) {
          var def = snippetDefs[node.attrs.code];
          name.textContent = def ? def.name : node.attrs.code;
          edit.textContent = ctx.word('edit');
          edit.hidden = !def || !def.properties.length;
          props.innerHTML = '';
          Object.keys(node.attrs.properties || {}).forEach(function (k) {
            var dt = document.createElement('dt'); dt.textContent = (def && label(def, k)) || k;
            var dd = document.createElement('dd'); dd.textContent = String(node.attrs.properties[k]);
            props.appendChild(dt); props.appendChild(dd);
          });
        };
        paint(p.node);
        edit.addEventListener('click', function () {
          var def = snippetDefs[p.node.attrs.code];
          if (!def || !ctx.askProperties) return;
          ctx.askProperties(def, p.node.attrs.properties, function (values) {
            var pos = p.getPos();
            p.editor.chain().focus().command(function (c) {
              c.tr.setNodeMarkup(pos, undefined, { code: def.code, properties: values });
              return true;
            }).run();
          });
        });
        return {
          dom: dom,
          update: function (node) { if (node.type.name !== 'snippet') return false; paint(node); return true; },
          selectNode: function () { dom.classList.add('is-selected'); },
          deselectNode: function () { dom.classList.remove('is-selected'); }
        };
      };
    }
  });
  var snippetDefs = {};
  function label(def, code) {
    for (var i = 0; i < def.properties.length; i++) if (def.properties[i].code === code) return def.properties[i].title;
    return null;
  }

  var FORMATS = {
    paragraph: function (c) { return c.setParagraph(); },
    h1: function (c) { return c.toggleHeading({ level: 1 }); },
    h2: function (c) { return c.toggleHeading({ level: 2 }); },
    h3: function (c) { return c.toggleHeading({ level: 3 }); },
    h4: function (c) { return c.toggleHeading({ level: 4 }); },
    blockquote: function (c) { return c.toggleBlockquote(); },
    code_block: function (c) { return c.toggleCodeBlock(); }
  };

  window.lat.widget('richeditor', function (root) {
    var area = root.querySelector('textarea');
    var body = root.querySelector('[data-rich-body]');
    if (!area || !body) return;
    var form = area.form;
    var readOnly = area.readOnly || area.disabled;
    var word = function (key) { return root.getAttribute('data-w-' + key) || key; };
    var buttons = {};
    root.querySelectorAll('[data-rich-cmd]').forEach(function (b) { buttons[b.getAttribute('data-rich-cmd')] = b; });
    var format = root.querySelector('[data-rich-format]');
    var tablebar = root.querySelector('[data-rich-tablebar]');

    var Submit = R.Extension.create({
      name: 'latSubmit',
      addKeyboardShortcuts: function () {
        return { 'Mod-Enter': function () { if (form) form.requestSubmit(); return true; } };
      }
    });

    var editor = new R.Editor({
      element: body,
      content: area.value,
      editable: !readOnly,
      extensions: [
        R.StarterKit.configure({
          heading: { levels: [1, 2, 3, 4] },
          // A link opens in the same tab unless the dialog says otherwise.
          link: { openOnClick: false, autolink: true, defaultProtocol: 'https', HTMLAttributes: { rel: 'noopener noreferrer', target: null } }
        }),
        LibraryImage,
        R.Placeholder.configure({ placeholder: area.placeholder || '' }),
        R.Table.configure({ resizable: false }), R.TableRow, R.TableHeader, R.TableCell,
        SnippetNode,
        Submit
      ],
      onUpdate: function () {
        area.value = editor.isEmpty ? '' : editor.getHTML();
        area.dispatchEvent(new Event('input', { bubbles: true }));
        refresh();
      },
      onSelectionUpdate: refresh,
      onTransaction: refresh
    });

    function chain() { return editor.chain().focus(); }
    function refresh() {
      Object.keys(buttons).forEach(function (cmd) {
        var on = false;
        switch (cmd) {
          case 'bold': case 'italic': case 'underline': case 'link': on = editor.isActive(cmd); break;
          case 'strike': on = editor.isActive('strike'); break;
          case 'bullet_list': on = editor.isActive('bulletList'); break;
          case 'ordered_list': on = editor.isActive('orderedList'); break;
          case 'table': on = editor.isActive('table'); break;
        }
        buttons[cmd].setAttribute('aria-pressed', on ? 'true' : 'false');
      });
      if (buttons.undo) buttons.undo.disabled = !editor.can().undo();
      if (buttons.redo) buttons.redo.disabled = !editor.can().redo();
      if (format) {
        var value = 'paragraph';
        [1, 2, 3, 4].forEach(function (l) { if (editor.isActive('heading', { level: l })) value = 'h' + l; });
        if (editor.isActive('blockquote')) value = 'blockquote';
        if (editor.isActive('codeBlock')) value = 'code_block';
        format.value = value;
      }
      if (tablebar) tablebar.hidden = !editor.isActive('table');
    }

    // The range a link applies to: the selection, the link under a caret, or
    // the word under a caret, so pressing Link with nothing selected still
    // links something an author can see.
    function linkRange() {
      var sel = editor.state.selection;
      if (!sel.empty || editor.isActive('link')) return null;
      var $pos = sel.$from;
      var text = $pos.parent.textContent;
      var at = $pos.parentOffset;
      var from = at, to = at;
      while (from > 0 && !/\s/.test(text[from - 1])) from--;
      while (to < text.length && !/\s/.test(text[to])) to++;
      if (from === to) return null;
      var start = $pos.start();
      return { from: start + from, to: start + to };
    }

    // The link dialog: the URL, whether it opens a new tab, and a way to remove.
    var linkDialog = null;
    var linkTarget = null;
    function askLink() {
      linkTarget = linkRange();
      if (!linkDialog) {
        linkDialog = document.createElement('dialog');
        linkDialog.className = 'lat-dialog';
        linkDialog.innerHTML =
          '<form method="dialog" class="lat-richeditor__linkform">' +
          '<div class="lat-dialog__head"><h2></h2><button type="button" class="lat-dialog__close" data-lat-dialog-close aria-label="' + word('cancel') + '">&times;</button></div>' +
          '<div class="lat-dialog__body">' +
          '<div class="lat-field"><label class="lat-field__label"></label><input class="lat-input" type="text" name="href" autocomplete="off" spellcheck="false" placeholder="https://"></div>' +
          '<label class="lat-check"><input type="checkbox" name="blank"> <span></span></label>' +
          '</div>' +
          '<div class="lat-dialog__actions"><button type="button" class="lat-btn lat-btn--ghost" data-link-remove></button><span style="flex:1"></span><button type="button" class="lat-btn lat-btn--ghost" data-lat-dialog-close></button><button type="submit" class="lat-btn lat-btn--primary"></button></div>' +
          '</form>';
        linkDialog.querySelector('h2').textContent = word('link-title');
        linkDialog.querySelector('.lat-field__label').textContent = word('url');
        linkDialog.querySelector('.lat-check span').textContent = word('new-tab');
        linkDialog.querySelector('[data-link-remove]').textContent = word('remove-link');
        linkDialog.querySelector('.lat-dialog__actions [data-lat-dialog-close]').textContent = word('cancel');
        linkDialog.querySelector('[type=submit]').textContent = word('apply');
        document.body.appendChild(linkDialog);
        linkDialog.querySelector('form').addEventListener('submit', function (e) {
          e.preventDefault();
          var href = linkDialog.querySelector('[name=href]').value.trim();
          var blank = linkDialog.querySelector('[name=blank]').checked;
          linkDialog.close();
          var c = chain();
          if (linkTarget) c = c.setTextSelection(linkTarget);
          if (!href) { c.extendMarkRange('link').unsetLink().run(); return; }
          c.extendMarkRange('link').setLink({ href: href, target: blank ? '_blank' : null }).run();
        });
        linkDialog.querySelector('[data-link-remove]').addEventListener('click', function () {
          linkDialog.close();
          chain().extendMarkRange('link').unsetLink().run();
        });
      }
      var current = editor.getAttributes('link');
      linkDialog.querySelector('[name=href]').value = current.href || '';
      linkDialog.querySelector('[name=blank]').checked = current.target === '_blank';
      linkDialog.querySelector('[data-link-remove]').hidden = !current.href;
      linkDialog.showModal();
      linkDialog.querySelector('[name=href]').focus();
    }

    // The properties dialog a snippet asks with: a field per declared
    // property, built from the menu's definitions.
    var propDialog = null;
    function askProperties(def, current, done) {
      if (propDialog) propDialog.remove();
      propDialog = document.createElement('dialog');
      propDialog.className = 'lat-dialog';
      var form = document.createElement('form');
      form.method = 'dialog';
      form.innerHTML =
        '<div class="lat-dialog__head"><h2></h2><button type="button" class="lat-dialog__close" data-lat-dialog-close aria-label="' + word('cancel') + '">&times;</button></div>' +
        '<div class="lat-dialog__body" data-props></div>' +
        '<div class="lat-dialog__actions"><button type="button" class="lat-btn lat-btn--ghost" data-lat-dialog-close></button><button type="submit" class="lat-btn lat-btn--primary"></button></div>';
      form.querySelector('h2').textContent = def.name;
      form.querySelector('.lat-dialog__actions [data-lat-dialog-close]').textContent = word('cancel');
      form.querySelector('[type=submit]').textContent = word('apply');
      var body = form.querySelector('[data-props]');
      if (def.description) {
        var desc = document.createElement('p');
        desc.className = 'lat-field__help';
        desc.textContent = def.description;
        body.appendChild(desc);
      }
      def.properties.forEach(function (prop, i) {
        var value = current && current[prop.code] !== undefined ? current[prop.code] : prop.default;
        var field = document.createElement('div');
        field.className = 'lat-field';
        var id = 'lat-snippet-prop-' + i;
        if (prop.kind === 'checkbox') {
          field.innerHTML = '<label class="lat-check"><input type="checkbox"> <span></span></label>';
          field.querySelector('input').checked = !!value && value !== '0' && value !== 'false';
          field.querySelector('input').name = prop.code;
          field.querySelector('span').textContent = prop.title;
        } else if (prop.kind === 'dropdown') {
          field.innerHTML = '<label class="lat-field__label" for="' + id + '"></label><select class="lat-input" id="' + id + '"></select>';
          field.querySelector('label').textContent = prop.title;
          var select = field.querySelector('select');
          select.name = prop.code;
          (prop.options || []).forEach(function (pair) {
            var opt = document.createElement('option');
            opt.value = pair[0]; opt.textContent = pair[1];
            if (String(value) === pair[0]) opt.selected = true;
            select.appendChild(opt);
          });
        } else {
          field.innerHTML = '<label class="lat-field__label" for="' + id + '"></label><input class="lat-input" type="text" id="' + id + '">';
          field.querySelector('label').textContent = prop.title;
          field.querySelector('input').name = prop.code;
          field.querySelector('input').value = value === undefined || value === null ? '' : String(value);
        }
        body.appendChild(field);
      });
      propDialog.appendChild(form);
      document.body.appendChild(propDialog);
      form.addEventListener('submit', function (e) {
        e.preventDefault();
        var values = {};
        def.properties.forEach(function (prop) {
          var el = form.elements[prop.code];
          if (!el) return;
          values[prop.code] = prop.kind === 'checkbox' ? el.checked : el.value;
        });
        propDialog.close();
        done(values);
      });
      propDialog.showModal();
      var first = form.querySelector('input:not([type=checkbox]), select, input');
      if (first) first.focus();
    }

    root.latRich = { word: word, askProperties: askProperties };

    // The Insert menu: commands modules registered for this surface, then the
    // snippets the sources list now, fetched when first opened.
    var menuBox = null;
    var menuData = null;
    function openInsert(anchor) {
      if (!menuBox) {
        menuBox = document.createElement('div');
        menuBox.className = 'lat-richeditor__menu';
        menuBox.setAttribute('role', 'menu');
        root.appendChild(menuBox);
        document.addEventListener('click', function (e) {
          if (!menuBox.hidden && !menuBox.contains(e.target) && !anchor.contains(e.target)) menuBox.hidden = true;
        });
        menuBox.addEventListener('keydown', function (e) {
          if (e.key === 'Escape') { menuBox.hidden = true; anchor.focus(); return; }
          if (e.key !== 'ArrowDown' && e.key !== 'ArrowUp') return;
          var items = Array.prototype.slice.call(menuBox.querySelectorAll('button'));
          var at = items.indexOf(document.activeElement);
          if (!items.length) return;
          e.preventDefault();
          items[e.key === 'ArrowDown' ? (at + 1) % items.length : (at - 1 + items.length) % items.length].focus();
        });
      }
      if (!menuBox.hidden) { menuBox.hidden = true; return; }
      var paint = function () {
        menuBox.innerHTML = '';
        var any = false;
        (menuData.commands || []).forEach(function (cmd) {
          any = true;
          var b = document.createElement('button');
          b.type = 'button'; b.className = 'lat-richeditor__menuitem'; b.setAttribute('role', 'menuitem');
          b.textContent = cmd.label;
          b.addEventListener('click', function () {
            menuBox.hidden = true;
            if (cmd.html) chain().insertContent(cmd.html).run();
          });
          menuBox.appendChild(b);
        });
        if ((menuData.snippets || []).length) {
          any = true;
          var h = document.createElement('div');
          h.className = 'lat-richeditor__menuhead';
          h.textContent = word('snippets');
          menuBox.appendChild(h);
          menuData.snippets.forEach(function (def) {
            var b = document.createElement('button');
            b.type = 'button'; b.className = 'lat-richeditor__menuitem'; b.setAttribute('role', 'menuitem');
            b.textContent = def.name;
            if (def.description) b.title = def.description;
            b.addEventListener('click', function () {
              menuBox.hidden = true;
              var insert = function (values) {
                chain().insertContent({ type: 'snippet', attrs: { code: def.code, properties: values } }).run();
              };
              if (def.properties.length) askProperties(def, null, insert); else insert({});
            });
            menuBox.appendChild(b);
          });
        }
        if (!any) {
          var none = document.createElement('div');
          none.className = 'lat-richeditor__menuhead';
          none.textContent = word('nothing-to-insert');
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
        .then(function (data) {
          menuData = data;
          (data.snippets || []).forEach(function (def) { snippetDefs[def.code] = def; });
          // Cards drawn before the definitions arrived learn their names.
          editor.view.updateState(editor.view.state);
          paint();
        });
    }
    // Definitions for the cards of a document that already holds snippets.
    if (root.getAttribute('data-menu') && editor.getHTML().indexOf('data-snippet') !== -1) {
      fetch(root.getAttribute('data-menu'), { credentials: 'same-origin', headers: { 'X-Requested-With': 'fetch' } })
        .then(function (r) { return r.ok ? r.json() : null; })
        .then(function (data) {
          if (!data) return;
          menuData = data;
          (data.snippets || []).forEach(function (def) { snippetDefs[def.code] = def; });
          editor.view.updateState(editor.view.state);
        })
        .catch(function () {});
    }

    // The library picker, shared with the media field.
    var picker = window.lat.mediaPicker ? window.lat.mediaPicker({
      multiple: false,
      words: { title: word('library-title'), use: word('library-use'), cancel: word('cancel') },
      onDone: function (items) {
        var item = items[0];
        if (!item || !item.is_image) return;
        chain().setImage({ src: item.url, alt: item.name, 'data-media': item.id }).run();
      }
    }) : null;

    var COMMANDS = {
      bold: function () { chain().toggleBold().run(); },
      italic: function () { chain().toggleItalic().run(); },
      underline: function () { chain().toggleUnderline().run(); },
      strike: function () { chain().toggleStrike().run(); },
      bullet_list: function () { chain().toggleBulletList().run(); },
      ordered_list: function () { chain().toggleOrderedList().run(); },
      link: askLink,
      image: function () { if (picker) picker.open(root.getAttribute('data-pick') + '?mode=image'); },
      table: function () {
        if (editor.isActive('table')) { chain().deleteTable().run(); return; }
        chain().insertTable({ rows: 3, cols: 3, withHeaderRow: true }).run();
      },
      rule: function () { chain().setHorizontalRule().run(); },
      insert: function () { if (buttons.insert) openInsert(buttons.insert); },
      undo: function () { chain().undo().run(); },
      redo: function () { chain().redo().run(); }
    };
    var TABLE = {
      row_after: function () { chain().addRowAfter().run(); },
      col_after: function () { chain().addColumnAfter().run(); },
      row_delete: function () { chain().deleteRow().run(); },
      col_delete: function () { chain().deleteColumn().run(); },
      header_row: function () { chain().toggleHeaderRow().run(); },
      table_delete: function () { chain().deleteTable().run(); }
    };

    root.addEventListener('mousedown', function (e) {
      // A toolbar press must not take the selection away from the document.
      if (e.target.closest && e.target.closest('[data-rich-cmd], [data-rich-table]')) e.preventDefault();
    });
    root.addEventListener('click', function (e) {
      if (!e.target.closest) return;
      var cmd = e.target.closest('[data-rich-cmd]');
      if (cmd && COMMANDS[cmd.getAttribute('data-rich-cmd')]) { COMMANDS[cmd.getAttribute('data-rich-cmd')](); return; }
      var tbl = e.target.closest('[data-rich-table]');
      if (tbl && TABLE[tbl.getAttribute('data-rich-table')]) TABLE[tbl.getAttribute('data-rich-table')]();
    });
    if (format) {
      format.addEventListener('change', function () {
        var f = FORMATS[format.value];
        if (f) f(chain()).run();
      });
    }

    // The textarea's label names the editor, so a screen reader still reads it.
    var label = area.id && document.querySelector('label[for="' + area.id + '"]');
    editor.view.dom.setAttribute('role', 'textbox');
    editor.view.dom.setAttribute('aria-multiline', 'true');
    if (label) {
      label.id = label.id || area.id + '-label';
      editor.view.dom.setAttribute('aria-labelledby', label.id);
      label.addEventListener('click', function () { editor.commands.focus(); });
    }
    if (readOnly) root.classList.add('is-readonly');
    area.hidden = true;
    if (form) form.addEventListener('reset', function () { setTimeout(function () { editor.commands.setContent(area.defaultValue); }, 0); });
    root.classList.add('is-ready');
    refresh();
    window.lat.emit(root, 'richeditor:ready', {});
    return {
      root: root,
      editor: editor,
      get: function () { return editor.isEmpty ? '' : editor.getHTML(); },
      set: function (html) { editor.commands.setContent(html); },
      focus: function () { editor.commands.focus(); }
    };
  });
})();
