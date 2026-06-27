import {
  type ChangeEvent,
  type CSSProperties,
  useEffect,
  useMemo,
  useState,
} from "react";
import { createRoot } from "react-dom/client";

type CategoryKey = "underweight" | "healthy" | "overweight" | "obesity";

type Category = {
  key: CategoryKey;
  label: string;
};

type BmiResult = {
  bmi: number;
  category: string;
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

function classify(bmi: number): Category {
  if (bmi < 18.5) return { key: "underweight", label: "Underweight" };
  if (bmi < 25) return { key: "healthy", label: "Healthy" };
  if (bmi < 30) return { key: "overweight", label: "Overweight" };
  return { key: "obesity", label: "Obesity" };
}

function localCalculate(heightCm: number, weightKg: number): BmiResult | null {
  const height = Number(heightCm);
  const weight = Number(weightKg);
  if (!(height > 0) || !(weight > 0)) return null;

  const meters = height / 100;
  const raw = weight / (meters * meters);
  const category = classify(raw);
  return {
    bmi: Math.round(raw * 10) / 10,
    category: category.label,
    key: category.key,
  };
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
  const [result, setResult] = useState<BmiResult | null>(() =>
    localCalculate(defaults.height, defaults.weight)
  );
  const [status, setStatus] = useState("Ready");

  useEffect(() => {
    let cancelled = false;

    async function calculate() {
      if (!window.terrane || typeof window.terrane.invoke !== "function") {
        setResult(localCalculate(height, weight));
        setStatus("Ready");
        return;
      }

      try {
        const json = await window.terrane.invoke("calculate", height, weight);
        if (cancelled) return;
        const next = JSON.parse(json) as BackendResult;
        const category = classify(Number(next.bmi));
        setResult({
          bmi: Number(next.bmi),
          category: next.category || category.label,
          key: category.key,
        });
        setStatus("Synced with Terrane");
      } catch (_error) {
        if (cancelled) return;
        setResult(localCalculate(height, weight));
        setStatus("Using local calculation");
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
      <section className="summary" aria-label="BMI summary">
        <div className="intro">
          <p className="eyebrow">Metric BMI</p>
          <h1>BMI Calculator</h1>
          <p className="lede">
            Adjust height and weight to calculate body mass index.
          </p>
        </div>
        <output
          className="result"
          id="bmi-output"
          htmlFor="height weight"
          aria-live="polite"
        >
          <span className="result-label">BMI</span>
          <strong id="bmi-value">
            {result ? result.bmi.toFixed(1) : "--"}
          </strong>
          <span
            className="badge"
            id="bmi-category"
            data-category={categoryKey || undefined}
          >
            {result ? result.category : "Enter values"}
          </span>
        </output>
      </section>

      <section className="controls" aria-label="Body measurements">
        <label className="control" htmlFor="height">
          <span className="control-head">
            <span>Height</span>
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
            <span>Weight</span>
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

      <section className="scale" aria-label="BMI ranges">
        <div
          data-range="underweight"
          aria-current={categoryKey === "underweight" ? "true" : undefined}
        >
          <span>Underweight</span>
          <strong>{"< 18.5"}</strong>
        </div>
        <div
          data-range="healthy"
          aria-current={categoryKey === "healthy" ? "true" : undefined}
        >
          <span>Healthy</span>
          <strong>18.5-24.9</strong>
        </div>
        <div
          data-range="overweight"
          aria-current={categoryKey === "overweight" ? "true" : undefined}
        >
          <span>Overweight</span>
          <strong>25-29.9</strong>
        </div>
        <div
          data-range="obesity"
          aria-current={categoryKey === "obesity" ? "true" : undefined}
        >
          <span>Obesity</span>
          <strong>30+</strong>
        </div>
      </section>

      <p className="status" id="bridge-status" aria-live="polite">{status}</p>
    </main>
  );
}

createRoot(document.getElementById("root")!).render(<BmiCalculator />);
