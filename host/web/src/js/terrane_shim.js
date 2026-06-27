(function () {
  var invokeUrl = __INVOKE_URL_JSON__;
  var previewUrl = __PREVIEW_URL_JSON__;

  window.APP_ID = __APP_ID_JSON__;
  window.terrane = {
    invoke: function (verb) {
      var args = Array.prototype.slice.call(arguments, 1).map(String);
      return fetch(invokeUrl, {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({ verb: verb, args: args }),
      })
        .then(function (r) {
          return r.json();
        })
        .then(function (j) {
          if (j.error) throw new Error(j.error);
          return j.output;
        });
    },
  };

  if (previewUrl) {
    window.terrane.preview = function (files) {
      return fetch(previewUrl, {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({ files: files || [] }),
      })
        .then(function (r) {
          return r.json();
        })
        .then(function (j) {
          if (j.error) throw new Error(j.error);
          return j;
        });
    };
  }

  __LIVE_RELOAD_SCRIPT__;
})();
