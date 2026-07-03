// Search Notes backend: hybrid BM25 + vector search over indexed text.
//
// Documents and embeddings are recorded as kv.* events under the reserved
// search projection prefix. Replay rebuilds the index from those events
// without re-running this JS or re-embedding.

var search = ctx.resource.search;
var lm = ctx.resource["local-model"];

var description =
  "Index short notes and run hybrid keyword + semantic search (search + local-model).";

function parseHits(raw) {
  try {
    return JSON.parse(raw);
  } catch (e) {
    return [];
  }
}

var actions = {
  index: {
    summary: "Index a note for search.",
    args: [
      { name: "docId", required: true, summary: "stable note id" },
      {
        name: "text",
        required: true,
        summary: "note body (may be several words)",
      },
    ],
    returns: "confirmation line",
    run: function (args, usage) {
      if (args.length < 2) return usage();
      var docId = args[0];
      var text = args.slice(1).join(" ").trim();
      if (text === "") return usage();
      search.upsert(docId, text);
      return "indexed " + docId;
    },
  },

  embed: {
    summary: "Store a dense embedding for an indexed note.",
    args: [
      { name: "docId", required: true, summary: "note id" },
      {
        name: "text",
        required: true,
        summary: "same text used when indexing (for the encoder)",
      },
    ],
    returns: "confirmation line with vector dimension",
    run: function (args, usage) {
      if (args.length < 2) return usage();
      var docId = args[0];
      var text = args.slice(1).join(" ").trim();
      if (text === "") return usage();
      var vector = JSON.parse(lm.embed(text));
      search.setEmbedding(docId, JSON.stringify(vector));
      return "embedded " + docId + " (" + vector.length + " dims)";
    },
  },

  query: {
    summary: "Hybrid search over indexed notes.",
    args: [
      {
        name: "text",
        required: true,
        summary: "query text (may be several words)",
      },
    ],
    returns: "JSON hit list",
    run: function (args, usage) {
      var text = args.join(" ").trim();
      if (text === "") return usage();
      var queryVec = JSON.parse(lm.embedQuery(text));
      var raw = search.query(
        text,
        JSON.stringify({ limit: 10, queryVec: queryVec })
      );
      return raw;
    },
  },

  bm25: {
    summary: "Keyword-only search over indexed notes.",
    args: [
      {
        name: "text",
        required: true,
        summary: "query text (may be several words)",
      },
    ],
    returns: "JSON hit list",
    run: function (args, usage) {
      var text = args.join(" ").trim();
      if (text === "") return usage();
      return search.bm25(text, JSON.stringify({ limit: 10 }));
    },
  },

  status: {
    summary: "Search index status for this app.",
    args: [],
    returns: "JSON status object",
    run: function () {
      return search.status();
    },
  },

  hits: {
    summary: "Pretty-print hybrid search hits for the CLI.",
    args: [
      {
        name: "text",
        required: true,
        summary: "query text (may be several words)",
      },
    ],
    returns: "newline-separated docId score lines",
    run: function (args, usage) {
      var text = args.join(" ").trim();
      if (text === "") return usage();
      var queryVec = JSON.parse(lm.embedQuery(text));
      var hits = parseHits(
        search.query(text, JSON.stringify({ limit: 10, queryVec: queryVec }))
      );
      if (hits.length === 0) return "(no hits)";
      return hits
        .map(function (hit) {
          return hit.docId + " " + hit.score.toFixed(4) + " " + hit.text;
        })
        .join("\n");
    },
  },
};