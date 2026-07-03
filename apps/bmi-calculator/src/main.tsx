import {
  type ChangeEvent,
  type CSSProperties,
  useEffect,
  useMemo,
  useState,
} from "react";
import { createRoot } from "react-dom/client";

type CategoryKey = "underweight" | "healthy" | "overweight" | "obesity";

type BmiResult = {
  bmi: number;
  key: CategoryKey;
};

type BackendResult = {
  bmi: number | string;
  category?: string;
};

type RangeStyle = CSSProperties & {
  "--fill": string;
};

type TerraneApi = {
  invoke?: (verb: string, ...args: Array<number | string>) => Promise<string>;
  t?: (key: string, params?: Record<string, unknown>) => string;
  getDir?: () => string;
  onMessages?: (cb: (messages: Record<string, string>) => void) => () => void;
};

declare global {
  interface Window {
    terrane?: TerraneApi;
  }
}

const defaults = {
  height: 170,
  weight: 65,
} as const;

// Translate through the host bundle when present, else the English fallback.
function tr(key: string, fallback: string): string {
  const api = window.terrane;
  if (api && typeof api.t === "function") {
    return api.t(key, { default: fallback });
  }
  return fallback;
}

const CATEGORY_LABELS: Record<CategoryKey, string> = {
  underweight: "Underweight",
  healthy: "Healthy",
  overweight: "Overweight",
  obesity: "Obesity",
};

const STATUS_LABELS = {
  waiting: "Waiting for Terrane",
  noBridge: "Terrane bridge unavailable",
  synced: "Synced with Terrane",
  failed: "Terrane invoke failed",
} as const;

type StatusKey = keyof typeof STATUS_LABELS;

function classify(bmi: number): CategoryKey {
  if (bmi < 18.5) return "underweight";
  if (bmi < 25) return "healthy";
  if (bmi < 30) return "overweight";
  return "obesity";
}

function categoryLabel(key: CategoryKey): string {
  return tr(`bmi-calculator.cat.${key}`, CATEGORY_LABELS[key]);
}

function formatSliderValue(value: number): string {
  return Number.isInteger(value) ? String(value) : value.toFixed(1);
}

function fill(value: number, min: number, max: number): number {
  return ((value - min) / (max - min)) * 100;
}

function rangeStyle(fillPercent: number): RangeStyle {
  return { "--fill": `${fillPercent}%` };
}

