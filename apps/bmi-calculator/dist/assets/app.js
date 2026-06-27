const { useEffect, useMemo, useState } = React;

const defaults = {
  height: 170,
  weight: 65
};

function classify(bmi) {
  if (bmi < 18.5) return { key: "underweight", label: "Underweight" };
  if (bmi < 25) return { key: "healthy", label: "Healthy" };
  if (bmi < 30) return { key: "overweight", label: "Overweight" };
  return { key: "obesity", label: "Obesity" };
}

function localCalculate(heightCm, weightKg) {
  const height = Number(heightCm);
  const weight = Number(weightKg);
  if (!(height > 0) || !(weight > 0)) return null;

  const meters = height / 100;
  const raw = weight / (meters * meters);
  const category = classify(raw);
  return {
    bmi: Math.round(raw * 10) / 10,
    category: category.label,
    key: category.key
  };
}

function formatSliderValue(value) {
  return Number.isInteger(value) ? String(value) : value.toFixed(1);
}

function fill(value, min, max) {
  return ((value - min) / (max - min)) * 100;
}

function BmiCalculator() {
  const [height, setHeight] = useState(defaults.height);
  const [weight, setWeight] = useState(defaults.weight);
  const [result, setResult] = useState(() => localCalculate(defaults.height, defaults.weight));
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
        const next = JSON.parse(json);
        const category = classify(Number(next.bmi));
        setResult({ bmi: Number(next.bmi), category: next.category || category.label, key: category.key });
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

  return (
    React.createElement("main", { className: "bmi-app", "data-bmi-app": true }, React.createElement("section", { className: "summary", "aria-label": "BMI summary" }, React.createElement("div", { className: "intro" }, React.createElement("p", { className: "eyebrow" }, "Metric BMI"), React.createElement("h1", null, "BMI Calculator"), React.createElement("p", { className: "lede" }, "Adjust height and weight to calculate body mass index.")), React.createElement("output", { className: "result", id: "bmi-output", htmlFor: "height weight", "aria-live": "polite" }, React.createElement("span", { className: "result-label" }, "BMI"), React.createElement("strong", { id: "bmi-value" }, result ? result.bmi.toFixed(1) : "--"), React.createElement("span", { className: "badge", id: "bmi-category", "data-category": categoryKey || undefined }, result ? result.category : "Enter values"))), React.createElement("section", { className: "controls", "aria-label": "Body measurements" }, React.createElement("label", { className: "control", htmlFor: "height" }, React.createElement("span", { className: "control-head" }, React.createElement("span", null, "Height"), React.createElement("strong", null, React.createElement("span", { id: "height-value" }, formatSliderValue(height)), " cm")), React.createElement("input", { id: "height", name: "height", type: "range", min: "120", max: "220", step: "0.5", value: height, style: { "--fill": `${heightFill}%` }, onChange: (event) => setHeight(Number(event.target.value)) }), React.createElement("span", { className: "range-labels" }, React.createElement("span", null, "120 cm"), React.createElement("span", null, "220 cm"))), React.createElement("label", { className: "control", htmlFor: "weight" }, React.createElement("span", { className: "control-head" }, React.createElement("span", null, "Weight"), React.createElement("strong", null, React.createElement("span", { id: "weight-value" }, formatSliderValue(weight)), " kg")), React.createElement("input", { id: "weight", name: "weight", type: "range", min: "35", max: "180", step: "0.5", value: weight, style: { "--fill": `${weightFill}%` }, onChange: (event) => setWeight(Number(event.target.value)) }), React.createElement("span", { className: "range-labels" }, React.createElement("span", null, "35 kg"), React.createElement("span", null, "180 kg")))), React.createElement("section", { className: "scale", "aria-label": "BMI ranges" }, React.createElement("div", { "data-range": "underweight", "aria-current": categoryKey === "underweight" ? "true" : undefined }, React.createElement("span", null, "Underweight"), React.createElement("strong", null, "< 18.5")), React.createElement("div", { "data-range": "healthy", "aria-current": categoryKey === "healthy" ? "true" : undefined }, React.createElement("span", null, "Healthy"), React.createElement("strong", null, "18.5-24.9")), React.createElement("div", { "data-range": "overweight", "aria-current": categoryKey === "overweight" ? "true" : undefined }, React.createElement("span", null, "Overweight"), React.createElement("strong", null, "25-29.9")), React.createElement("div", { "data-range": "obesity", "aria-current": categoryKey === "obesity" ? "true" : undefined }, React.createElement("span", null, "Obesity"), React.createElement("strong", null, "30+"))), React.createElement("p", { className: "status", id: "bridge-status", "aria-live": "polite" }, status))
  );
}

ReactDOM.createRoot(document.getElementById("root")).render(React.createElement(BmiCalculator, null));
