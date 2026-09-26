"use strict";
/* ============================== 通用确认模态 ============================== */
let modalAction = null;
let modalDismiss = null;
let modalOpener = null;
function modalFocusable() {
  return Array.from($("modal").querySelectorAll(
    'button:not([disabled]), [href], input:not([disabled]), select:not([disabled]), textarea:not([disabled]), [tabindex]:not([tabindex="-1"])'
  )).filter((node) => !node.hidden);
}
function openModal(title, bodyNode, okLabel, action, opts) {
  modalOpener = document.activeElement instanceof HTMLElement ? document.activeElement : null;
  $("modalTitle").textContent = "";
  if (opts && opts.icon) { const i = icon(opts.icon, 19); i.style.color = "var(--danger)"; $("modalTitle").appendChild(i); }
  $("modalTitle").appendChild(el("span", { text: title }));
  $("modalBody").textContent = "";
  $("modalBody").appendChild(bodyNode);
  $("modalOk").textContent = okLabel;
  // 默认危险红（历史上模态只用于删除类确认）；非破坏性动作传 okStyle:
  // "primary" 用琥珀主按钮，别把「保存」渲染成要出事的样子。
  $("modalOk").className = "btn " + ((opts && opts.okStyle === "primary") ? "primary" : "danger-fill");
  modalAction = action;
  modalDismiss = (opts && opts.onDismiss) || null;
  $("modalMask").classList.add("open");
  syncOverlayHistory();
  $("modalCancel").focus();
}
function closeModal() {
  if (!$("modalMask").classList.contains("open")) return;
  const opener = modalOpener;
  const dismiss = modalDismiss;
  $("modalMask").classList.remove("open");
  modalAction = null;
  modalDismiss = null;
  modalOpener = null;
  syncOverlayHistory();
  if (opener && document.contains(opener)) requestAnimationFrame(() => opener.focus());
  if (dismiss) {
    try { dismiss(); } catch (_) {}
  }
}
$("modalCancel").addEventListener("click", closeModal);
$("modalMask").addEventListener("click", (e) => { if (e.target === $("modalMask")) closeModal(); });
document.addEventListener("keydown", (e) => {
  if (!$("modalMask").classList.contains("open")) return;
  if (e.key === "Escape") { closeModal(); return; }
  if (e.key !== "Tab") return;
  const focusable = modalFocusable();
  if (!focusable.length) { e.preventDefault(); return; }
  const current = focusable.indexOf(document.activeElement);
  const next = e.shiftKey
    ? (current <= 0 ? focusable.length - 1 : current - 1)
    : (current === focusable.length - 1 ? 0 : current + 1);
  e.preventDefault();
  focusable[next].focus();
});
$("modalOk").addEventListener("click", async () => {
  const act = modalAction;
  modalDismiss = null;
  closeModal();
  if (act) await act();
});
