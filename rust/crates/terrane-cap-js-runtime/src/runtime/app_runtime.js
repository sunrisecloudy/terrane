(function () {
  if (typeof handle === 'function') return;
  if (typeof actions !== 'object' || actions === null) return;
  var ID = (typeof __terrane_app_id === 'string') ? __terrane_app_id : '';
  var NAME = (typeof __terrane_app_name === 'string') ? __terrane_app_name : '';
  var DESC = (typeof description === 'string') ? description : '';
  function ensureCommonActions() {
    if (!actions['common.receive']) {
      actions['common.receive'] = {
        summary: 'Receive an inbound common payload.',
        args: [
          { name: 'kind', required: true },
          { name: 'payloadJson', required: true }
        ],
        returns: 'JSON acknowledgement',
        run: function (args) {
          var kind = args[0] || 'json';
          var payload = args[1] || '{}';
          if (ctx && ctx.resource && ctx.resource.kv) {
            var kv = ctx.resource.kv;
            var raw = kv.get('inbox/seq');
            var seq = raw == null ? 1 : (parseInt(raw, 10) || 0) + 1;
            kv.set('inbox/seq', String(seq));
            kv.set('inbox/' + seq, JSON.stringify({ id: String(seq), kind: kind, payload: payload }));
            return JSON.stringify({ ok: true, id: String(seq) });
          }
          return JSON.stringify({ ok: true });
        }
      };
    }
    if (!actions['common.list']) {
      actions['common.list'] = {
        summary: 'List addressable items.',
        args: [{ name: 'filterJson', required: false }],
        returns: 'JSON array of {id,title,kind}',
        run: function () {
          if (!(ctx && ctx.resource && ctx.resource.kv)) return '[]';
          var all = ctx.resource.kv.all();
          var out = [];
          Object.keys(all).sort().forEach(function (key) {
            if (key.indexOf('items/') !== 0) return;
            try {
              var item = JSON.parse(all[key]);
              out.push({
                id: String(item.id || key.slice('items/'.length)),
                title: String(item.title || item.text || item.id || key),
                kind: String(item.kind || 'item')
              });
            } catch (_) {}
          });
          return JSON.stringify(out);
        }
      };
    }
    if (!actions['common.get']) {
      actions['common.get'] = {
        summary: 'Read one addressable item.',
        args: [{ name: 'id', required: true }],
        returns: 'item JSON or typed not-found JSON',
        run: function (args) {
          var id = args[0] || '';
          if (ctx && ctx.resource && ctx.resource.kv) {
            var raw = ctx.resource.kv.get('items/' + id);
            if (raw != null) return raw;
          }
          return JSON.stringify({ ok: false, error: { code: 'NotFound', id: id } });
        }
      };
    }
  }
  ensureCommonActions();
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
