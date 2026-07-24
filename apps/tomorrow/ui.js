(function () {
  "use strict";
  var state = null;
  var activeView = "plan";
  var toastTimer = null;
  var previewThemes = [
    ["Begin softly", "Notice what would help."],
    ["Make a little room", "Protect one small pocket."],
    ["Follow curiosity", "Let something surprise you."],
    ["Choose enough", "Keep the plan honest."],
    ["Offer warmth", "Make one interaction gentler."],
    ["Keep what helped", "Carry a useful pattern."],
    ["Look back kindly", "Finish without grading."]
  ];

  function el(id) { return document.getElementById(id); }
  function invoke() {
    if (!window.terrane || typeof window.terrane.invoke !== "function") {
      return Promise.reject(new Error("Terrane bridge unavailable"));
    }
    return window.terrane.invoke.apply(window.terrane, arguments);
  }
  function parse(json) {
    var value = JSON.parse(json);
    if (value && value.ok === false) throw new Error(value.error || "That did not work.");
    return value;
  }
  function escapeText(value) {
    return String(value == null ? "" : value);
  }
  function toast(message) {
    el("toast").textContent = message;
    el("toast").classList.add("show");
    clearTimeout(toastTimer);
    toastTimer = setTimeout(function () { el("toast").classList.remove("show"); }, 2600);
  }
  function fail(error) {
    toast(error && error.message ? error.message : String(error));
  }

  function renderPreview() {
    var list = el("preview-list");
    list.textContent = "";
    previewThemes.forEach(function (theme, index) {
      var item = document.createElement("li");
      var number = document.createElement("span");
      number.className = "day-num";
      number.textContent = String(index + 1).padStart(2, "0");
      var title = document.createElement("strong");
      title.textContent = theme[0];
      var copy = document.createElement("small");
      copy.textContent = theme[1];
      item.append(number, title, copy);
      list.appendChild(item);
    });
  }

  function publishSidebar() {
    if (!window.terrane || typeof window.terrane.setSidebarSection !== "function") return;
    var days = state && state.days ? state.days : previewThemes.map(function (theme, index) {
      return { day: index + 1, title: theme[0], complete: false };
    });
    window.terrane.setSidebarSection({
      title: "Seven-day journey",
      items: days.map(function (day) {
        return {
          id: "day-" + day.day,
          title: "Day " + day.day + " · " + day.title,
          subtitle: day.complete ? "Reflection saved" : (state && state.started ? "Open and editable" : "A gentle preview"),
          systemImage: day.complete ? "checkmark.circle.fill" : "leaf"
        };
      }),
      selectedItemId: state && state.started ? "day-" + state.selectedDay : undefined,
      createLabel: state && state.started ? "Plan the next day" : "Start the journey"
    }).catch(function () {});
  }

  function render() {
    if (!state) return;
    el("onboarding").classList.toggle("hidden", state.started);
    el("journey").classList.toggle("hidden", !state.started);
    el("mode-badge").textContent = state.settings && state.settings.harness === "templates"
      ? "Local templates"
      : "Verified assistance";
    publishSidebar();
    if (!state.started) return;

    var day = state.days[state.selectedDay - 1];
    el("day-eyebrow").textContent = "Day " + state.selectedDay + " of 7";
    el("day-title").textContent = day.title;
    el("day-preview").textContent = day.preview;
    el("day-ring-number").textContent = state.selectedDay;
    el("carried-insight").textContent = day.carriedInsight || "";
    el("carried-insight").classList.toggle("hidden", !day.carriedInsight);
    el("reflection-text").value = day.reflection || "";
    el("encouragement").textContent = day.encouragement || "";
    el("encouragement").classList.toggle("hidden", !day.encouragement);
    renderPlan(day);
    setView(activeView);
  }

  function renderPlan(day) {
    var list = el("plan-list");
    list.textContent = "";
    el("plan-empty").classList.toggle("hidden", day.items.length > 0);
    day.items.forEach(function (item, index) {
      var article = document.createElement("article");
      article.className = "plan-item" + (item.done ? " done" : "");
      article.dataset.id = item.id;

      var toggle = document.createElement("button");
      toggle.className = "complete-button";
      toggle.type = "button";
      toggle.setAttribute("aria-label", item.done ? "Mark " + item.title + " open" : "Mark " + item.title + " complete");
      toggle.textContent = item.done ? "✓" : "";
      toggle.addEventListener("click", function () { action("toggle_item", String(item.id)); });

      var copy = document.createElement("div");
      copy.className = "item-copy";
      var title = document.createElement("h3");
      title.textContent = item.title;
      var meta = document.createElement("div");
      meta.className = "item-meta";
      var tag = document.createElement("span");
      tag.className = "type-tag";
      tag.textContent = item.type;
      meta.appendChild(tag);
      if (item.time) { var time = document.createElement("span"); time.textContent = item.time; meta.appendChild(time); }
      if (item.note) { var note = document.createElement("span"); note.textContent = "· " + item.note; meta.appendChild(note); }
      copy.append(title, meta);

      var actions = document.createElement("div");
      actions.className = "item-actions";
      [["↑", "Move up", "up"], ["↓", "Move down", "down"]].forEach(function (spec) {
        var button = document.createElement("button");
        button.type = "button";
        button.textContent = spec[0];
        button.setAttribute("aria-label", spec[1] + " " + item.title);
        button.disabled = spec[2] === "up" ? index === 0 : index === day.items.length - 1;
        button.addEventListener("click", function () { action("reorder_item", String(item.id), spec[2]); });
        actions.appendChild(button);
      });
      var edit = document.createElement("button");
      edit.type = "button";
      edit.textContent = "Edit";
      edit.setAttribute("aria-label", "Edit " + item.title);
      edit.addEventListener("click", function () { openItem(item); });
      actions.appendChild(edit);
      article.append(toggle, copy, actions);
      list.appendChild(article);
    });
  }

  function setView(view) {
    activeView = view;
    document.querySelectorAll(".tab").forEach(function (tab) {
      var selected = tab.dataset.view === view;
      tab.classList.toggle("active", selected);
      tab.setAttribute("aria-selected", selected ? "true" : "false");
    });
    document.querySelectorAll(".view").forEach(function (panel) {
      panel.classList.toggle("active", panel.id === "view-" + view);
    });
  }

  function action() {
    var args = Array.prototype.slice.call(arguments);
    invoke.apply(null, args).then(parse).then(function (next) {
      state = next;
      render();
    }).catch(fail);
  }

  function openItem(item) {
    el("item-dialog-title").textContent = item ? "Edit this plan item" : "Add to the plan";
    el("item-id").value = item ? item.id : "";
    el("item-type").value = item ? item.type : "task";
    el("item-title").value = item ? item.title : "";
    el("item-time").value = item ? item.time : "";
    el("item-note").value = item ? item.note : "";
    el("item-dialog").showModal();
    setTimeout(function () { el("item-title").focus(); }, 50);
  }

  function load() {
    invoke("state").then(parse).then(function (next) {
      state = next;
      render();
    }).catch(fail);
  }

  document.querySelectorAll('input[name="intent"]').forEach(function (input) {
    input.addEventListener("change", function () {
      el("custom-intent-wrap").classList.toggle("hidden", input.value !== "own");
      if (input.value === "own") el("custom-intent").focus();
    });
  });
  el("continue-intent").addEventListener("click", function () {
    var checked = document.querySelector('input[name="intent"]:checked');
    if (!checked) return fail(new Error("Choose an intention first."));
    if (checked.value === "own" && !el("custom-intent").value.trim()) return fail(new Error("Write your own intention first."));
    el("intent-step").classList.add("hidden");
    el("questions-step").classList.remove("hidden");
    el("energy").focus();
  });
  el("back-intent").addEventListener("click", function () {
    el("questions-step").classList.add("hidden");
    el("intent-step").classList.remove("hidden");
  });
  el("create-plan").addEventListener("click", function () {
    var checked = document.querySelector('input[name="intent"]:checked');
    var space = document.querySelector('input[name="space"]:checked');
    var details = {
      intention: checked ? checked.value : "",
      customIntent: checked && checked.value === "own" ? el("custom-intent").value : "",
      energy: el("energy").value,
      anchor: el("anchor").value,
      space: space ? space.value : "a little",
      createdAt: new Date().toISOString()
    };
    invoke("start_journey", JSON.stringify(details)).then(parse).then(function (next) {
      state = next;
      activeView = "plan";
      render();
      toast("Tomorrow’s first plan is ready — and entirely yours to change.");
    }).catch(fail);
  });

  document.querySelectorAll(".tab").forEach(function (tab) {
    tab.addEventListener("click", function () { setView(tab.dataset.view); });
  });
  el("settings-shortcut").addEventListener("click", function () {
    if (state && state.started) setView("settings");
    else toast("Settings will be ready after you begin the journey.");
  });
  el("brand-home").addEventListener("click", function (event) {
    event.preventDefault();
    if (state && state.started) setView("plan");
  });
  el("add-item").addEventListener("click", function () { openItem(null); });
  el("add-empty").addEventListener("click", function () { openItem(null); });
  document.querySelectorAll(".close-dialog").forEach(function (button) {
    button.addEventListener("click", function () { el("item-dialog").close(); });
  });
  el("item-form").addEventListener("submit", function (event) {
    event.preventDefault();
    var item = {
      type: el("item-type").value,
      title: el("item-title").value,
      time: el("item-time").value,
      note: el("item-note").value
    };
    var id = el("item-id").value;
    var promise = id
      ? invoke("edit_item", id, JSON.stringify(item))
      : invoke("add_item", JSON.stringify(item));
    promise.then(parse).then(function (next) {
      state = next;
      el("item-dialog").close();
      render();
      toast(id ? "Plan item updated." : "Added — exactly where you put it.");
    }).catch(fail);
  });
  el("reflection-form").addEventListener("submit", function (event) {
    event.preventDefault();
    invoke("save_reflection", JSON.stringify({
      text: el("reflection-text").value,
      encouragement: el("want-encouragement").checked
    })).then(parse).then(function (next) {
      state = next;
      render();
      setView("reflection");
      toast("Saved locally. Tomorrow can carry this gently forward.");
    }).catch(fail);
  });
  document.querySelectorAll(".review-connection").forEach(function (button) {
    button.addEventListener("click", function () {
      var service = button.dataset.service;
      el("connection-dialog-title").textContent = "Review " + service + " access";
      el("connection-detail").textContent = service === "Calendar"
        ? "Requested scope would be read-only event titles and times for a date you explicitly choose. No event creation, changes, or deletion."
        : "Requested scope would be read-only metadata for messages you explicitly select. No sending, deletion, archiving, or broad mailbox access.";
      el("connection-dialog").showModal();
    });
  });
  el("save-settings").addEventListener("click", function () {
    invoke("save_settings", JSON.stringify({ harness: "templates", calendar: "disconnected", gmail: "disconnected" }))
      .then(parse).then(function (next) { state = next; render(); setView("settings"); toast("Local templates remain active."); })
      .catch(fail);
  });

  if (window.terrane && typeof window.terrane.onSidebarItemSelect === "function") {
    window.terrane.onSidebarItemSelect(function (id) {
      var day = parseInt(String(id).replace("day-", ""), 10);
      if (!state || !state.started) {
        document.querySelector('input[name="intent"]').focus();
        return toast("Choose an intention to begin Day 1.");
      }
      invoke("select_day", String(day)).then(parse).then(function (next) {
        state = next;
        activeView = "plan";
        render();
      }).catch(fail);
    });
  }
  if (window.terrane && typeof window.terrane.onSidebarCreate === "function") {
    window.terrane.onSidebarCreate(function () {
      if (!state || !state.started) {
        el("intent-step").scrollIntoView({ behavior: "smooth" });
        return;
      }
      var nextDay = Math.min(7, state.selectedDay + 1);
      invoke("select_day", String(nextDay)).then(parse).then(function (next) {
        state = next;
        activeView = "plan";
        render();
        if (!state.days[state.selectedDay - 1].items.length) openItem(null);
      }).catch(fail);
    });
  }

  renderPreview();
  load();
}());
