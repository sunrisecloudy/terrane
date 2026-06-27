(function () {
  var seen = null;

  function check() {
    fetch("/apps/" + window.APP_ID + "/__terrane/live-version", {
      cache: "no-store",
    })
      .then(function (r) {
        if (!r.ok) throw new Error("live-version");
        return r.json();
      })
      .then(function (j) {
        if (!j.version) return;
        if (seen === null) {
          seen = j.version;
          return;
        }
        if (seen !== j.version) window.location.reload();
      })
      .catch(function () {});
  }

  setInterval(check, 750);
  check();
})();
