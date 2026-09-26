"use strict";
/* ============================== 本地统一搜索 ============================== */
const SEARCH_SOURCES = [
  { view: "agenda", label: "计划 · 日程", roots: ["agendaBox"] },
  { view: "notifs", label: "计划 · 提醒", roots: ["dueBox", "notifBox"] },
  { view: "suggest", label: "计划 · 建议", roots: ["suggestBox"] },
  { view: "ledger", label: "记忆 · 台账", roots: ["ledgerBox"] },
  { view: "journal", label: "记忆 · 行为", roots: ["journalBox"] },
  { view: "wearable", label: "记忆 · 健康趋势", roots: ["wearableBox", "wearableOut"] },
  { view: "review", label: "记忆 · 复盘", roots: ["reviewOut"] },
  { view: "captures", label: "记忆 · 通知", roots: ["notificationLedger"] },
];
const SEARCH_QUICK = [
  { view: "ledger", label: "记忆台账", desc: "查看 Solum 已经记住的事实、事件与来源。", icon: "memory" },
  { view: "agenda", label: "计划与日程", desc: "回到即将发生的安排和时间节点。", icon: "calendar" },
  { view: "journal", label: "行为记录", desc: "按原始顺序回看最近的状态与活动。", icon: "journal" },
  { view: "captures", label: "通知回看", desc: "找回过滤、判重或仍待处理的通知。", icon: "inbox" },
];
function searchText(value) {
  return String(value || "").replace(/\s+/g, " ").trim();
}
function collectSearchEntries() {
  const entries = [];
  const seen = new Set();
  const selectors = ".ev-row,.notif-row,.row,.mail-row,.gc-card,.tp-item,tbody tr,.empty,.statusline";
  for (const source of SEARCH_SOURCES) {
    for (const rootId of source.roots) {
      const root = $(rootId);
      if (!root) continue;
      const candidates = Array.from(root.querySelectorAll(selectors));
      const nodes = candidates.length ? candidates : [root];
      for (const node of nodes) {
        const text = searchText(node.textContent);
        if (!text || text.length < 2) continue;
        const key = source.view + "\u0000" + text;
        if (seen.has(key)) continue;
        seen.add(key);
        entries.push({ view: source.view, label: source.label, text });
        if (entries.length >= 240) return entries;
      }
    }
  }
  return entries;
}
function searchResultButton(entry) {
  const title = entry.text.length > 76 ? entry.text.slice(0, 76) + "…" : entry.text;
  return el("button", { class: "search-result", type: "button", onclick: () => showView(entry.view) }, [
    el("span", null, [
      el("span", { class: "search-result-type", text: entry.label }),
      el("span", { class: "search-result-title", text: title }),
      el("span", { class: "search-result-copy", text: entry.text }),
    ]),
    icon("chev", 20),
  ]);
}
function renderSearchQuick() {
  const box = $("globalSearchResults");
  box.textContent = "";
  const grid = el("div", { class: "search-quick-grid" });
  for (const item of SEARCH_QUICK) {
    grid.appendChild(el("button", { class: "search-quick", type: "button", onclick: () => showView(item.view) }, [
      el("span", null, [
        el("span", { class: "search-result-type", text: "LOCAL INDEX" }),
        el("span", { class: "search-result-title", text: item.label }),
        el("span", { class: "search-result-copy", text: item.desc }),
      ]),
      icon(item.icon, 24),
    ]));
  }
  box.appendChild(grid);
  if ($("globalSearchHead")) $("globalSearchHead").hidden = true;
}
function runGlobalSearch() {
  const query = searchText($("globalSearchInput").value).toLocaleLowerCase();
  if (!query) { renderSearchQuick(); return; }
  const terms = query.split(" ").filter(Boolean);
  const matches = collectSearchEntries().filter((entry) => {
    const haystack = entry.text.toLocaleLowerCase();
    return terms.every((term) => haystack.includes(term));
  }).slice(0, 40);
  const box = $("globalSearchResults");
  box.textContent = "";
  if ($("globalSearchHead")) $("globalSearchHead").hidden = false;
  $("globalSearchHeading").textContent = "搜索结果";
  $("globalSearchCount").textContent = `${matches.length} 条`;
  if (!matches.length) {
    box.appendChild(emptyBox("search", "没有找到匹配内容", "换个更短的关键词，或先在「今天」告诉 Solum 要记住什么。"));
    return;
  }
  for (const entry of matches) box.appendChild(searchResultButton(entry));
}
$("globalSearchIcon").appendChild(icon("search", 22));
$("globalSearchForm").addEventListener("submit", (event) => {
  event.preventDefault();
  runGlobalSearch();
});
let globalSearchTimer = 0;
$("globalSearchInput").addEventListener("input", () => {
  clearTimeout(globalSearchTimer);
  globalSearchTimer = setTimeout(runGlobalSearch, 160);
});
renderSearchQuick();
