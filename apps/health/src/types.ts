export type Period = "day" | "week" | "month";

export type Settings = {
  provider: "opencode" | "codex" | "claude";
  model: string;
};

export type Dish = {
  food_name: string;
  confidence: number;
  serving_description: string;
  calories_kcal: number;
  protein_g: number;
  carbs_g: number;
  fat_g: number;
  fiber_g: number;
  sugar_g: number;
  sodium_mg: number;
};

export type MealEntry = Dish & {
  id: string;
  blob_name: string;
  note: string;
  provider: "opencode" | "codex" | "claude";
  model: string;
  eaten_at: string;
  reviewed: boolean;
  dishes: Dish[];
  assumptions: string[];
  warnings: string[];
};

export type HealthState = {
  ok: boolean;
  settings: Settings;
  history: MealEntry[];
};

export type MutationResult = {
  ok: boolean;
  error?: string;
  estimate?: MealEntry;
  history?: MealEntry[];
};

export type BlobItem = {
  kind: "blob";
  name: string;
  hash: string;
  size: number;
  mime: "image/jpeg";
  originalName?: string;
  width: number;
  height: number;
};

export type PickResult = {
  cancelled: boolean;
  items: BlobItem[];
};

export type Navigation = {
  route: RouteName;
  segments: string[];
  params: Record<string, string>;
};

export type RouteName =
  | "add"
  | "calendar"
  | "history"
  | "insights"
  | "settings"
  | "meal";

export type RouteState = {
  name: RouteName;
  mealId?: string;
  date?: string;
  period?: Period;
};
