ctx.resource.search.upsert("doc-1", "the quick brown fox");
ctx.resource.search.upsert("doc-2", "lazy dog sleeps all day");

const embed = JSON.parse(ctx.resource.localModel.embed("quick brown fox"));
ctx.resource.search.setEmbedding("doc-1", JSON.stringify(embed));

const queryVec = JSON.parse(ctx.resource.localModel.embedQuery("fox"));
return ctx.resource.search.query(
  "fox",
  JSON.stringify({ limit: 5, queryVec })
);