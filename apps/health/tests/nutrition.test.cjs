const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const test = require("node:test");
const vm = require("node:vm");

function loadBackend(initial) {
  const values = Object.assign({}, initial);
  const blobs = {};
  const syncValues = {};
  const kv = {
    get(key) {
      return Object.prototype.hasOwnProperty.call(values, key)
        ? values[key]
        : null;
    },
    set(key, value) {
      values[key] = String(value);
    },
    rm(key) {
      delete values[key];
    },
    all() {
      return Object.assign({}, values);
    },
  };
  const context = {
    ctx: {
      resource: {
        kv,
        crdt: {
          mapGet(_doc, key) {
            return Object.prototype.hasOwnProperty.call(syncValues, key)
              ? syncValues[key]
              : null;
          },
          mapSet(_doc, key, value) {
            syncValues[key] = String(value);
          },
          mapDel(_doc, key) {
            delete syncValues[key];
          },
          mapAll() {
            return Object.assign({}, syncValues);
          },
        },
        crypto: {
          randomId() {
            const next = Object.keys(syncValues).length + 1;
            return JSON.stringify({
              ok: true,
              id: next.toString(16).padStart(32, "0"),
            });
          },
        },
        blob: {
          put(name, mime, base64) {
            blobs[name] = { mime, base64 };
          },
          rm(name) {
            delete blobs[name];
          },
          stat() {
            return "{}";
          },
        },
        model: {
          ask() {
            throw new Error("model.ask is not used by these tests");
          },
        },
      },
    },
    console,
  };
  vm.createContext(context);
  vm.runInContext(
    fs.readFileSync(path.join(__dirname, "..", "main.js"), "utf8"),
    context,
    { filename: "apps/health/main.js" },
  );
  return { context, values, blobs, syncValues };
}

function dish(name, calories, protein) {
  return {
    food_name: name,
    confidence: 0.8,
    serving_description: "one visible portion",
    calories_kcal: calories,
    protein_g: protein,
    carbs_g: calories / 10,
    fat_g: calories / 40,
    fiber_g: 2,
    sugar_g: 1,
    sodium_mg: 100,
  };
}

test("multiple visible dishes remain separate and produce canonical meal totals", () => {
  const { context } = loadBackend();
  const normalized = context.normalizeEstimate({
    food_name: "Lunch plate",
    confidence: 0.7,
    serving_description: "one plate",
    calories_kcal: 9999,
    dishes: [dish("Grilled chicken", 300, 40), dish("Steamed rice", 220, 4)],
  });

  assert.equal(normalized.food_name, "Lunch plate");
  assert.equal(normalized.dishes.length, 2);
  assert.equal(normalized.dishes[0].food_name, "Grilled chicken");
  assert.equal(normalized.dishes[1].food_name, "Steamed rice");
  assert.equal(normalized.calories_kcal, 520);
  assert.equal(normalized.protein_g, 44);
  assert.equal(normalized.sodium_mg, 200);
});

test("single-dish and legacy estimates remain compatible", () => {
  const { context } = loadBackend();
  const separated = context.normalizeEstimate({
    dishes: [dish("Soup", 180, 8)],
  });
  assert.equal(separated.dishes.length, 1);
  assert.equal(separated.food_name, "Soup");
  assert.equal(separated.calories_kcal, 180);

  const legacy = context.normalizeEstimate(dish("Legacy sandwich", 410, 18));
  assert.deepEqual(Array.from(legacy.dishes), []);
  assert.equal(legacy.food_name, "Legacy sandwich");
  assert.equal(legacy.calories_kcal, 410);
});

test("review updates persist dish data and cannot override its combined total", () => {
  const existing = {
    id: "00000001",
    blob_name: "meal/00000001.jpg",
    note: "",
    provider: "opencode",
    model: "opencode-go/kimi-k2.6",
    eaten_at: "2026-07-27T02:00:00.000Z",
  };
  const { context, values } = loadBackend({
    "estimate:00000001": JSON.stringify(existing),
  });
  const changes = {
    food_name: "Reviewed plate",
    confidence: 0.8,
    serving_description: "one plate",
    calories_kcal: 9999,
    dishes: [dish("Fish", 250, 30), dish("Rice", 200, 4)],
    eaten_at: "2026-07-27T03:00:00.000Z",
  };

  const result = JSON.parse(
    context.actions.update.run(
      ["00000001", JSON.stringify(changes)],
      () => {
        throw new Error("usage should not be called");
      },
    ),
  );

  assert.equal(result.ok, true);
  assert.equal(result.estimate.dishes.length, 2);
  assert.equal(result.estimate.calories_kcal, 450);
  assert.equal(result.estimate.protein_g, 34);
  assert.equal(JSON.parse(values["estimate:00000001"]).dishes.length, 2);
});