function BmiCalculator() {
  const [height, setHeight] = useState(defaults.height);
  const [weight, setWeight] = useState(defaults.weight);
  const [result, setResult] = useState<BmiResult | null>(null);
  const [statusKey, setStatusKey] = useState<StatusKey>("waiting");
  // Bumped when the host pushes/updates the message bundle so the (memoized)
  // labels re-translate on a language change.
  const [, setI18nTick] = useState(0);

  useEffect(() => {
    const api = window.terrane;
    document.documentElement.dir = (api && api.getDir && api.getDir()) || "ltr";
    if (api && typeof api.onMessages === "function") {
      return api.onMessages(() => {
        document.documentElement.dir = (api.getDir && api.getDir()) || "ltr";
        setI18nTick((tick) => tick + 1);
      });
    }
    return undefined;
  }, []);

  useEffect(() => {
    let cancelled = false;

    async function calculate() {
      if (!window.terrane || typeof window.terrane.invoke !== "function") {
        setResult(null);
        setStatusKey("noBridge");
        return;
      }

      try {
        const json = await window.terrane.invoke("calculate", height, weight);
        if (cancelled) return;
        const next = JSON.parse(json) as BackendResult;
        setResult({ bmi: Number(next.bmi), key: classify(Number(next.bmi)) });
        setStatusKey("synced");
      } catch (_error) {
        if (cancelled) return;
        setResult(null);
        setStatusKey("failed");
      }
    }

    calculate();
    return () => {
      cancelled = true;
    };
  }, [height, weight]);

  const heightFill = useMemo(() => fill(height, 120, 220), [height]);
  const weightFill = useMemo(() => fill(weight, 35, 180), [weight]);
  const categoryKey = result ? result.key : "";
  const updateHeight = (event: ChangeEvent<HTMLInputElement>) =>
    setHeight(Number(event.target.value));
  const updateWeight = (event: ChangeEvent<HTMLInputElement>) =>
    setWeight(Number(event.target.value));

  return (
    <main className="bmi-app" data-bmi-app>
      <section className="summary" aria-label={tr("bmi-calculator.aria.summary", "BMI summary")}>
        <div className="intro">
          <p className="eyebrow">{tr("bmi-calculator.eyebrow", "Metric BMI")}</p>
          <h1>{tr("bmi-calculator.title", "BMI Calculator")}</h1>
          <p className="lede">
            {tr("bmi-calculator.lede", "Adjust height and weight to calculate body mass index.")}
          </p>
        </div>
        <output
          className="result"
          id="bmi-output"
          htmlFor="height weight"
          aria-live="polite"
        >
          <span className="result-label">{tr("bmi-calculator.bmi", "BMI")}</span>
          <strong id="bmi-value">
            {result ? result.bmi.toFixed(1) : "--"}
          </strong>
          <span
            className="badge"
            id="bmi-category"
            data-category={categoryKey || undefined}
          >
            {result ? categoryLabel(result.key) : tr("bmi-calculator.enterValues", "Enter values")}
          </span>
        </output>
      </section>

      <section className="controls" aria-label={tr("bmi-calculator.aria.measurements", "Body measurements")}>
        <label className="control" htmlFor="height">
          <span className="control-head">
            <span>{tr("bmi-calculator.height", "Height")}</span>
            <strong>
              <span id="height-value">{formatSliderValue(height)}</span>
              {" cm"}
            </strong>
          </span>
          <input
            id="height"
            name="height"
            type="range"
            min="120"
            max="220"
            step="0.5"
            value={height}
            style={rangeStyle(heightFill)}
            onChange={updateHeight}
          />
          <span className="range-labels">
            <span>120 cm</span>
            <span>220 cm</span>
          </span>
        </label>

        <label className="control" htmlFor="weight">
          <span className="control-head">
            <span>{tr("bmi-calculator.weight", "Weight")}</span>
            <strong>
              <span id="weight-value">{formatSliderValue(weight)}</span>
              {" kg"}
            </strong>
          </span>
          <input
            id="weight"
            name="weight"
            type="range"
            min="35"
            max="180"
            step="0.5"
            value={weight}
            style={rangeStyle(weightFill)}
            onChange={updateWeight}
          />
          <span className="range-labels">
            <span>35 kg</span>
            <span>180 kg</span>
          </span>
        </label>
      </section>

      <section className="scale" aria-label={tr("bmi-calculator.aria.ranges", "BMI ranges")}>
        <div
          data-range="underweight"
          aria-current={categoryKey === "underweight" ? "true" : undefined}
        >
          <span>{categoryLabel("underweight")}</span>
          <strong>{"< 18.5"}</strong>
        </div>
        <div
          data-range="healthy"
          aria-current={categoryKey === "healthy" ? "true" : undefined}
        >
          <span>{categoryLabel("healthy")}</span>
          <strong>18.5-24.9</strong>
        </div>
        <div
          data-range="overweight"
          aria-current={categoryKey === "overweight" ? "true" : undefined}
        >
          <span>{categoryLabel("overweight")}</span>
          <strong>25-29.9</strong>
        </div>
        <div
          data-range="obesity"
          aria-current={categoryKey === "obesity" ? "true" : undefined}
        >
          <span>{categoryLabel("obesity")}</span>
          <strong>30+</strong>
        </div>
      </section>

      <p className="status" id="bridge-status" aria-live="polite">
        {tr(`bmi-calculator.status.${statusKey}`, STATUS_LABELS[statusKey])}
      </p>
    </main>
  );
}

createRoot(document.getElementById("root")!).render(<BmiCalculator />);
