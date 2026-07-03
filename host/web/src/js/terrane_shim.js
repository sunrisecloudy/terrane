(function () {
  var invokeUrl = __INVOKE_URL_JSON__;
  var previewUrl = __PREVIEW_URL_JSON__;
  var builderUrl = __BUILDER_URL_JSON__;
  var builderStatusUrl = __BUILDER_STATUS_URL_JSON__;
  var previewId = __PREVIEW_ID_JSON__;
  var bridgeSeq = 0;
  var bridgePending = {};
  // App frames post to the shell — a real origin we can pin. Preview frames
  // post to their embedding APP frame, whose sandboxed opaque origin never
  // matches a concrete targetOrigin: the browser would silently drop the
  // message, so they must post with "*" (the target window is still fixed).
  var bridgeTargetOrigin = previewId
    ? "*"
    : window.location.protocol + "//" + window.location.host;
  var bridgeTimeoutsMs = { invoke: 30000, preview: 30000 };
  // Generation runs in the background on the host; the start request returns
  // immediately and the shim polls status until the draft is committed.
  var BUILDER_POLL_MS = 2000;
  var BUILDER_DEADLINE_MS = 900000;

  // The host asks the user in person for some requests (permission prompts);
  // give those a human-scale deadline once the host signals progress.
  var ELICITATION_TIMEOUT_MS = 600000;

  window.addEventListener("message", function (event) {
    if (event.source !== window.parent) return;
    var message = event.data || {};
    if (message && message.type === "terrane:bridge:progress") {
      var waiting = bridgePending[message.id];
      if (!waiting) return;
      if (waiting.relayTo) {
        waiting.relayTo.postMessage(
          { type: "terrane:bridge:progress", id: waiting.relayId },
          "*"
        );
        return;
      }
      clearTimeout(waiting.timeout);
      waiting.timeout = setTimeout(function () {
        delete bridgePending[message.id];
        waiting.reject(new Error("Terrane host bridge timed out waiting for approval"));
      }, ELICITATION_TIMEOUT_MS);
      return;
    }
    if (!message || message.type !== "terrane:bridge:response") return;
    var pending = bridgePending[message.id];
    if (!pending) return;
    delete bridgePending[message.id];
    if (pending.relayTo) {
      // A nested frame's request we forwarded: hand the answer back down.
      pending.relayTo.postMessage(
        {
          type: "terrane:bridge:response",
          id: pending.relayId,
          ok: !!message.ok,
          body: message.body || {},
        },
        "*"
      );
      return;
    }
    clearTimeout(pending.timeout);
    if (message.ok) {
      pending.resolve(message.body || {});
    } else {
      pending.reject(new Error(errorFromBody(message.body)));
    }
  });

  // Relay bridge traffic for frames nested inside this app — e.g. the App
  // Builder preview iframe. postMessage only reaches the immediate parent and
  // the nested frame's opaque origin blocks fetch, so without this hop its
  // invokes would never reach the shell.
  window.addEventListener("message", function (event) {
    var message = event.data || {};
    if (!message || message.type !== "terrane:bridge:request") return;
    if (!message.id || !isChildFrame(event.source)) return;
    if (canUseParentBridge()) {
      var relayId = "terrane-relay-" + (++bridgeSeq);
      bridgePending[relayId] = { relayTo: event.source, relayId: message.id };
      window.parent.postMessage(
        {
          type: "terrane:bridge:request",
          id: relayId,
          kind: message.kind,
          body: message.body || {},
        },
        bridgeTargetOrigin
      );
      return;
    }
    // Opened as the top-level page there is no shell above us, but this
    // document is same-origin and unsandboxed — answer the child via fetch.
    answerChildLocally(event.source, message);
  });

  function answerChildLocally(child, message) {
    var body = message.body || {};
    var respond = function (ok, payload) {
      child.postMessage(
        {
          type: "terrane:bridge:response",
          id: message.id,
          ok: ok,
          body: payload || {},
        },
        "*"
      );
    };
    if (message.kind !== "previewInvoke" || !body.previewId) {
      respond(false, { error: "unsupported bridge request" });
      return;
    }
    fetch(
      "/__terrane/previews/" + encodeURIComponent(String(body.previewId)) + "/invoke",
      {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({ verb: String(body.verb || ""), args: body.args || [] }),
      }
    )
      .then(function (response) {
        return response.text().then(function (text) {
          var parsed = {};
          if (text) {
            try {
              parsed = JSON.parse(text);
            } catch (_) {
              parsed = { error: text };
            }
          }
          if (!response.ok && !parsed.error) parsed.error = "HTTP " + response.status;
          respond(response.ok, parsed);
        });
      })
      .catch(function (error) {
        respond(false, { error: errorFromBody({ error: String(error) }) });
      });
  }

  function isChildFrame(source) {
    if (!source) return false;
    var frames = document.querySelectorAll("iframe");
    for (var i = 0; i < frames.length; i++) {
      if (frames[i].contentWindow === source) return true;
    }
    return false;
  }

  window.APP_ID = __APP_ID_JSON__;
  window.terrane = {
    invoke: function (verb) {
      var args = Array.prototype.slice.call(arguments, 1).map(String);
      var request;
      if (previewId && canUseParentBridge()) {
        // Preview frames are nested (App Builder embeds them inside an app
        // frame), so the plain "invoke" kind would resolve to the wrong app
        // upstream. Carry the preview id, like the macOS bridge does.
        request = bridgeJson("previewInvoke", {
          previewId: previewId,
          verb: String(verb),
          args: args,
        });
      } else {
        request = postJson("invoke", invokeUrl, { verb: verb, args: args });
      }
      return request.then(function (j) {
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