test("legacy meals migrate into the CRDT sync document without losing local state", () => {
  const existing = {
    id: "00000001",
    food_name: "Saved lunch",
    eaten_at: "2026-07-27T02:00:00.000Z",
  };
  const { context, syncValues } = loadBackend({
    "estimate:00000001": JSON.stringify(existing),
    "settings:model": JSON.stringify({
      provider: "opencode",
      model: "opencode-go/kimi-k2.6",
    }),
  });

  const prepared = JSON.parse(context.actions.sync_prepare.run());
  assert.equal(prepared.ok, true);
  assert.equal(prepared.entries, 1);
  assert.deepEqual(
    JSON.parse(syncValues["estimate:00000001"]),
    existing,
  );
  assert.equal(
    JSON.parse(syncValues.settings).model,
    "opencode-go/kimi-k2.6",
  );
});

test("vision prompt requires bounded per-dish output", () => {
  const { context } = loadBackend();
  const prompt = context.promptFor("");
  assert.match(prompt, /every distinct visible dish/i);
  assert.match(prompt, /up to 12/i);
  assert.match(prompt, /Do not return combined nutrient totals/i);
});

test("encrypted cross-device analysis imports once and keeps the meal reviewable", () => {
  const { context, values, blobs } = loadBackend();
  const payload = {
    contract: "terrane.health-nutrition-result.v1",
    provider: "opencode",
    model: "opencode-go/kimi-k2.6",
    note: "no dipping sauce",
    completed_at: "2026-07-31T02:00:00.000Z",
    estimate: {
      dishes: [dish("Grilled meatballs", 420, 24)],
      assumptions: ["one photographed plate"],
      warnings: [],
    },
  };
  const usage = () => {
    throw new Error("usage should not be called");
  };

  const first = JSON.parse(
    context.actions.import_analysis.run(
      [
        "job-123",
        Buffer.from("jpeg fixture").toString("base64"),
        "image/jpeg",
        JSON.stringify(payload),
      ],
      usage,
    ),
  );
  assert.equal(first.ok, true);
  assert.equal(first.idempotent, false);
  assert.equal(first.estimate.source_job_id, "job-123");
  assert.equal(first.estimate.reviewed, false);
  assert.equal(first.estimate.provider, "opencode");
  assert.equal(first.estimate.model, "opencode-go/kimi-k2.6");
  assert.equal(first.estimate.note, "no dipping sauce");
  assert.equal(blobs[first.estimate.blob_name].mime, "image/jpeg");

  const second = JSON.parse(
    context.actions.import_analysis.run(
      [
        "job-123",
        Buffer.from("different bytes").toString("base64"),
        "image/jpeg",
        JSON.stringify(payload),
      ],
      usage,
    ),
  );
  assert.equal(second.ok, true);
  assert.equal(second.idempotent, true);
  assert.equal(second.estimate.id, first.estimate.id);
  assert.equal(values["analysis-job:job-123"], first.estimate.id);
});

test("route links are allowlisted, bounded, queued, and legacy items map to meals", () => {
  const { context, values } = loadBackend();
  const usage = () => {
    throw new Error("usage should not be called");
  };

  let result = JSON.parse(
    context.actions["common.receive"].run(
      [
        "link",
        JSON.stringify({
          version: 1,
          route: "calendar",
          segments: [],
          params: { date: "2026-07-27", period: "week" },
        }),
      ],
      usage,
    ),
  );
  assert.equal(result.ok, true);
  result = JSON.parse(context.actions["navigation.consume"].run([], usage));
  assert.deepEqual(
    JSON.parse(JSON.stringify(result.navigation)),
    {
      route: "calendar",
      segments: [],
      params: { date: "2026-07-27", period: "week" },
    },
  );
  assert.equal(values["navigation:pending"], undefined);

  result = JSON.parse(
    context.actions["common.receive"].run(
      ["link", JSON.stringify({ item: "00000003" })],
      usage,
    ),
  );
  assert.equal(result.ok, true);
  result = JSON.parse(context.actions["navigation.consume"].run([], usage));
  assert.equal(result.navigation.route, "meal");
  assert.deepEqual(Array.from(result.navigation.segments), ["00000003"]);

  for (const route of ["add", "calendar", "history", "insights", "settings"]) {
    result = JSON.parse(
      context.actions["common.receive"].run(
        ["link", JSON.stringify({ route, segments: [], params: {} })],
        usage,
      ),
    );
    assert.equal(result.ok, true, `${route} should be deep-linkable`);
    result = JSON.parse(context.actions["navigation.consume"].run([], usage));
    assert.equal(result.navigation.route, route);
  }
  result = JSON.parse(
    context.actions["common.receive"].run(
      [
        "link",
        JSON.stringify({ route: "meal", segments: ["00000003"], params: {} }),
      ],
      usage,
    ),
  );
  assert.equal(result.ok, true);
  result = JSON.parse(context.actions["navigation.consume"].run([], usage));
  assert.equal(result.navigation.route, "meal");

  for (
    const payload of [
      { route: "admin", segments: [], params: {} },
      { route: "meal", segments: [], params: {} },
      { route: "calendar", segments: ["unexpected"], params: {} },
      { route: "insights", segments: [], params: { period: "year" } },
    ]
  ) {
    result = JSON.parse(
      context.actions["common.receive"].run(
        ["link", JSON.stringify(payload)],
        usage,
      ),
    );
    assert.equal(result.ok, false);
  }
});
