import { jsx as _jsx, jsxs as _jsxs } from "../../terrane-react-jsx-runtime.js";
import { useEffect, useMemo, useState } from "../../terrane-react.js";
import { createRoot } from "../../terrane-react-dom-client.js";
const defaults = {
    height: 170,
    weight: 65
};
// Translate through the host bundle when present, else the English fallback.
function tr(key, fallback) {
    const api = window.terrane;
    if (api && typeof api.t === "function") {
        return api.t(key, {
            default: fallback
        });
    }
    return fallback;
}
const CATEGORY_LABELS = {
    underweight: "Underweight",
    healthy: "Healthy",
    overweight: "Overweight",
    obesity: "Obesity"
};
const STATUS_LABELS = {
    waiting: "Waiting for Terrane",
    noBridge: "Terrane bridge unavailable",
    synced: "Synced with Terrane",
    failed: "Terrane invoke failed"
};
function classify(bmi) {
    if (bmi < 18.5) return "underweight";
    if (bmi < 25) return "healthy";
    if (bmi < 30) return "overweight";
    return "obesity";
}
function categoryLabel(key) {
    return tr(`bmi-calculator.cat.${key}`, CATEGORY_LABELS[key]);
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
    const [result, setResult] = useState(null);
    const [statusKey, setStatusKey] = useState("waiting");
    // Bumped when the host pushes/updates the message bundle so the (memoized)
    // labels re-translate on a language change.
    const [, setI18nTick] = useState(0);
    useEffect(()=>{
        const api = window.terrane;
        document.documentElement.dir = api && api.getDir && api.getDir() || "ltr";
        if (api && typeof api.onMessages === "function") {
            return api.onMessages(()=>{
                document.documentElement.dir = api.getDir && api.getDir() || "ltr";
                setI18nTick((tick)=>tick + 1);
            });
        }
        return undefined;
    }, []);
    useEffect(()=>{
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
                const next = JSON.parse(json);
                setResult({
                    bmi: Number(next.bmi),
                    key: classify(Number(next.bmi))
                });
                setStatusKey("synced");
            } catch (_error) {
                if (cancelled) return;
                setResult(null);
                setStatusKey("failed");
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
                "aria-label": tr("bmi-calculator.aria.summary", "BMI summary"),
                children: [
                    /*#__PURE__*/ _jsxs("div", {
                        className: "intro",
                        children: [
                            /*#__PURE__*/ _jsx("p", {
                                className: "eyebrow",
                                children: tr("bmi-calculator.eyebrow", "Metric BMI")
                            }),
                            /*#__PURE__*/ _jsx("h1", {
                                children: tr("bmi-calculator.title", "BMI Calculator")
                            }),
                            /*#__PURE__*/ _jsx("p", {
                                className: "lede",
                                children: tr("bmi-calculator.lede", "Adjust height and weight to calculate body mass index.")
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
                                children: tr("bmi-calculator.bmi", "BMI")
                            }),
                            /*#__PURE__*/ _jsx("strong", {
                                id: "bmi-value",
                                children: result ? result.bmi.toFixed(1) : "--"
                            }),
                            /*#__PURE__*/ _jsx("span", {
                                className: "badge",
                                id: "bmi-category",
                                "data-category": categoryKey || undefined,
                                children: result ? categoryLabel(result.key) : tr("bmi-calculator.enterValues", "Enter values")
                            })
                        ]
                    })
                ]
            }),
            /*#__PURE__*/ _jsxs("section", {
                className: "controls",
                "aria-label": tr("bmi-calculator.aria.measurements", "Body measurements"),
                children: [
                    /*#__PURE__*/ _jsxs("label", {
                        className: "control",
                        htmlFor: "height",
                        children: [
                            /*#__PURE__*/ _jsxs("span", {
                                className: "control-head",
                                children: [
                                    /*#__PURE__*/ _jsx("span", {
                                        children: tr("bmi-calculator.height", "Height")
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
                                        children: tr("bmi-calculator.weight", "Weight")
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
                "aria-label": tr("bmi-calculator.aria.ranges", "BMI ranges"),
                children: [
                    /*#__PURE__*/ _jsxs("div", {
                        "data-range": "underweight",
                        "aria-current": categoryKey === "underweight" ? "true" : undefined,
                        children: [
                            /*#__PURE__*/ _jsx("span", {
                                children: categoryLabel("underweight")
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
                                children: categoryLabel("healthy")
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
                                children: categoryLabel("overweight")
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
                                children: categoryLabel("obesity")
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
                children: tr(`bmi-calculator.status.${statusKey}`, STATUS_LABELS[statusKey])
            })
        ]
    });
}
createRoot(document.getElementById("root")).render(/*#__PURE__*/ _jsx(BmiCalculator, {}));
