(function () {
  var h = React.createElement;

  function classify(bmi) {
    if (!isFinite(bmi) || bmi <= 0) return "";
    if (bmi < 18.5) return "Underweight";
    if (bmi < 25) return "Healthy";
    if (bmi < 30) return "Overweight";
    return "Obesity";
  }

  function App() {
    var heightState = React.useState("170");
    var weightState = React.useState("65");
    var height = heightState[0];
    var setHeight = heightState[1];
    var weight = weightState[0];
    var setWeight = weightState[1];
    var heightNumber = parseFloat(height);
    var weightNumber = parseFloat(weight);
    var bmi = heightNumber > 0 && weightNumber > 0
      ? weightNumber / Math.pow(heightNumber / 100, 2)
      : 0;
    var rounded = bmi > 0 ? (Math.round(bmi * 10) / 10).toFixed(1) : "--";
    var category = classify(bmi);

    return h("main", { className: "bmi-app" },
      h("style", null, css()),
      h("section", { className: "hero" },
        h("div", null,
          h("h1", null, "BMI Calculator"),
          h("p", null, "A small React app running inside the Terrane web host.")
        ),
        h("div", { className: "result", "aria-live": "polite" },
          h("span", { className: "result-label" }, "BMI"),
          h("strong", null, rounded),
          h("span", { className: "badge" }, category || "Enter values")
        )
      ),
      h("section", { className: "controls", "aria-label": "BMI inputs" },
        slider("Height", height, "120", "220", "0.5", "cm", setHeight),
        slider("Weight", weight, "35", "180", "0.5", "kg", setWeight)
      ),
      h("section", { className: "scale", "aria-label": "BMI ranges" },
        range("Underweight", "< 18.5"),
        range("Healthy", "18.5-24.9"),
        range("Overweight", "25-29.9"),
        range("Obesity", "30+")
      )
    );
  }

  function range(name, value) {
    return h("div", null, h("span", null, name), h("strong", null, value));
  }

  function slider(label, value, min, max, step, unit, onChange) {
    var id = label.toLowerCase() + "-slider";
    var percent = ((parseFloat(value) - parseFloat(min)) / (parseFloat(max) - parseFloat(min))) * 100;
    return h("label", { className: "slider-control", htmlFor: id },
      h("span", { className: "slider-top" },
        h("span", null, label),
        h("strong", null, displayValue(value) + " " + unit)
      ),
      h("input", {
        id: id,
        type: "range",
        min: min,
        max: max,
        step: step,
        value: value,
        style: { "--fill": percent + "%" },
        onInput: function (event) { onChange(event.target.value); }
      }),
      h("span", { className: "slider-range" },
        h("small", null, min + " " + unit),
        h("small", null, max + " " + unit)
      )
    );
  }

  function displayValue(value) {
    var number = parseFloat(value);
    return number % 1 === 0 ? String(number) : number.toFixed(1);
  }

  function css() {
    return ".bmi-app{min-height:100vh;display:grid;place-content:center;gap:18px;padding:24px;background:Canvas;color:CanvasText;font:14px -apple-system,BlinkMacSystemFont,'Segoe UI',sans-serif}.hero,.controls,.scale{width:min(560px,calc(100vw - 32px));border:1px solid color-mix(in srgb,CanvasText 14%,transparent);border-radius:8px;background:color-mix(in srgb,Canvas 96%,CanvasText 4%)}.hero{display:grid;grid-template-columns:1fr 150px;gap:18px;align-items:center;padding:22px}.hero h1{margin:0 0 8px;font-size:26px;letter-spacing:0}.hero p{margin:0;color:color-mix(in srgb,CanvasText 65%,transparent);line-height:1.45}.result{display:grid;gap:4px;justify-items:end}.result-label{font-size:12px;text-transform:uppercase;color:color-mix(in srgb,CanvasText 60%,transparent);font-weight:700}.result strong{font-size:44px;line-height:1;transition:color .16s ease}.badge{padding:5px 9px;border-radius:999px;background:#0071e3;color:white;font-weight:700;transition:background .16s ease}.controls{display:grid;gap:18px;padding:16px}.slider-control{display:grid;gap:10px}.slider-top,.slider-range{display:flex;align-items:baseline;justify-content:space-between;gap:12px}.slider-top span{font-weight:700}.slider-top strong{font-size:20px;font-variant-numeric:tabular-nums}.slider-range small{color:color-mix(in srgb,CanvasText 58%,transparent)}input[type=range]{--fill:50%;width:100%;height:28px;appearance:none;background:transparent;cursor:pointer}input[type=range]::-webkit-slider-runnable-track{height:8px;border-radius:999px;background:linear-gradient(90deg,#0071e3 0%,#0071e3 var(--fill),color-mix(in srgb,CanvasText 14%,transparent) var(--fill),color-mix(in srgb,CanvasText 14%,transparent) 100%)}input[type=range]::-webkit-slider-thumb{appearance:none;width:22px;height:22px;margin-top:-7px;border:3px solid Canvas;border-radius:50%;background:#0071e3;box-shadow:0 2px 8px color-mix(in srgb,CanvasText 24%,transparent);transition:transform .12s ease,box-shadow .12s ease}input[type=range]:active::-webkit-slider-thumb{transform:scale(1.12);box-shadow:0 3px 12px color-mix(in srgb,#0071e3 45%,transparent)}input[type=range]::-moz-range-track{height:8px;border-radius:999px;background:color-mix(in srgb,CanvasText 14%,transparent)}input[type=range]::-moz-range-progress{height:8px;border-radius:999px;background:#0071e3}input[type=range]::-moz-range-thumb{width:18px;height:18px;border:3px solid Canvas;border-radius:50%;background:#0071e3;box-shadow:0 2px 8px color-mix(in srgb,CanvasText 24%,transparent)}.scale{display:grid;grid-template-columns:repeat(4,1fr);gap:1px;overflow:hidden}.scale div{display:grid;gap:4px;padding:12px;background:Canvas}.scale span{color:color-mix(in srgb,CanvasText 62%,transparent);font-size:12px}.scale strong{font-size:13px}@media(max-width:640px){.hero{grid-template-columns:1fr}.result{justify-items:start}.scale{grid-template-columns:1fr}}";
  }

  ReactDOM.createRoot(document.getElementById("root")).render(h(App));
})();
