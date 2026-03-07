document.addEventListener("DOMContentLoaded", async () => {
  const resultDiv = document.getElementById("result");
  const data = await chrome.storage.local.get("lastResult");

  if (data.lastResult) {
    const r = data.lastResult;
    if (r.title) {
      resultDiv.className = "status ok";
      let html = `<strong>${r.title}</strong>`;
      if (r.tags && r.tags.length) {
        html += `<div class="tags">${r.tags.map(t => `#${t}`).join(", ")}</div>`;
      }
      if (r.folder) {
        html += `<div class="tags">Folder: ${r.folder}</div>`;
      }
      resultDiv.innerHTML = html;
    } else if (r.status && r.status.Failed) {
      resultDiv.className = "status err";
      resultDiv.textContent = `Failed: ${r.status.Failed.reason}`;
    }
  }

  document.getElementById("capture").addEventListener("click", async () => {
    const [tab] = await chrome.tabs.query({ active: true, currentWindow: true });
    if (!tab || !tab.url) return;

    const endpoint = (await chrome.storage.local.get("endpoint")).endpoint || "http://localhost:8181";
    resultDiv.className = "status none";
    resultDiv.textContent = "Sending...";

    try {
      const response = await fetch(`${endpoint}/ingest`, {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ url: tab.url }),
      });
      const result = await response.json();
      await chrome.storage.local.set({ lastResult: result });

      if (result.title) {
        resultDiv.className = "status ok";
        resultDiv.innerHTML = `<strong>${result.title}</strong>`;
      } else {
        resultDiv.className = "status err";
        resultDiv.textContent = "Failed";
      }
    } catch (err) {
      resultDiv.className = "status err";
      resultDiv.textContent = `Error: ${err.message}`;
    }
  });
});
