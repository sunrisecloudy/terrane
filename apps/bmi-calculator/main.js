function calculate(heightCm, weightKg) {
  var height = parseFloat(heightCm);
  var weight = parseFloat(weightKg);
  if (!(height > 0) || !(weight > 0)) return null;
  var meters = height / 100;
  var bmi = weight / (meters * meters);
  var category = "Healthy";
  if (bmi < 18.5) category = "Underweight";
  else if (bmi < 25) category = "Healthy";
  else if (bmi < 30) category = "Overweight";
  else category = "Obesity";
  return {
    bmi: Math.round(bmi * 10) / 10,
    category: category
  };
}

var kv = ctx.resource.kv;
var STATE_KEY = "state";
var DEFAULT_STATE = {
  height: 170,
  weight: 65
};

function readState() {
  var raw = kv.get(STATE_KEY);
  if (raw == null || raw === "") return cloneState(DEFAULT_STATE);
  try {
    var parsed = JSON.parse(raw);
    return normalizeState(parsed) || cloneState(DEFAULT_STATE);
  } catch (_e) {
    return cloneState(DEFAULT_STATE);
  }
}

function writeState(state) {
  kv.set(STATE_KEY, JSON.stringify(state));
}

function cloneState(state) {
  return {
    height: state.height,
    weight: state.weight
  };
}

function normalizeState(value) {
  if (value == null || typeof value !== "object") return null;
  var height = parseFloat(value.height);
  var weight = parseFloat(value.weight);
  if (!(height > 0) || !(weight > 0)) return null;
  return {
    height: Math.round(height * 10) / 10,
    weight: Math.round(weight * 10) / 10
  };
}

function withResult(state) {
  return {
    height: state.height,
    weight: state.weight,
    result: calculate(state.height, state.weight)
  };
}

function setMeasurement(key, value) {
  var numeric = parseFloat(value);
  if (!(numeric > 0)) return null;
  var state = readState();
  state[key] = Math.round(numeric * 10) / 10;
  writeState(state);
  return withResult(state);
}

var description = "A Rust-built React BMI calculator with kv-backed measurements.";

var actions = {
  state: {
    summary: "Return saved height, weight, and BMI result.",
    args: [],
    returns: "JSON with height, weight, and result.",
    run: function () {
      return JSON.stringify(withResult(readState()));
    }
  },

  set_height: {
    summary: "Persist height in centimeters and return updated BMI state.",
    args: [
      { name: "height_cm", required: true, summary: "height in centimeters" }
    ],
    returns: "JSON with height, weight, and result.",
    run: function (args, usage) {
      var state = setMeasurement("height", args[0]);
      return state ? JSON.stringify(state) : usage();
    }
  },

  set_weight: {
    summary: "Persist weight in kilograms and return updated BMI state.",
    args: [
      { name: "weight_kg", required: true, summary: "weight in kilograms" }
    ],
    returns: "JSON with height, weight, and result.",
    run: function (args, usage) {
      var state = setMeasurement("weight", args[0]);
      return state ? JSON.stringify(state) : usage();
    }
  },

  calculate: {
    summary: "Calculate BMI from metric height and weight, or saved state when no args are given.",
    args: [
      { name: "height_cm", required: false, summary: "height in centimeters" },
      { name: "weight_kg", required: false, summary: "weight in kilograms" }
    ],
    returns: "JSON with bmi and category.",
    run: function (args, usage) {
      var state = args.length >= 2 ? { height: args[0], weight: args[1] } : readState();
      var result = calculate(state.height, state.weight);
      return result ? JSON.stringify(result) : usage();
    }
  }
};
