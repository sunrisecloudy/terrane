// Scribe backend for Terrane — ambient speech-to-text transcript UI.
//
// The app holds no audio and never captures. It reads the recorded transcript
// (finalized segments the host edge dispatched as trusted `stt.segment.append`)
// and records user selections via `stt.select`. Each selection is exactly one
// `stt.selection.made` event, so Option-A replay rebuilds selections by folding
// events, never by re-running this JS.
//
// Capture is host-owned: starting/stopping a session is a host decision (the
// app can request a stop via `stt.stop`, but opening a session and producing
// segments is the edge's job). The UI polls the folded transcript and renders
// selectable chips; the user highlights a slice and picks a sink.
//
// The app is a `description` + an `actions` table; the runtime synthesizes
// `handle`, usage, and unknown-verb help. The UI calls these verbs via
// window.terrane.invoke.

var stt = ctx.resource["stt"];

// Defensive JSON parse: reads return JSON strings (or null when ungranted).
function readJson(raw) {
  if (raw == null || raw === "") return [];
  try {
    return JSON.parse(String(raw));
  } catch (e) {
    return [];
  }
}

var description =
  "Ambient speech-to-text: read the live transcript and record selections.";

var actions = {
  state: {
    summary:
      "Full UI snapshot: live session, transcript, and recent selections.",
    args: [],
    returns: "a JSON object { session, segments, selections }",
    run: function () {
      var sessions = readJson(stt.sessions());
      var live = sessions.filter(function (s) {
        return s.status === "open";
      })[0];
      var sessionId = live ? live.sessionId : sessions[0] ? sessions[0].sessionId : "";
      var segments = sessionId ? readJson(stt.segments(sessionId)) : [];
      var selections = sessionId ? readJson(stt.selections(sessionId)) : [];
      return JSON.stringify({
        session: live || null,
        sessionId: sessionId,
        segments: segments,
        selections: selections,
      });
    },
  },

  sessions: {
    summary: "This app's capture sessions (open first).",
    args: [],
    returns: "the stt.sessions() JSON verbatim",
    run: function () {
      return String(stt.sessions());
    },
  },

  transcript: {
    summary: "The finalized transcript for a session, as plain text.",
    args: [
      {
        name: "sessionId",
        required: true,
        summary: "the session id (from `sessions`)",
      },
    ],
    returns: "segment texts joined by spaces, oldest first",
    run: function (args, usage) {
      var sessionId = args[0];
      if (!sessionId) return usage();
      var segs = readJson(stt.segments(sessionId));
      return segs
        .map(function (s) {
          return s.text;
        })
        .join(" ");
    },
  },

  segments: {
    summary: "The finalized segments for a session, as JSON.",
    args: [
      {
        name: "sessionId",
        required: true,
        summary: "the session id",
      },
    ],
    returns: "the stt.segments(sessionId) JSON verbatim",
    run: function (args, usage) {
      var sessionId = args[0];
      if (!sessionId) return usage();
      return String(stt.segments(sessionId));
    },
  },

  select: {
    summary:
      "Record a user-chosen transcript slice. The slice text is re-derived by the core.",
    args: [
      {
        name: "sessionId",
        required: true,
        summary: "the session id",
      },
      {
        name: "fromSeq",
        required: true,
        summary: "inclusive start segment seq",
      },
      {
        name: "toSeq",
        required: true,
        summary: "inclusive end segment seq",
      },
      {
        name: "sink",
        required: true,
        summary: "clipboard | field | app:<id> | note",
      },
    ],
    returns: "the re-derived slice text",
    run: function (args, usage) {
      if (args.length < 4) return usage();
      return String(stt.select(args[0], args[1], args[2], args[3]));
    },
  },

  selections: {
    summary: "Recorded selections for a session, as JSON.",
    args: [
      {
        name: "sessionId",
        required: true,
        summary: "the session id",
      },
    ],
    returns: "the stt.selections(sessionId) JSON verbatim",
    run: function (args, usage) {
      var sessionId = args[0];
      if (!sessionId) return usage();
      return String(stt.selections(sessionId));
    },
  },

  stop: {
    summary: "Stop the live session (records reason `stopped`).",
    args: [
      {
        name: "sessionId",
        required: true,
        summary: "the session id",
      },
    ],
    returns: "`ok` once the close is recorded",
    run: function (args, usage) {
      var sessionId = args[0];
      if (!sessionId) return usage();
      return String(stt.stop(sessionId));
    },
  },
};
