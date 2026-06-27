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

var description = "A React-powered BMI calculator.";

var actions = {
  calculate: {
    summary: "Calculate BMI from metric height and weight.",
    args: [
      { name: "height_cm", required: true, summary: "height in centimeters" },
      { name: "weight_kg", required: true, summary: "weight in kilograms" }
    ],
    returns: "JSON with bmi and category.",
    run: function (args, usage) {
      var result = calculate(args[0], args[1]);
      return result ? JSON.stringify(result) : usage();
    }
  }
};
