# Calendar App Product Prompt

Build a complete calendar app named "{{APP_NAME}}" with id "{{APP_ID}}".

Please do not inspect repository source files, list directories, run shell
commands, or browse/search the web. Build the app directly through the local
app-building surface available to you.

The app should feel like a real personal calendar, not a demo page. It needs a
usable visual calendar view and a smart text box where I can type ordinary
sentences to create events. It should also let me type natural-language requests
to create custom views of my events.

Core experience:

- A first-screen calendar UI with month and agenda/list views.
- Event cards that show title, date, start/end time, location, notes, tags, and
  source text.
- A natural-language event input box. Examples it should handle:
  - "Dinner with Nok next Friday at 7pm at Siam Paragon"
  - "Doctor appointment on July 8 from 10:30 to 11:15, bring insurance card"
  - "Team planning every Monday at 9am for the next 4 weeks"
- A separate natural-language view box for questions like:
  - "Look at my events on Saturdays over the last 5 months, but show only the
    Saturdays that have events"
  - "Show evening events this month grouped by week"
  - "Show meetings with work tags in the next 14 days"
- Custom views should render inside the app, with a readable title, the
  interpreted filter, grouped results, and an empty state when nothing matches.
- Include enough realistic sample events that the custom-view box is useful
  immediately: spread them across at least the past six months and the
  upcoming weeks, covering weekdays and weekends alike.
- Changes should persist when I leave and reopen the app.
- Keep the UI polished: dense enough to manage a calendar, clear controls,
  good empty/loading/error states, and no marketing-style landing page.

When finished, leave the installed app ready to open and use. The important
result is the app itself.
