(function () {
  if (typeof handle === 'function') return;
  if (typeof actions !== 'object' || actions === null) return;
  var ID = (typeof __terrane_app_id === 'string') ? __terrane_app_id : '';
  var NAME = (typeof __terrane_app_name === 'string') ? __terrane_app_name : '';
  var DESC = (typeof description === 'string') ? description : '';
  function usageFor(verb) {
    var a = actions[verb];
    var slots = (a && a.args ? a.args : []).map(function (x) {
      return x.required ? '<' + x.name + '>' : '[' + x.name + ']';
    });
    return 'usage: ' + verb + (slots.length ? ' ' + slots.join(' ') : '');
  }
  function runnerFor(a) {
    if (typeof a === 'function') return a;
    if (a && typeof a.run === 'function') {
      return function (args, usage) {
        return a.run(args, usage);
      };
    }
    return null;
  }
  function describe() {
    var list = Object.keys(actions).map(function (verb) {
      var a = actions[verb];
      return {
        verb: verb,
        summary: a.summary || '',
        args: a.args || [],
        returns: a.returns || ''
      };
    });
    return JSON.stringify({ app: ID, title: NAME, description: DESC, actions: list });
  }
  globalThis.handle = function (input) {
    var argv = input || [];
    var verb = argv[0] || '';
    if (verb === '__actions__') return describe();
    var a = actions[verb];
    var run = runnerFor(a);
    if (!run) {
      return 'unknown verb: ' + verb
        + ' (try ' + Object.keys(actions).join(' | ') + ')';
    }
    return run(argv.slice(1), function () { return usageFor(verb); });
  };
})();
