import { useEffect, useState } from "react";

import { Header } from "./components/Header";
import { navigate, navigationToRoute, parseRouteHash } from "./router";
import { AddMealRoute } from "./routes/AddMealRoute";
import { CalendarRoute } from "./routes/CalendarRoute";
import { HistoryRoute } from "./routes/HistoryRoute";
import { InsightsRoute } from "./routes/InsightsRoute";
import { MealRoute } from "./routes/MealRoute";
import { SettingsRoute } from "./routes/SettingsRoute";
import { invokeJson, publishSidebar } from "./terrane";
import type {
  HealthState,
  MealEntry,
  Navigation,
  RouteState,
  Settings,
} from "./types";

const defaultSettings: Settings = {
  provider: "opencode",
  model: "opencode-go/kimi-k2.6",
};

export function App() {
  const [route, setRoute] = useState<RouteState>(() => parseRouteHash());
  const [history, setHistory] = useState<MealEntry[]>([]);
  const [settings, setSettings] = useState<Settings>(defaultSettings);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState("");
  const routeRefreshKey =
    route.name === "meal" ? `${route.name}:${route.mealId}` : route.name;

  useEffect(() => {
    const update = () => setRoute(parseRouteHash());
    window.addEventListener("hashchange", update);
    if (!window.location.hash) navigate({ name: "add" });
    return () => window.removeEventListener("hashchange", update);
  }, []);

  useEffect(() => {
    const selected = route.name === "meal" ? "history" : route.name;
    publishSidebar(selected, (id) => {
      if (
        id === "add" ||
        id === "calendar" ||
        id === "history" ||
        id === "insights" ||
        id === "settings"
      ) {
        navigate({ name: id });
      }
    });
  }, [route.name]);

  useEffect(() => {
    let cancelled = false;
    setLoading(true);
    invokeJson<HealthState>("state")
      .then((state) => {
        if (cancelled) return;
        setHistory(state.history || []);
        setSettings(state.settings || defaultSettings);
      })
      .catch((caught) => {
        if (!cancelled) {
          setError(caught instanceof Error ? caught.message : "Could not load Health.");
        }
      })
      .finally(() => {
        if (!cancelled) setLoading(false);
      });
    return () => {
      cancelled = true;
    };
  }, [routeRefreshKey]);

  useEffect(() => {
    let cancelled = false;
    let timer = 0;
    async function consume() {
      try {
        const result = await invokeJson<{ ok: boolean; navigation: Navigation | null }>(
          "navigation.consume",
        );
        if (!cancelled && result.navigation) {
          navigate(navigationToRoute(result.navigation));
        }
      } catch (_error) {
        // A route poll is best-effort; state actions still surface real failures.
      }
      if (!cancelled) timer = window.setTimeout(consume, 750);
    }
    consume();
    return () => {
      cancelled = true;
      window.clearTimeout(timer);
    };
  }, []);

  function openMeal(entry: MealEntry) {
    navigate({ name: "meal", mealId: entry.id });
  }

  function updateHistory(next: MealEntry[]) {
    setHistory(next);
  }

  function routeContent() {
    if (loading) {
      return <section className="panel route-empty"><strong>Loading Health…</strong></section>;
    }
    switch (route.name) {
      case "calendar":
        return (
          <CalendarRoute
            history={history}
            initialDate={route.date}
            initialPeriod={route.period}
            onOpen={openMeal}
          />
        );
      case "history":
        return <HistoryRoute history={history} onOpen={openMeal} />;
      case "insights":
        return (
          <InsightsRoute
            history={history}
            initialDate={route.date}
            initialPeriod={route.period}
            onOpen={openMeal}
          />
        );
      case "settings":
        return <SettingsRoute settings={settings} onSettings={setSettings} />;
      case "meal":
        return (
          <MealRoute
            entry={history.find((entry) => entry.id === route.mealId)}
            onChanged={(entry, next) => {
              updateHistory(next);
              navigate({ name: "meal", mealId: entry.id });
            }}
            onDeleted={(next) => {
              updateHistory(next);
              navigate({ name: "history" });
            }}
          />
        );
      default:
        return (
          <AddMealRoute
            settings={settings}
            onSettings={setSettings}
            onEstimated={(entry, next) => {
              updateHistory(next);
              navigate({ name: "meal", mealId: entry.id });
            }}
          />
        );
    }
  }

  return (
    <div className="shell">
      <Header />
      {error ? <div className="error visible global-error" role="alert">{error}</div> : null}
      <main>{routeContent()}</main>
    </div>
  );
}
