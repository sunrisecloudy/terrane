import {
  type ChangeEvent,
  type CSSProperties,
  useEffect,
  useMemo,
  useRef,
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

type BackendState = {
  height: number | string;
  weight: number | string;
  result?: BackendResult | null;
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

function formatSliderValue(value: number): string {
  return Number.isInteger(value) ? String(value) : value.toFixed(1);
}

function fill(value: number, min: number, max: number): number {
  return ((value - min) / (max - min)) * 100;
}

function rangeStyle(fillPercent: number): RangeStyle {
  return { "--fill": `${fillPercent}%` };
}

function toBmiResult(result: BackendResult | null | undefined): BmiResult | null {
  if (!result) return null;
  const bmi = Number(result.bmi);
  if (!Number.isFinite(bmi)) return null;
  const category = classify(bmi);
  return {
    bmi,
    category: result.category || category.label,
    key: category.key,
  };
}

function BmiCalculator() {
  const [height, setHeight] = useState(defaults.height);
  const [weight, setWeight] = useState(defaults.weight);
  const [result, setResult] = useState<BmiResult | null>(null);
  const [status, setStatus] = useState("Waiting for Terrane");
  const syncSeq = useRef(0);

  useEffect(() => {
    let cancelled = false;

    async function loadState() {
      if (!window.terrane || typeof window.terrane.invoke !== "function") {
        setResult(null);
        setStatus("Terrane bridge unavailable");
        return;
      }

      try {
        const json = await window.terrane.invoke("state");
        if (cancelled) return;
        applyBackendState(JSON.parse(json) as BackendState);
        setStatus("Synced with Terrane");
      } catch (_error) {
        if (cancelled) return;
        setResult(null);
        setStatus("Terrane invoke failed");
      }
    }

    loadState();
    return () => {
      cancelled = true;
    };
  }, []);

  function applyBackendState(next: BackendState) {
    const nextHeight = Number(next.height);
    const nextWeight = Number(next.weight);
    if (Number.isFinite(nextHeight)) setHeight(nextHeight);
    if (Number.isFinite(nextWeight)) setWeight(nextWeight);
    setResult(toBmiResult(next.result));
  }

  async function persistMeasurement(verb: "set_height" | "set_weight", value: number) {
    if (!window.terrane || typeof window.terrane.invoke !== "function") {
      setStatus("Terrane bridge unavailable");
      return;
    }

    const seq = syncSeq.current + 1;
    syncSeq.current = seq;
    setStatus("Saving to Terrane");
    try {
      const json = await window.terrane.invoke(verb, value);
      if (syncSeq.current !== seq) return;
      applyBackendState(JSON.parse(json) as BackendState);
      setStatus("Synced with Terrane");
    } catch (_error) {
      if (syncSeq.current !== seq) return;
      setStatus("Terrane invoke failed");
    }
  }

  const heightFill = useMemo(() => fill(height, 120, 220), [height]);
  const weightFill = useMemo(() => fill(weight, 35, 180), [weight]);
  const categoryKey = result ? result.key : "";
  const updateHeight = (event: ChangeEvent<HTMLInputElement>) => {
    const nextHeight = Number(event.target.value);
    setHeight(nextHeight);
    void persistMeasurement("set_height", nextHeight);
  };
  const updateWeight = (event: ChangeEvent<HTMLInputElement>) => {
    const nextWeight = Number(event.target.value);
    setWeight(nextWeight);
    void persistMeasurement("set_weight", nextWeight);
  };

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
