const usersSpec = {
  specVersion: 1,
  schemaVersion: 1,
  fields: {
    tenantId: { type: "string", required: true, minLength: 1 },
    userId: { type: "string", required: true, minLength: 1 },
    email: { type: "string", required: true, format: "email" },
    name: { type: "string", required: true },
    status: { type: "string", enum: ["active", "disabled"], default: "active" },
    createdAt: { type: "string", required: true, format: "date-time" }
  },
  primaryKey: { partition: ["tenantId"], sort: ["userId"] },
  indexes: {
    byEmail: {
      partition: ["tenantId", "email"],
      unique: true,
      projection: { type: "include", fields: ["name", "status"] }
    },
    byStatus: {
      partition: ["tenantId", "status"],
      sort: ["createdAt", "userId"],
      projection: { type: "all" }
    }
  },
  constraints: {
    uniqueTenantEmail: { type: "unique", fields: ["tenantId", "email"] }
  },
  options: { unknownFields: "reject", defaultQueryLimit: 25, maxQueryLimit: 100, canonicalJson: true }
};

ctx.resource.relational_db.defineTable("users", JSON.stringify(usersSpec));
ctx.resource.relational_db.put("users", JSON.stringify({
  tenantId: "acme", userId: "u_1", email: "ada@example.com", name: "Ada", createdAt: "2026-06-28T00:00:00Z"
}));

const byEmail = ctx.resource.relational_db.query("users", "byEmail", JSON.stringify({
  partition: { tenantId: "acme", email: "ada@example.com" }, select: "rows", limit: 1
}));
