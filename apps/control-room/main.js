// Control Room is intentionally a thin, read-only adapter. The capability
// owns redaction and aggregation; this backend never opens another app, reads
// raw records, or exposes a mutation verb.

var catalogResource = ctx.resource && ctx.resource["control-room"];

function snapshot() {
  if (!catalogResource) {
    return JSON.stringify({
      error: "control-room not granted",
      readOnly: true,
      help: "Grant read-only metadata access to the Control Room app.",
    });
  }
  return catalogResource.catalog();
}

function searchCatalog(query) {
  var data = JSON.parse(snapshot());
  if (data.error) return JSON.stringify(data);
  var needle = String(query || "").toLowerCase();
  if (!needle) {
    return JSON.stringify({ apps: data.apps, capabilities: data.capabilities });
  }
  function matches(item) {
    return JSON.stringify(item).toLowerCase().indexOf(needle) !== -1;
  }
  return JSON.stringify({
    apps: data.apps.filter(matches),
    capabilities: data.capabilities.filter(matches),
    mcpTools: data.mcp.tools.filter(matches),
  });
}

var description =
  "Read-only, redacted inventory of Terrane apps, capabilities, permissions, " +
  "MCP tools, runtimes, models, storage aggregates, and documentation.";

var actions = {
  overview: {
    summary: "Return the system overview and privacy boundary.",
    args: [],
    returns: "JSON with system counts, source labels, and redaction guarantees",
    run: function () {
      var data = JSON.parse(snapshot());
      if (data.error) return JSON.stringify(data);
      return JSON.stringify({
        system: data.system,
        generatedFrom: data.generatedFrom,
        privacy: data.privacy,
      });
    },
  },
  catalog: {
    summary: "Return the complete read-only Control Room catalog.",
    args: [],
    returns: "metadata-only JSON; never raw secrets or sensitive records",
    run: function () {
      return snapshot();
    },
  },
  search: {
    summary: "Search apps, capabilities, and MCP tools by metadata text.",
    args: [{ name: "query", required: true, summary: "case-insensitive search text" }],
    returns: "JSON with matching apps, capabilities, and MCP tools",
    run: function (args) {
      return searchCatalog(args.join(" "));
    },
  },
  "common.list": {
    summary: "List apps and capabilities as addressable metadata items.",
    args: [{ name: "filterJson", required: false }],
    returns: "JSON array of {id,title,kind}",
    run: function () {
      var data = JSON.parse(snapshot());
      if (data.error) return "[]";
      return JSON.stringify(
        data.apps.map(function (app) {
          return { id: "app:" + app.id, title: app.name, kind: "app" };
        }).concat(data.capabilities.map(function (capability) {
          return {
            id: "capability:" + capability.namespace,
            title: capability.title,
            kind: "capability",
          };
        }))
      );
    },
  },
  "common.get": {
    summary: "Read one app or capability metadata item by its prefixed id.",
    args: [{ name: "id", required: true }],
    returns: "metadata JSON or typed not-found JSON",
    run: function (args) {
      var id = String(args[0] || "");
      var data = JSON.parse(snapshot());
      if (data.error) return JSON.stringify(data);
      var parts = id.split(":");
      var kind = parts.shift();
      var key = parts.join(":");
      var items = kind === "app" ? data.apps : data.capabilities;
      var field = kind === "app" ? "id" : "namespace";
      for (var index = 0; index < items.length; index += 1) {
        if (items[index][field] === key) return JSON.stringify(items[index]);
      }
      return JSON.stringify({ error: "not_found", id: id });
    },
  },
};
