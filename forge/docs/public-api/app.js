const filter = document.getElementById("api-filter");
if (filter) {
  filter.addEventListener("input", () => {
    const q = filter.value.trim().toLowerCase();
    document.querySelectorAll(".command-table tbody tr").forEach((row) => {
      const hay = (row.dataset.name + " " + row.dataset.summary).toLowerCase();
      row.classList.toggle("is-hidden", q.length > 0 && !hay.includes(q));
    });
    document.querySelectorAll(".api-card, .example-card").forEach((card) => {
      const hay = card.textContent.toLowerCase();
      card.classList.toggle("is-hidden", q.length > 0 && !hay.includes(q));
    });
  });
}