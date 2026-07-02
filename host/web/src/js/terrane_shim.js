(function () {
  var invokeUrl = __INVOKE_URL_JSON__;
  var previewUrl = __PREVIEW_URL_JSON__;
  var builderUrl = __BUILDER_URL_JSON__;
  var builderStatusUrl = __BUILDER_STATUS_URL_JSON__;
  var bridgeSeq = 0;
  var bridgePending = {};
  var bridgeTargetOrigin = window.location.protocol + "//" + window.location.host;
  var bridgeTimeoutsMs = { invoke: 30000, preview: 30000 };
  // Generation runs in the background on the host; the start request returns
  // immediately and the shim polls status until the draft is committed.
  var BUILDER_POLL_MS = 2000;
  var BUILDER_DEADLINE_MS = 900000;

  window.addEventListener("message", function (event) {
    if (event.source !== window.parent) return;
    var message = event.data || {};
    if (!message || message.type !== "terrane:bridge:response") return;
    var pending = bridgePending[message.id];
    if (!pending) return;
    delete bridgePending[message.id];
    clearTimeout(pending.timeout);
    if (message.ok) {
      pending.resolve(message.body || {});
    } else {
      pending.reject(new Error(errorFromBody(message.body)));
    }
  });

  window.APP_ID = __APP_ID_JSON__;
  window.terrane = {
    invoke: function (verb) {
      var args = Array.prototype.slice.call(arguments, 1).map(String);
      return postJson("invoke", invokeUrl, { verb: verb, args: args })
        .then(function (j) {
          if (j.error) throw new Error(j.error);
          return j.output;
        });
    },
  };

  if (previewUrl) {
    window.terrane.preview = function (files) {
      return postJson("preview", previewUrl, { files: files || [] })
        .then(function (j) {
          if (j.error) throw new Error(j.error);
          return j;
        });
    };
  }

  if (builderUrl) {
    window.terrane.builderGenerate = function (request) {
      request = request || {};
      return postJson(
        "builderGenerate",
        builderUrl,
        {
          id: String(request.id || ""),
          name: String(request.name || ""),
          prompt: String(request.prompt || ""),
          harness: String(request.harness || request.agent || "codex"),
        }
      )
        .then(function (j) {
          if (j.error) throw new Error(j.error);
          if (j.status === "running" && j.id && builderStatusUrl) {
            return waitForBuilderDraft(j.id, Date.now() + BUILDER_DEADLINE_MS);
          }
          return j;
        });
    };
  }

  function waitForBuilderDraft(id, deadline) {
    return new Promise(function (resolve) {
      setTimeout(resolve, BUILDER_POLL_MS);
    })
      .then(function () {
        return postJson("builderStatus", builderStatusUrl, { id: id });
      })
      .then(function (j) {
        if (j.error) throw new Error(j.error);
        if (j.status === "running") {
          if (Date.now() > deadline) {
            throw new Error("Terrane app generation timed out");
          }
          return waitForBuilderDraft(id, deadline);
        }
        return j;
      });
  }

  function postJson(kind, url, body) {
    if (canUseParentBridge()) return bridgeJson(kind, body);
    return fetch(url, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify(body || {}),
    })
      .then(function (response) {
        return response.json();
      });
  }

  function canUseParentBridge() {
    return window.parent && window.parent !== window;
  }

  function bridgeJson(kind, body) {
    return new Promise(function (resolve, reject) {
      var id = "terrane-bridge-" + (++bridgeSeq);
      var timeoutMs = bridgeTimeoutsMs[kind] || 30000;
      var timeout = setTimeout(function () {
        delete bridgePending[id];
        reject(
          new Error(
            "Terrane host bridge timed out after " + Math.round(timeoutMs / 1000) + "s"
          )
        );
      }, timeoutMs);
      bridgePending[id] = {
        resolve: resolve,
        reject: reject,
        timeout: timeout,
      };
      window.parent.postMessage(
        {
          type: "terrane:bridge:request",
          id: id,
          kind: kind,
          body: body || {},
        },
        bridgeTargetOrigin
      );
    });
  }

  function errorFromBody(body) {
    if (body && body.error) return body.error;
    return "Terrane host bridge request failed";
  }

  __LIVE_RELOAD_SCRIPT__;
})();
