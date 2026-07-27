import type { Navigation, Period, RouteName, RouteState } from "./types";

const pageRoutes: RouteName[] = [
  "add",
  "calendar",
  "history",
  "insights",
  "settings",
];

function validDate(value: string | null): string | undefined {
  return value && /^\d{4}-\d{2}-\d{2}$/.test(value) ? value : undefined;
}

function validPeriod(value: string | null): Period | undefined {
  return value === "day" || value === "week" || value === "month"
    ? value
    : undefined;
}

export function parseRouteHash(hash = window.location.hash): RouteState {
  const raw = hash.replace(/^#\/?/, "");
  const [path, query = ""] = raw.split("?", 2);
  const segments = path.split("/").filter(Boolean).map(decodeURIComponent);
  const params = new URLSearchParams(query);
  const name = segments[0] as RouteName;
  if (name === "meal" && /^[A-Za-z0-9._-]{1,128}$/.test(segments[1] || "")) {
    return { name, mealId: segments[1] };
  }
  if (pageRoutes.includes(name)) {
    return {
      name,
      date: validDate(params.get("date")),
      period: validPeriod(params.get("period")),
    };
  }
  return { name: "add" };
}

export function routeHref(route: RouteState): string {
  const path =
    route.name === "meal" && route.mealId
      ? `/meal/${encodeURIComponent(route.mealId)}`
      : `/${route.name}`;
  const params = new URLSearchParams();
  if (route.date) params.set("date", route.date);
  if (route.period) params.set("period", route.period);
  const query = params.toString();
  return `#${path}${query ? `?${query}` : ""}`;
}

export function navigate(route: RouteState): void {
  const href = routeHref(route);
  if (window.location.hash === href) {
    window.dispatchEvent(new HashChangeEvent("hashchange"));
  } else {
    window.location.hash = href;
  }
}

export function navigationToRoute(value: Navigation): RouteState {
  if (value.route === "meal") {
    return { name: "meal", mealId: value.segments[0] };
  }
  return {
    name: value.route,
    date: validDate(value.params.date || null),
    period: validPeriod(value.params.period || null),
  };
}
