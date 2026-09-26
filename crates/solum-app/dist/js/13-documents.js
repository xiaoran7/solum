"use strict";
async function getDocument(id) {
  return withDocuments("readonly", (store, set) => {
    const request = store.get(id);
    request.onsuccess = () => set(request.result || null);
  });
}
async function putDocument(file) {
  return withDocuments("readwrite", (store) => {
    store.put({
      id: `doc-${Date.now()}-${Math.random().toString(36).slice(2, 8)}`,
      name: file.name || "未命名资料",
      type: file.type || "",
      size: file.size || 0,
      addedAt: new Date().toISOString(),
      blob: file,
    });
  });
}
async function removeDocument(id) {
  return withDocuments("readwrite", (store) => { store.delete(id); });
}
function readableDocumentSize(size) {
  if (size < 1024 * 1024) return `${Math.max(1, Math.round(size / 1024))} KB`;
  return `${(size / (1024 * 1024)).toFixed(1)} MB`;
}
function isTextDocument(doc) {
  return /^text\//.test(doc.type || "") || /\.(txt|md|markdown)$/i.test(doc.name || "");
}
async function addDocumentToChat(id) {
  try {
    const doc = await getDocument(id);
    if (!doc || !isTextDocument(doc)) return;
    const text = (await doc.blob.text()).trim();
    if (!text) return toast("这份文本资料没有可加入的内容", "danger");
    const excerpt = text.slice(0, 12_000);
    closeDrawer();
    showView("chat");
    $("chatInput").value = `请基于我手动附上的资料《${doc.name}》回答：\n\n${excerpt}${text.length > excerpt.length ? "\n\n[资料过长，已截取前 12000 字符]" : ""}`;
    $("chatInput").focus();
    toast("资料已加入输入框；发送前可继续修改", "ok");
  } catch (e) { toast(`无法读取资料：${String(e)}`, "danger"); }
}
async function refreshDocuments() {
  const box = $("documentsBox");
  // 资料面板没有挂载在当前壳层时，保留本地资料实现但不让轮询链路报错。
  if (!box) return;
  try {
    const docs = (await listDocuments()).sort((a, b) => String(b.addedAt).localeCompare(String(a.addedAt)));
    box.textContent = "";
    if (!docs.length) {
      box.appendChild(emptyBox("folder", "还没有本地资料", "选择一份 PDF 或文档后，它只保存在这台设备，可随时删除。"));
      return;
    }
    const list = el("div", { class: "rows" });
    for (const doc of docs) {
      const isText = isTextDocument(doc);
      list.appendChild(el("div", { class: "doc-source" }, [
        icon(doc.type === "application/pdf" || /\.pdf$/i.test(doc.name) ? "doc" : "folder", 17),
        el("div", { class: "name", text: doc.name }),
        el("span", { class: "meta", text: `${readableDocumentSize(doc.size)} · ${sessionTime(doc.addedAt)}` }),
        isText ? el("button", { class: "btn sm", type: "button", text: "加入对话", onclick: () => addDocumentToChat(doc.id) }) : null,
        el("button", { class: "btn icon danger", type: "button", title: "删除本地资料", "aria-label": `删除资料：${doc.name}`, onclick: async () => {
          try { await removeDocument(doc.id); await refreshDocuments(); toast("已从本机资料库删除", "ok"); }
          catch (e) { toast(String(e), "danger"); }
        } }, icon("trash", 16)),
      ]));
    }
    box.appendChild(list);
  } catch (e) {
    box.textContent = "";
    box.appendChild(emptyBox("alert", "无法读取本地资料", safeUserError(e, "本地资料暂时无法读取，请稍后重试。")));
  }
}
$("documentPickBtn")?.addEventListener("click", () => $("documentInput")?.click());
$("documentInput")?.addEventListener("change", async () => {
  const files = Array.from($("documentInput").files || []);
  $("documentInput").value = "";
  if (!files.length) return;
  try {
    const existing = await listDocuments();
    let room = Math.max(0, DOCUMENT_MAX_COUNT - existing.length);
    let saved = 0;
    const rejected = [];
    for (const file of files) {
      if (!room) { rejected.push("资料库最多保留 30 份文件"); break; }
      if (file.size > DOCUMENT_MAX_BYTES) { rejected.push(`《${file.name}》超过 20 MB`); continue; }
      await putDocument(file);
      room -= 1; saved += 1;
    }
    await refreshDocuments();
    if (saved) { toast(`已在本机保存 ${saved} 份资料`, "ok"); openDrawer("documents"); }
    if (rejected.length) toast(rejected.join("；"), "danger", 4200);
  } catch (e) { toast(`保存资料失败：${String(e)}`, "danger"); }
});
