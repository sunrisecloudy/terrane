import { jsx as _jsx, jsxs as _jsxs } from "../../terrane-react-jsx-runtime.js";
import { useEffect, useMemo, useState } from "../../terrane-react.js";
import { createRoot } from "../../terrane-react-dom-client.js";
const defaults = {
    height: 170,
    weight: 65
};
function classify(bmi) {
    if (bmi < 18.5) return {
        key: "underweight",
        label: "Underweight"
    };
    if (bmi < 25) return {
        key: "healthy",
        label: "Healthy"
    };
    if (bmi < 30) return {
        key: "overweight",
        label: "Overweight"
    };
    return {
        key: "obesity",
        label: "Obesity"
    };
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
    return (value - min) / (max - min) * 100;
}
function rangeStyle(fillPercent) {
    return {
        "--fill": `${fillPercent}%`
    };
}
function BmiCalculator() {
    const [height, setHeight] = useState(defaults.height);
    const [weight, setWeight] = useState(defaults.weight);
    const [result, setResult] = useState(()=>localCalculate(defaults.height, defaults.weight));
    const [status, setStatus] = useState("Ready");
    useEffect(()=>{
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
                setResult({
                    bmi: Number(next.bmi),
                    category: next.category || category.label,
                    key: category.key
                });
                setStatus("Synced with Terrane");
            } catch (_error) {
                if (cancelled) return;
                setResult(localCalculate(height, weight));
                setStatus("Using local calculation");
            }
        }
        calculate();
        return ()=>{
            cancelled = true;
        };
    }, [
        height,
        weight
    ]);
    const heightFill = useMemo(()=>fill(height, 120, 220), [
        height
    ]);
    const weightFill = useMemo(()=>fill(weight, 35, 180), [
        weight
    ]);
    const categoryKey = result ? result.key : "";
    const updateHeight = (event)=>setHeight(Number(event.target.value));
    const updateWeight = (event)=>setWeight(Number(event.target.value));
    return /*#__PURE__*/ _jsxs("main", {
        className: "bmi-app",
        "data-bmi-app": true,
        children: [
            /*#__PURE__*/ _jsxs("section", {
                className: "summary",
                "aria-label": "BMI summary",
                children: [
                    /*#__PURE__*/ _jsxs("div", {
                        className: "intro",
                        children: [
                            /*#__PURE__*/ _jsx("p", {
                                className: "eyebrow",
                                children: "Metric BMI"
                            }),
                            /*#__PURE__*/ _jsx("h1", {
                                children: "BMI Calculator"
                            }),
                            /*#__PURE__*/ _jsx("p", {
                                className: "lede",
                                children: "Adjust height and weight to calculate body mass index."
                            })
                        ]
                    }),
                    /*#__PURE__*/ _jsxs("output", {
                        className: "result",
                        id: "bmi-output",
                        htmlFor: "height weight",
                        "aria-live": "polite",
                        children: [
                            /*#__PURE__*/ _jsx("span", {
                                className: "result-label",
                                children: "BMI"
                            }),
                            /*#__PURE__*/ _jsx("strong", {
                                id: "bmi-value",
                                children: result ? result.bmi.toFixed(1) : "--"
                            }),
                            /*#__PURE__*/ _jsx("span", {
                                className: "badge",
                                id: "bmi-category",
                                "data-category": categoryKey || undefined,
                                children: result ? result.category : "Enter values"
                            })
                        ]
                    })
                ]
            }),
            /*#__PURE__*/ _jsxs("section", {
                className: "controls",
                "aria-label": "Body measurements",
                children: [
                    /*#__PURE__*/ _jsxs("label", {
                        className: "control",
                        htmlFor: "height",
                        children: [
                            /*#__PURE__*/ _jsxs("span", {
                                className: "control-head",
                                children: [
                                    /*#__PURE__*/ _jsx("span", {
                                        children: "Height"
                                    }),
                                    /*#__PURE__*/ _jsxs("strong", {
                                        children: [
                                            /*#__PURE__*/ _jsx("span", {
                                                id: "height-value",
                                                children: formatSliderValue(height)
                                            }),
                                            " cm"
                                        ]
                                    })
                                ]
                            }),
                            /*#__PURE__*/ _jsx("input", {
                                id: "height",
                                name: "height",
                                type: "range",
                                min: "120",
                                max: "220",
                                step: "0.5",
                                value: height,
                                style: rangeStyle(heightFill),
                                onChange: updateHeight
                            }),
                            /*#__PURE__*/ _jsxs("span", {
                                className: "range-labels",
                                children: [
                                    /*#__PURE__*/ _jsx("span", {
                                        children: "120 cm"
                                    }),
                                    /*#__PURE__*/ _jsx("span", {
                                        children: "220 cm"
                                    })
                                ]
                            })
                        ]
                    }),
                    /*#__PURE__*/ _jsxs("label", {
                        className: "control",
                        htmlFor: "weight",
                        children: [
                            /*#__PURE__*/ _jsxs("span", {
                                className: "control-head",
                                children: [
                                    /*#__PURE__*/ _jsx("span", {
                                        children: "Weight"
                                    }),
                                    /*#__PURE__*/ _jsxs("strong", {
                                        children: [
                                            /*#__PURE__*/ _jsx("span", {
                                                id: "weight-value",
                                                children: formatSliderValue(weight)
                                            }),
                                            " kg"
                                        ]
                                    })
                                ]
                            }),
                            /*#__PURE__*/ _jsx("input", {
                                id: "weight",
                                name: "weight",
                                type: "range",
                                min: "35",
                                max: "180",
                                step: "0.5",
                                value: weight,
                                style: rangeStyle(weightFill),
                                onChange: updateWeight
                            }),
                            /*#__PURE__*/ _jsxs("span", {
                                className: "range-labels",
                                children: [
                                    /*#__PURE__*/ _jsx("span", {
                                        children: "35 kg"
                                    }),
                                    /*#__PURE__*/ _jsx("span", {
                                        children: "180 kg"
                                    })
                                ]
                            })
                        ]
                    })
                ]
            }),
            /*#__PURE__*/ _jsxs("section", {
                className: "scale",
                "aria-label": "BMI ranges",
                children: [
                    /*#__PURE__*/ _jsxs("div", {
                        "data-range": "underweight",
                        "aria-current": categoryKey === "underweight" ? "true" : undefined,
                        children: [
                            /*#__PURE__*/ _jsx("span", {
                                children: "Underweight"
                            }),
                            /*#__PURE__*/ _jsx("strong", {
                                children: "< 18.5"
                            })
                        ]
                    }),
                    /*#__PURE__*/ _jsxs("div", {
                        "data-range": "healthy",
                        "aria-current": categoryKey === "healthy" ? "true" : undefined,
                        children: [
                            /*#__PURE__*/ _jsx("span", {
                                children: "Healthy"
                            }),
                            /*#__PURE__*/ _jsx("strong", {
                                children: "18.5-24.9"
                            })
                        ]
                    }),
                    /*#__PURE__*/ _jsxs("div", {
                        "data-range": "overweight",
                        "aria-current": categoryKey === "overweight" ? "true" : undefined,
                        children: [
                            /*#__PURE__*/ _jsx("span", {
                                children: "Overweight"
                            }),
                            /*#__PURE__*/ _jsx("strong", {
                                children: "25-29.9"
                            })
                        ]
                    }),
                    /*#__PURE__*/ _jsxs("div", {
                        "data-range": "obesity",
                        "aria-current": categoryKey === "obesity" ? "true" : undefined,
                        children: [
                            /*#__PURE__*/ _jsx("span", {
                                children: "Obesity"
                            }),
                            /*#__PURE__*/ _jsx("strong", {
                                children: "30+"
                            })
                        ]
                    })
                ]
            }),
            /*#__PURE__*/ _jsx("p", {
                className: "status",
                id: "bridge-status",
                "aria-live": "polite",
                children: status
            })
        ]
    });
}
createRoot(document.getElementById("root")).render(/*#__PURE__*/ _jsx(BmiCalculator, {}));
