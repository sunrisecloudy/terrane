window.APP_ID = __APP_ID_JSON__;
window.terrane = {
  invoke: function (verb) {
    var args = Array.prototype.slice.call(arguments, 1).map(String);
    return fetch("/apps/" + window.APP_ID + "/invoke", {
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
__LIVE_RELOAD_SCRIPT__
