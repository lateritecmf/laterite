// Boards (laterite_admin::board): packs a dashboard's widgets under one
// another, and runs its edit mode. The model is the rows of widgets; every
// change rewrites each widget's columns and order, and the grid packs them.
(function () {
  var COLUMNS = 20, UNIT = 8;

  function pack(grid) {
    var packed = getComputedStyle(grid).getPropertyValue("--lat-board-packed").trim() === "1";
    var gap = parseFloat(getComputedStyle(grid).columnGap) || 16;
    grid.querySelectorAll(".lat-board__widget").forEach(function (w) {
      w.style.gridRowEnd = "";
      if (packed) w.style.gridRowEnd = "span " + Math.ceil((w.getBoundingClientRect().height + gap) / UNIT);
    });
  }

  function watch(grid) {
    if (grid.dataset.latBoardWatched) return;
    grid.dataset.latBoardWatched = "1";
    var pending = 0;
    var again = function () {
      cancelAnimationFrame(pending);
      pending = requestAnimationFrame(function () { pack(grid); });
    };
    grid.latBoardPack = again;
    if (window.ResizeObserver) {
      var seen = new ResizeObserver(again);
      seen.observe(grid);
      grid.querySelectorAll(".lat-board__body").forEach(function (el) { seen.observe(el); });
    }
    grid.addEventListener("htmx:afterSettle", again);
    window.addEventListener("resize", again);
    again();
  }

  function edit(bar) {
    var grid = bar.parentElement.querySelector("[data-lat-board]");
    if (!grid || bar.dataset.latBoardBound) return;
    bar.dataset.latBoardBound = "1";
    var dialog = document.getElementById("lat-board-configure");
    var base = bar.getAttribute("action").split("/layout")[0];
    var rows = [], dragged = null, before = null, strip = null;

    function read() {
      rows = []; var byRow = {};
      grid.querySelectorAll(".lat-board__widget").forEach(function (el) {
        var key = el.dataset.row;
        if (!byRow[key]) { byRow[key] = []; rows.push(byRow[key]); }
        byRow[key].push({ id: el.dataset.id, width: +el.dataset.width, el: el });
      });
    }
    function draw() {
      rows = rows.filter(function (r) { return r.length; });
      rows.forEach(function (row, i) {
        var start = 1;
        row.forEach(function (w) {
          w.el.dataset.row = i + 1; w.el.dataset.width = w.width;
          w.el.style.setProperty("--span", w.width);
          w.el.style.setProperty("--start", start);
          start += w.width;
          grid.appendChild(w.el);
        });
      });
      if (strip) grid.appendChild(strip);
      pack(grid);
    }
    function find(id) {
      for (var r = 0; r < rows.length; r++)
        for (var i = 0; i < rows[r].length; i++)
          if (rows[r][i].id === id) return { row: r, index: i, item: rows[r][i] };
      return null;
    }
    function used(row, except) {
      return row.reduce(function (n, w) { return n + (w === except ? 0 : w.width); }, 0);
    }
    function refuse(el) {
      el.classList.remove("is-refused"); void el.offsetWidth; el.classList.add("is-refused");
    }
    function layout() {
      return JSON.stringify(rows.map(function (r) {
        return r.map(function (w) { return { id: w.id, width: w.width }; });
      }));
    }

    function tool(label, text, run) {
      var b = document.createElement("button");
      b.type = "button"; b.className = "lat-board__tool"; b.title = label;
      b.setAttribute("aria-label", label); b.textContent = text;
      b.addEventListener("click", run);
      return b;
    }
    function configure(id, values) {
      var url = base + "/configure/" + encodeURIComponent(id);
      fetch(url + (values ? "?" + values : ""), { credentials: "same-origin", headers: { "HX-Request": "true" } })
        .then(function (r) { return r.ok ? r.text() : Promise.reject(r.status); })
        .then(function (html) {
          dialog.innerHTML = html;
          var form = dialog.querySelector("form");
          form.action = url + (form.dataset.carried || "");
          form.addEventListener("submit", function () { form.elements.layout.value = layout(); });
          // A setting others depend on draws the form again with what it holds.
          var reload = (form.dataset.reload || "").split(",").filter(Boolean);
          form.addEventListener("change", function (e) {
            if (reload.indexOf(e.target.name) < 0) return;
            var held = [];
            new FormData(form).forEach(function (value, name) {
              if (name !== "_csrf" && name !== "layout" && typeof value === "string")
                held.push("p." + encodeURIComponent(name) + "=" + encodeURIComponent(value));
            });
            configure(id, held.join("&"));
          });
          if (window.htmx) window.htmx.process(dialog);
          if (!dialog.open) dialog.showModal();
          var first = dialog.querySelector(".lat-dialog__body input:not([type=hidden]), .lat-dialog__body select, .lat-dialog__body textarea");
          if (first) first.focus();
        })
        .catch(function () { /* the widget keeps its settings */ });
    }
    // One place earlier or later in reading order: along its row, then into
    // the next row when it fits there, else into a row of its own.
    function step(el, by, button) {
      var at = find(el.dataset.id); if (!at) return;
      var row = rows[at.row], item = at.item, to = at.index + by, next = rows[at.row + by];
      if (to >= 0 && to < row.length) {
        row.splice(at.index, 1); row.splice(to, 0, item);
      } else if (next && used(next) + item.width <= COLUMNS) {
        row.splice(at.index, 1);
        if (by < 0) next.push(item); else next.unshift(item);
      } else if (row.length > 1) {
        row.splice(at.index, 1);
        rows.splice(at.row + (by < 0 ? 0 : 1), 0, [item]);
      } else if (next) {
        rows[at.row] = next; rows[at.row + by] = row;
      } else { refuse(el); return; }
      draw();
      button.focus();
      el.scrollIntoView({ block: "nearest" });
    }
    function tools(el) {
      if (el.querySelector(".lat-board__tools")) return;
      var box = document.createElement("div");
      box.className = "lat-board__tools";
      if (el.dataset.move === "1") {
        box.appendChild(tool(bar.dataset.earlierLabel || "Move earlier", "↑", function (e) { step(el, -1, e.currentTarget); }));
        box.appendChild(tool(bar.dataset.laterLabel || "Move later", "↓", function (e) { step(el, 1, e.currentTarget); }));
      }
      if (el.dataset.configure === "1")
        box.appendChild(tool(bar.dataset.configureLabel || "Configure", "⚙", function () { configure(el.dataset.id); }));
      if (el.dataset.remove === "1")
        box.appendChild(tool(bar.dataset.removeLabel || "Remove", "×", function () {
          var at = find(el.dataset.id); if (!at) return;
          rows[at.row].splice(at.index, 1); el.remove(); draw();
        }));
      if (box.children.length) el.insertBefore(box, el.firstChild);
      if (el.dataset.resize === "1") {
        var grip = document.createElement("div");
        grip.className = "lat-board__resize";
        grip.addEventListener("pointerdown", function (e) { resize(e, el, grip); });
        el.appendChild(grip);
      }
    }
    function resize(e, el, grip) {
      e.preventDefault(); e.stopPropagation();
      var at = find(el.dataset.id); if (!at) return;
      var column = grid.getBoundingClientRect().width / COLUMNS;
      var startX = e.clientX, startWidth = at.item.width;
      var least = +el.dataset.min || 1;
      var most = Math.min(+el.dataset.max || COLUMNS, COLUMNS - used(rows[at.row], at.item));
      try { grip.setPointerCapture(e.pointerId); } catch (err) { /* synthetic pointers have no capture */ }
      var was = el.draggable; el.draggable = false;
      function move(ev) {
        var width = Math.max(least, Math.min(most, startWidth + Math.round((ev.clientX - startX) / column)));
        if (width !== at.item.width) { at.item.width = width; draw(); }
      }
      function up() {
        grip.removeEventListener("pointermove", move); grip.removeEventListener("pointerup", up);
        el.draggable = was;
      }
      grip.addEventListener("pointermove", move); grip.addEventListener("pointerup", up);
    }

    function start() {
      if (grid.classList.contains("is-editing")) return;
      read();
      strip = document.createElement("div");
      strip.className = "lat-board__newrow";
      strip.textContent = bar.dataset.newRowLabel || "Drop here for a new row";
      grid.classList.add("is-editing");
      bar.querySelector("[data-lat-board-start]").hidden = true;
      bar.querySelector(".lat-board__editing").hidden = false;
      grid.querySelectorAll(".lat-board__widget").forEach(function (el) {
        el.draggable = el.dataset.move === "1"; tools(el);
      });
      draw();
    }

    grid.addEventListener("dragstart", function (e) {
      var el = e.target.closest && e.target.closest(".lat-board__widget");
      if (!el || !grid.classList.contains("is-editing") || el.dataset.move !== "1") return;
      dragged = find(el.dataset.id).item; before = rows.map(function (r) { return r.slice(); });
      e.dataTransfer.effectAllowed = "move";
      try { e.dataTransfer.setData("text/plain", dragged.id); } catch (err) { /* some browsers refuse */ }
      requestAnimationFrame(function () { el.classList.add("is-dragged"); });
    });
    grid.addEventListener("dragover", function (e) {
      if (!dragged) return;
      e.preventDefault();
      var over = e.target.closest && e.target.closest(".lat-board__widget, .lat-board__newrow");
      if (!over || over === dragged.el) return;
      var from = find(dragged.id);
      if (over === strip) {
        if (rows[from.row].length === 1 && from.row === rows.length - 1) return;
        rows[from.row].splice(from.index, 1); rows.push([dragged]); draw(); return;
      }
      var to = find(over.dataset.id); if (!to) return;
      if (from.row !== to.row && used(rows[to.row]) + dragged.width > COLUMNS) { refuse(over); return; }
      var box = over.getBoundingClientRect();
      var after = e.clientX > box.left + box.width / 2;
      rows[from.row].splice(from.index, 1);
      to = find(over.dataset.id);
      rows[to.row].splice(to.index + (after ? 1 : 0), 0, dragged);
      draw();
    });
    grid.addEventListener("drop", function (e) { if (dragged) { e.preventDefault(); before = null; } });
    grid.addEventListener("dragend", function () {
      if (!dragged) return;
      dragged.el.classList.remove("is-dragged");
      if (before) { rows = before; draw(); }
      dragged = null; before = null;
    });

    bar.querySelector("[data-lat-board-start]").addEventListener("click", start);
    bar.querySelector("[data-lat-board-cancel]").addEventListener("click", function () {
      var url = new URL(location.href);
      url.searchParams.delete("edit"); url.searchParams.delete("configure");
      location.href = url.toString();
    });
    bar.addEventListener("submit", function () { bar.elements.layout.value = layout(); });

    // Back from adding a widget or saving its settings: still editing.
    if (grid.dataset.latBoardEdit === "1") {
      start();
      var wanted = new URL(location.href).searchParams.get("configure");
      if (wanted && find(wanted)) configure(wanted);
    }
  }

  function scan() {
    document.querySelectorAll("[data-lat-board]").forEach(watch);
    document.querySelectorAll("[data-lat-board-bar]").forEach(edit);
  }
  if (document.readyState === "loading") document.addEventListener("DOMContentLoaded", scan);
  else scan();
  document.addEventListener("htmx:load", scan);
})();
