# Calendar App App-Only Rubric

Use this rubric outside the model prompt. Do not send it to the model.

Judge only the produced app: catalog entry, UI behavior, app actions if exposed,
and persisted state. Do not score the model's transcript, route, or explanations.

## Required App Evidence

- App exists with the requested id and name.
- App has a UI entry point, not only backend actions.
- First screen is a calendar/product surface, not a landing page.
- UI includes at least month and agenda/list calendar views.
- Events include title, date, time, location, notes, tags, and original/source
  text when created from natural language.
- Natural-language event entry can create at least:
  - one dated single event
  - one timed event with location and note/details
  - one repeated weekly event
- Natural-language custom view can answer, in-app:
  - "Look at my events on Saturdays over the last 5 months, but show only the
    Saturdays that have events"
  - "Show evening events this month grouped by week"
  - "Show meetings with work tags in the next 14 days"
- Custom view output has a title, interpreted filter, grouped results, and an
  empty state.
- Sample data covers enough recent and upcoming dates to make custom views
  meaningful immediately.
- Created events and app state persist after reload/reopen.
- UI has clear validation or error states for ambiguous or unsupported text.

## Scoring

- Pass: the app can be opened and all required evidence works from the UI.
- Partial: app installs and core calendar storage works, but one major natural
  language or custom-view path is incomplete.
- Fail: app is not installed/openable, has no calendar UI, or cannot persist and
  read back events.
